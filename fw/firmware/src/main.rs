//! yotonrs firmware (docs/plan/02-architecture.md §4, WP SYS-1).
//!
//! Start-up (`main`, resident): NOR, the pager from the slot header, HAL,
//! clock level, then the tasks:
//!
//! | Task | Executor | Job |
//! |---|---|---|
//! | `input` | main | key ladders every 10 ms → debounced edges |
//! | `ui` | main | shell, screens, drawing, settings writes |
//! | `power` | main | charger state machine, battery gauge (1 Hz) |
//! | `usb` | main | embassy-usb device with the developer channel |
//! | `health` | main | marks the start healthy after 30 s |
//! | `watchdog` | main | feeds the watchdog when RT, main and UI progress |
//! | `rt_beat` | RT | RT executor heartbeat for `watchdog` |

#![no_std]
#![no_main]
// `&mut *(&raw mut STATIC)` borrows a `static mut` buffer (2024 edition).
#![allow(clippy::deref_addrof)]

mod audio;
mod media;
mod pictures;
mod services;
mod state;
mod ui;
mod usbdev;

use atj2157_hal::clock::{self, Level};
use atj2157_hal::{self as hal, executor, log, nor, pac, pager, persist, wdt};
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Ticker, Timer};
use static_cell::StaticCell;
use yoton_bsp::charger::{self, Charger};
use yoton_bsp::keys;
use yoton_power::gauge;
use yoton_proto::layout;
use yoton_proto::slot::{LAYOUT_FULL, SlotHeader};

/// Key edges from the ladders and from the developer channel.
pub static KEYS: Channel<CriticalSectionRawMutex, (yoton_input::Key, bool), 16> = Channel::new();
/// Battery/cable changes for the UI.
pub static BATTERY: Channel<CriticalSectionRawMutex, (), 2> = Channel::new();

#[embassy_executor::task]
async fn input() {
    keys::init();
    let mut deb = keys::Debouncer::new();
    let mut ticker = Ticker::every(Duration::from_millis(10));
    loop {
        ticker.next().await;
        let (ev, n) = deb.feed(keys::decode(keys::sample()));
        for e in &ev[..n] {
            let _ = KEYS.try_send((e.key, e.down));
        }
    }
}

#[embassy_executor::task]
async fn power() {
    charger::init();
    let mut c = Charger::new();
    let mut smooth = gauge::Smoother::default();
    let mut last_pct = 255u8;
    let mut ticker = Ticker::every(Duration::from_secs(1));
    loop {
        let i = charger::inputs();
        let cmd = c.step(i);
        charger::apply(&mut c, cmd).await;
        let raw = smooth.feed(i.battery_raw);
        let mv = gauge::raw_to_mv(raw);
        let pct = gauge::percent(mv);
        let charge = c.state();
        state::update(|s| {
            s.percent = pct;
            s.millivolts = mv as u16;
            s.charge = charge;
            s.cable = i.cable;
        });
        if pct != last_pct {
            last_pct = pct;
            let _ = BATTERY.try_send(());
        }
        ticker.next().await;
    }
}

/// Feeds the watchdog only when the RT executor and the UI loop have made
/// progress (this task running proves the main executor; 02 §8).
#[embassy_executor::task]
async fn watchdog() {
    let mut h = wdt::Health::new();
    let mut gov = Governor::new();
    loop {
        Timer::after_millis(1000).await;
        h.check();
        if GOV_ON.load(core::sync::atomic::Ordering::Relaxed) {
            gov.step();
        }
    }
}

/// Lowest clock level the governor may choose (0 = Base, 1 = Mid, 2 =
/// High). The audio task sets it from the codec and the DSP chain: at Base,
/// Vorbis/Opus, or MP3 + DSP, plus the UI overload the CPU (paging
/// included) and starve thread mode.
pub static CLOCK_FLOOR: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// Governor on/off (test command `gov on|off`).
pub static GOV_ON: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Clock governor (PWR-2, first cut): from the main executor's idle share
/// over the last second, one level up when idle < 25 %, one level down
/// after three seconds above 70 %. Range Base (48 MHz) .. High (144 MHz).
struct Governor {
    last_idle: u32,
    last_t: u64,
    calm: u8,
}

impl Governor {
    fn new() -> Self {
        Self { last_idle: executor::idle_us(), last_t: hal::time::now_us(), calm: 0 }
    }

