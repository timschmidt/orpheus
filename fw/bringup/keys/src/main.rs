//! `keys` — WP-INPUT-1 bring-up image.
//!
//! Shows the two ladder values, the power level, earphone sense and the ten
//! keys (held = filled, seen = green outline) for `arg0` seconds (default
//! 20), sampling every 10 ms through the debouncer. Records what it saw.
//!
//! Counters:
//!  0 keys ever seen (bit mask, orpheus_input::Key order)
//!  1 ladder 1 idle value at start      5 ladder 2 min
//!  2 ladder 2 idle value at start      6 edges seen
//!  3 ladder 1 min                      7 samples
//!  4 earphones seen (1/0)              8 power level at start

#![no_std]
#![no_main]

use atj2157_hal::{self as hal, dma, executor, log, pac, persist, wdt};
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Ticker, Timer};
use orpheus_proto::result::Verdict;
use static_cell::StaticCell;
use yoton_bsp::display::{Backlight, Display};
use yoton_bsp::keys::{self, Debouncer, Key, Keys};
use yoton_bsp::{display, tiny};

const BG: u16 = tiny::rgb(0, 0, 0);
const FG: u16 = tiny::rgb(255, 255, 255);
const DIM: u16 = tiny::rgb(80, 80, 80);
const SEEN: u16 = tiny::rgb(0, 200, 0);
const HELD: u16 = tiny::rgb(255, 200, 0);

fn draw(fb: &mut [u16], s: keys::Sample, held: Keys, seen: Keys, secs_left: u32) {
    tiny::fill_rect(fb, 0, 0, 128, 128, BG);
    let mut nb = [0u8; 10];
    tiny::text(fb, 2, 2, 1, FG, b"KEYS");
    let x = tiny::text(fb, 80, 2, 1, DIM, tiny::dec(secs_left, &mut nb));
    tiny::text(fb, x, 2, 1, DIM, b"S");
    // Bars for the ladder values (0..1023 over 120 px).
    for (row, v, label) in [(14usize, s.ladder1, b"L1"), (30, s.ladder2, b"L2")] {
        tiny::text(fb, 2, row, 1, FG, label);
        tiny::frame_rect(fb, 16, row, 110, 9, DIM);
        tiny::fill_rect(fb, 17, row + 1, usize::from(v) * 108 / 1023, 7, tiny::rgb(60, 120, 255));
        tiny::text(fb, 90, row + 1, 1, FG, tiny::dec(u32::from(v), &mut nb));
    }
    if keys::earphones_present(s) {
        tiny::text(fb, 2, 44, 1, SEEN, b"EARPHONES");
    }
    // Ten boxes, two rows of five.
    for (i, k) in Key::ALL.iter().enumerate() {
        let x = 2 + (i % 5) * 25;
        let y = 60 + (i / 5) * 32;
        let c = if held.contains(*k) {
            HELD
        } else if seen.contains(*k) {
            SEEN
        } else {
            DIM
        };
        if held.contains(*k) {
            tiny::fill_rect(fb, x, y, 23, 28, c);
        } else {
            tiny::frame_rect(fb, x, y, 23, 28, c);
        }
        let short: &[u8] = match k {
            Key::Power => b"PWR",
            Key::Back => b"BCK",
            Key::Menu => b"MNU",
            Key::Ok => b"OK",
            Key::Prev => b"PRV",
            Key::Next => b"NXT",
            Key::Up => b"UP",
            Key::Down => b"DN",
            Key::VolUp => b"V+",
            Key::VolDown => b"V-",
        };
        let tc = if held.contains(*k) { BG } else { FG };
        tiny::text(fb, x + 2, y + 10, 1, tc, short);
    }
}

#[embassy_executor::task]
async fn run(test_id: u32, secs: u32) {
    let mut r = persist::Results::begin(test_id);
    let fb = display::take_framebuffer().unwrap_or_else(|| wdt::reboot(wdt::Target::Adfu));
    // SAFETY: channel 1 is the display's.
    let mut disp = Display::new(unsafe { dma::Channel::steal(1) });
    let _bl = Backlight::new(12);
    keys::init();
    Timer::after_millis(20).await;
    let s0 = keys::sample();
    r.set(1, u32::from(s0.ladder1));
    r.set(2, u32::from(s0.ladder2));
    r.set(8, u32::from(s0.power));

    let mut deb = Debouncer::new();
    let mut seen = Keys::NONE;
    let (mut min1, mut min2) = (u16::MAX, u16::MAX);
    let (mut edges, mut samples, mut ear) = (0u32, 0u32, false);
    let start = Instant::now();
    let end = start + Duration::from_secs(u64::from(secs));
    let mut ticker = Ticker::every(Duration::from_millis(10));
    let mut frame = 0u32;
    while Instant::now() < end {
        ticker.next().await;
        let s = keys::sample();
        samples += 1;
        min1 = min1.min(s.ladder1);
        min2 = min2.min(s.ladder2);
        ear |= keys::earphones_present(s);
        let (ev, n) = deb.feed(keys::decode(s));
        for e in &ev[..n] {
            edges += 1;
            if e.down {
                seen = seen.with(e.key);
                let mut lb = log::LineBuf::new();
                lb.push_str("key ").push_str(e.key.name());
                lb.emit();
            }
        }
        frame += 1;
        if frame.is_multiple_of(5) {
            let left = end.saturating_duration_since(Instant::now()).as_secs() as u32;
            draw(fb.as_mut_slice(), s, deb.held(), seen, left);
            disp.flush_rows(fb.as_slice(), 0, 128).await;
        }
    }
    r.set(0, u32::from(seen.0));
    r.set(3, u32::from(min1));
    r.set(4, u32::from(ear));
    r.set(5, u32::from(min2));
    r.set(6, edges);
    r.set(7, samples);
    // Idle must decode to nothing (the user presses keys only during the
    // user check).
    let idle_ok = keys::decode(s0).is_empty() || s0.power;
    let mut msg = log::LineBuf::new();
    msg.push_str("idle l1=").push_u32(u32::from(s0.ladder1)).push_str(" l2=").push_u32(u32::from(s0.ladder2));
    msg.push_str(" seen=").push_hex(u32::from(seen.0));
    r.flush(if idle_ok { Verdict::Pass } else { Verdict::Fail }, msg.as_bytes());
    wdt::reboot(wdt::Target::Adfu);
}

static MAIN: StaticCell<executor::MainExecutor> = StaticCell::new();

#[unsafe(no_mangle)]
extern "C" fn main() -> ! {
    log::init();
    hal::init();
    wdt::start(pac::rtc::wd::PERIOD_5S6);
    wdt::feed_from_idle(true);
    let (test_id, secs) =
        persist::test_request().map_or((0, 20), |(id, m)| (id, if m.args[0] == 0 { 20 } else { m.args[0] }));
    MAIN.init(executor::MainExecutor::new()).run(|s: Spawner| {
        s.spawn(run(test_id, secs).unwrap());
    })
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    hal::panic(info)
}
