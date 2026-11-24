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

#![no_std]
#![no_main]

mod audio;
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
use yoton_proto::layout::dual;
use yoton_proto::slot::SlotHeader;

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

#[embassy_executor::task]
async fn health() {
    Timer::after_secs(30).await;
    persist::clear_boot_counter();
    log::line(b"fw: healthy start");
}

/// Start the pager from the slot header in NOR (`0x762000`). Returns
/// whether paging is on. Resident (a root in resident.toml).
#[inline(never)]
fn start_pager() -> bool {
    nor::init();
    let mut h = [0u8; 128];
    if nor::read(dual::SLOT_A, &mut h).is_err() {
        return false;
    }
    match SlotHeader::from_bytes(&h) {
        Ok(hdr) if hdr.code_pages + hdr.rodata_pages > 0 => {
            pager::init(dual::SLOT_A + hdr.pages_off, hdr.code_pages, hdr.rodata_pages);
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
    pager::arm_deny();
    log::init();
    let paged = start_pager();
    hal::init();
    wdt::feed_from_idle(true);
    clock::set_level(Level::Base);
    log::line(if paged { b"fw: start (paged)" } else { b"fw: start (no pages)" });
    let rt = executor::start_rt();
    rt.spawn(audio::audio().unwrap());
    MAIN.init(executor::MainExecutor::new()).run(|s: Spawner| {
        s.spawn(input().unwrap());
        s.spawn(power().unwrap());
        s.spawn(ui::ui().unwrap());
        s.spawn(usbdev::usb().unwrap());
        s.spawn(health().unwrap());
    })
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    hal::panic(info)
}