    fn step(&mut self) {
        let idle = executor::idle_us();
        let t = hal::time::now_us();
        let dt = (t - self.last_t).max(1);
        let pct = u64::from(idle.wrapping_sub(self.last_idle)) * 100 / dt;
        self.last_idle = idle;
        self.last_t = t;
        let cur = clock::current();
        if pct < 25 {
            self.calm = 0;
            let up = match cur {
                Level::Low | Level::Base => Some(Level::Mid),
                Level::Mid => Some(Level::High),
                _ => None,
            };
            if let Some(l) = up {
                clock::set_level(l);
            }
        } else if pct > 70 {
            self.calm = self.calm.saturating_add(1);
            if self.calm >= 3 {
                self.calm = 0;
                let floor = CLOCK_FLOOR.load(core::sync::atomic::Ordering::Relaxed);
                let down = match cur {
                    Level::Turbo => Some(Level::High),
                    Level::High if floor < 2 => Some(Level::Mid),
                    Level::Mid if floor < 1 => Some(Level::Base),
                    _ => None,
                };
                if let Some(l) = down {
                    clock::set_level(l);
                }
            }
        } else {
            self.calm = 0;
        }
    }
}

/// RT executor heartbeat (the audio task may sleep for long periods).
#[embassy_executor::task]
async fn rt_beat() {
    loop {
        wdt::beat(&wdt::RT_BEAT);
        Timer::after_millis(250).await;
    }
}

#[embassy_executor::task]
async fn health() {
    // Started by the HIL harness: return to USB recovery after args[0]
    // seconds (default 120) whatever happens, so a test can never leave the
    // player needing a hand.
    if let Some((_, m)) = persist::test_request() {
        let secs = if m.args[0] == 0 { 120 } else { m.args[0] };
        Timer::after_secs(u64::from(secs)).await;
        log::line(b"fw: test window over");
        wdt::reboot(wdt::Target::Adfu);
    }
    Timer::after_secs(30).await;
    persist::clear_boot_counter();
    log::line(b"fw: healthy start");
}

/// The slot this firmware runs from: the one stage 2 recorded in the
/// boot-info word (path 1, slot index in the low byte), otherwise (started
/// from RAM by a test) the first slot holding a valid header.
#[inline(never)]
fn find_slot(h: &mut [u8; 128]) -> Option<(u32, SlotHeader)> {
    // SAFETY: persistent boot-info word written by stage 2.
    let bi = unsafe { core::ptr::read_volatile(0x0013_7FFC as *const u32) };
    let recorded = (bi >> 24 == 0x42 && (bi >> 16) & 0xFF == 1).then_some((bi & 0xFF) as usize);
    let mut i = 0;
    while i < layout::SLOTS.len() {
        let idx = recorded.unwrap_or(i);
        let base = layout::SLOTS.get(idx).copied()?;
        if nor::read(base, h).is_ok()
            && let Ok(hdr) = SlotHeader::from_bytes(h)
        {
            return Some((base, hdr));
        }
        if recorded.is_some() {
            return None;
        }
        i += 1;
    }
    None
}

/// Start the pager from the slot header in NOR. Returns whether paging is
/// on. Resident (a root in resident.toml).
#[inline(never)]
fn start_pager() -> bool {
    nor::init();
    let mut h = [0u8; 128];
    match find_slot(&mut h) {
        Some((base, hdr)) if hdr.code_pages + hdr.rodata_pages > 0 => {
            if hdr.layout == LAYOUT_FULL {
                nor::allow_full_layout();
            }
            pager::init(base + hdr.pages_off, hdr.code_pages, hdr.rodata_pages);
            true
        }
        _ => false,
    }
}

static MAIN: StaticCell<executor::MainExecutor> = StaticCell::new();

/// Resident (`[bin.firmware]` in resident.toml): runs before the pager.
#[unsafe(no_mangle)]
extern "C" fn main() -> ! {
    // Watchdog first: a hang anywhere in start-up must end in a reset (and
    // after three, in USB recovery), never in a player that needs a hand.
    wdt::start(pac::rtc::wd::PERIOD_5S6);
    // Keep the previous boot's last interrupt sample (rt::SAMPLE_*) for
    // post-mortems of watchdog resets: 0x137FDC = exception, 0x137FE0 = PC.
    // SAFETY: persistent-area words, before any interrupt runs.
    unsafe {
        let e = core::ptr::read_volatile(hal::rt::SAMPLE_EXC as *const u32);
        let p = core::ptr::read_volatile(hal::rt::SAMPLE_PC as *const u32);
        core::ptr::write_volatile(0x0013_7FDC as *mut u32, e);
        core::ptr::write_volatile(0x0013_7FE0 as *mut u32, p);
    }
    pager::arm_deny();
    log::init();
    let paged = start_pager();
    hal::init();
    clock::set_level(Level::Base);
    log::line(if paged { b"fw: start (paged)" } else { b"fw: start (no pages)" });
    audio::init_chain();
    let rt = executor::start_rt();
    rt.spawn(audio::audio().unwrap());
    rt.spawn(rt_beat().unwrap());
    MAIN.init(executor::MainExecutor::new()).run(|s: Spawner| {
        s.spawn(input().unwrap());
        s.spawn(power().unwrap());
        s.spawn(ui::ui().unwrap());
        s.spawn(usbdev::usb().unwrap());
        s.spawn(health().unwrap());
        s.spawn(watchdog().unwrap());
    })
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    hal::panic(info)
}
