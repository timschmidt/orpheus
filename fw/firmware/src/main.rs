//! orpheus firmware (docs/plan/02-architecture.md §4, WP SYS-1).
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
//! | `watchdog` | main | feeds the watchdog when RT, main and UI progress and USB is not wedged; USB liveness (USB-W1) |
//! | `rt_beat` | RT | RT executor heartbeat for `watchdog` |
//! | `bt` | main | Bluetooth transmitter module (off until asked) |
//! | `fm` | main | FM tuner and its route to the amplifier (off until asked) |
//! | `sensor` | main | accelerometer at 25 Hz while steps are counted (SENS-1) |

#![no_std]
#![no_main]
// `&mut *(&raw mut STATIC)` borrows a `static mut` buffer (2024 edition).
#![allow(clippy::deref_addrof)]

mod audio;
mod books;
mod bt;
mod drain;
mod drive;
mod fm;
mod frame;
mod games;
mod kbd;
mod library;
mod media;
mod pictures;
mod rec;
mod sensor;
mod services;
mod state;
mod ui;
mod update;
mod usbdev;
mod vault;

use atj2157_hal::clock::{self, Level};
use atj2157_hal::{self as hal, executor, log, nor, pac, pager, persist, wdt};
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Ticker, Timer};
use orpheus_power::{gauge, governor};
use orpheus_proto::layout;
use orpheus_proto::slot::{LAYOUT_FULL, SlotHeader};
use static_cell::StaticCell;
use yoton_bsp::charger::{self, Charger};
use yoton_bsp::keys;

/// Key edges from the ladders and from the developer channel.
pub static KEYS: Channel<CriticalSectionRawMutex, (orpheus_input::Key, bool), 16> = Channel::new();
/// Battery/cable changes for the UI.
pub static BATTERY: Channel<CriticalSectionRawMutex, (), 2> = Channel::new();

#[embassy_executor::task]
async fn input() {
    keys::init();
    let mut deb = keys::Debouncer::new();
    let mut ticker = Ticker::every(Duration::from_millis(10));
    // Earphone sense (the radio's antenna note): ladder 1's idle level,
    // classified only while no ladder-1 key is down, after 11 equal
    // samples as the vendor's key driver does (fm-i2c-imu.md §2.6).
    let mut ear = (false, 0u8);
    loop {
        ticker.next().await;
        let s = keys::sample();
        let level = s.ladder1 & 0x3FF;
        let sensed = if level >= 592 { Some(false) } else { keys::earphones_present(s).then_some(true) };
        if let Some(x) = sensed {
            ear = if x == ear.0 { (x, ear.1.saturating_add(1)) } else { (x, 1) };
            if ear.1 == 11 {
                state::EARPHONES.store(x, core::sync::atomic::Ordering::Relaxed);
            }
        }
        let (ev, n) = deb.feed(keys::decode(s));
        if n > 0 {
            send_keys(&ev[..n]);
        }
    }
}

/// Key edges to the UI. Out of the key tick's poll: the tick runs 100
/// times a second and shares its code-cache line with the main executor's
/// loop and the synth's wake-up entry (`hot.toml` `synth-block`); an edge
/// is rare.
#[cold]
#[inline(never)]
fn send_keys(ev: &[orpheus_input::Edge]) {
    for e in ev {
        let _ = KEYS.try_send((e.key, e.down));
    }
}

/// Test command `charge pause|resume` (usbdev): charging paused.
pub static CHARGE_PAUSE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

#[embassy_executor::task]
async fn power() {
    charger::init();
    let mut c = Charger::new();
    let mut smooth = gauge::Smoother::default();
    let mut last_pct = 255u8;
    let mut ticker = Ticker::every(Duration::from_secs(1));
    let mut paused = false;
    loop {
        let i = charger::inputs();
        // Test only (PWR-5 drain measurements on the cable): charging
        // paused with the vendor's stop sequence; resuming restarts the
        // charger state machine so it detects and charges again. A test
        // always ends in a reboot, which clears the flag.
        if CHARGE_PAUSE.load(core::sync::atomic::Ordering::Relaxed) {
            if !paused {
                charger::stop_now();
                paused = true;
            }
        } else {
            if paused {
                c = Charger::new();
                paused = false;
            }
            let cmd = c.step(i);
            charger::apply(&mut c, cmd).await;
        }
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
        if drain::active() {
            drain_sample(mv, i, pct, charge);
        }
        if pct != last_pct {
            last_pct = pct;
            let _ = BATTERY.try_send(());
        }
        ticker.next().await;
    }
}

/// The battery logger's second (PWR-5): reads only (battery, charger state,
/// clock level, what the other tasks publish).
#[inline(never)]
fn drain_sample(
    mv: u32,
    i: orpheus_power::charger::Inputs,
    percent: u8,
    charge: orpheus_power::charger::ChargeState,
) {
    use core::sync::atomic::Ordering::Relaxed;
    drain::second(orpheus_power::drain::Sample {
        uptime_s: (hal::time::now_us() / 1_000_000) as u32,
        mv: mv.min(0xFFFF) as u16,
        mv_raw: gauge::raw_to_mv(i.battery_raw).min(0xFFFF) as u16,
        percent,
        charge,
        cable: i.cable,
        level: clock::current(),
        playing: audio::PLAYING.load(Relaxed),
        backlight: drain::shared().backlight.load(Relaxed),
        screen: drain::shared().screen_on.load(Relaxed),
        bt: drain::bt_code(),
        card_io: yoton_bsp::card::io_count(),
    });
}

/// Feeds the watchdog only when the RT executor and the UI loop have made
/// progress (this task running proves the main executor; 02 §8).
#[embassy_executor::task]
async fn watchdog() {
    let mut h = wdt::Health::new();
    let mut gov = Governor::new();
    // USB-W1: USB liveness, here because this task runs whatever the USB
    // task does; sets `wdt::USB_WEDGED` only when recoveries cannot help.
    let mut usb_live = orpheus_usb::liveness::Liveness::new();
    loop {
        Timer::after_millis(1000).await;
        usbdev::liveness_tick(&mut usb_live);
        h.check();
        if GOV_ON.load(core::sync::atomic::Ordering::Relaxed) {
            gov.step();
        }
    }
}

/// Lowest clock level the audio task allows while something plays or a
/// picture decodes: 0 Base, 1 Mid, 2 High, 3 Turbo, 4 Max
/// (`governor::Level::from_floor`). The audio task sets it from the codec,
/// the DSP chain and the synth: at Base, Vorbis/Opus, or MP3 + DSP, plus
/// the UI overload the CPU (paging included) and starve thread mode. With
/// 0 and nothing playing there is no floor (the governor may go to Low).
pub static CLOCK_FLOOR: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// Governor on/off (test command `gov on|off`).
pub static GOV_ON: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Settings › Power › Power mode as the governor follows it (Automatic
/// under a host test, `ui::POWER_MODE`).
pub fn power_mode() -> governor::PowerMode {
    governor::PowerMode::from_setting(i16::from(
        crate::ui::POWER_MODE.load(core::sync::atomic::Ordering::Relaxed),
    ))
}

/// Raise the clock at once to the audio floor `floor` (0 Base … 4 Max)
/// unless the mode pins it (`governor::raise_for_floor`; Battery saver
/// stays at its minimum, and nothing changes with the governor off).
pub fn raise_to_floor(floor: u8) {
    if !GOV_ON.load(core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    if let Some(l) = governor::raise_for_floor(power_mode(), clock::current(), Level::from_floor(floor)) {
        clock::set_level(l);
    }
}

/// Clock governor (PWR-2, PWR-5): once a second, from the main executor's
/// idle share, the audio floor, the peripherals in use and the power mode
/// (`orpheus_power::governor`, host-tested).
struct Governor {
    last_idle: u32,
    last_t: u64,
    g: governor::Governor,
}

impl Governor {
    fn new() -> Self {
        Self { last_idle: executor::idle_us(), last_t: hal::time::now_us(), g: governor::Governor::new() }
    }

    fn step(&mut self) {
        use core::sync::atomic::Ordering::Relaxed;
        use governor::periph;
        let idle = executor::idle_us();
        let t = hal::time::now_us();
        let idle_pct = governor::idle_percent(u64::from(idle.wrapping_sub(self.last_idle)), t - self.last_t);
        self.last_idle = idle;
        self.last_t = t;
        let f = CLOCK_FLOOR.load(Relaxed);
        let playing = audio::PLAYING.load(Relaxed);
        let mut active = periph::ALWAYS;
        if hal::usb::vbus_present() {
            active |= periph::USB;
        }
        if drain::shared().screen_on.load(Relaxed) {
            active |= periph::DISPLAY;
        }
        if playing {
            active |= periph::AUDIO;
        }
        let input = governor::Input {
            idle_pct,
            current: clock::current(),
            floor: governor::floor_level(f, playing, audio::PAUSED.load(Relaxed)),
            active,
        };
        if let Some(l) = self.g.step(input, power_mode()) {
            clock::set_level(l);
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
/// boot-info word (path 1, slot index in the low byte; it may be a
/// superseded slot stage 2 started as a fallback, T-6), otherwise (started
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
        // Direct calls, no function pointer: no indirect branch in a root.
        if nor::read(base, h).is_ok()
            && let Ok(hdr) = if recorded.is_some() {
                SlotHeader::from_bytes_or_superseded(h)
            } else {
                SlotHeader::from_bytes(h)
            }
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
    // A test request from an earlier `dev ram` start survives warm reboots;
    // a start from flash is never a test.
    if persist::forget_stale_test() {
        log::line(b"fw: stale test request cleared");
    }
    // T-6: `boot crash-test` armed a failing start for this slot (stage 2
    // fallback test): reboot as a fault would, keeping the boot counter.
    // Before any task runs, so nothing (no NOR write) is cut short.
    if persist::take_crash_test() {
        log::line(b"fw: crash test: this start fails");
        wdt::reboot_after_fault();
    }
    // Also the battery logger's shared state (`drain::init`): main is
    // resident, its callees here are not.
    audio::init_chain();
    // The Books model and card side (statics both tasks use).
    books::init();
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
        s.spawn(bt::bt().unwrap());
        s.spawn(fm::fm().unwrap());
        s.spawn(sensor::sensor().unwrap());
    })
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    hal::panic(info)
}
