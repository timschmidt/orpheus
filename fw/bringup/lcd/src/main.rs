//! `lcd` — WP-LCD-1 bring-up image.
//!
//! Panel init, PIO fill, DMA blit of a test pattern from the frame buffer,
//! panel id and GRAM read-back, backlight on PWM4.
//!
//! Counters:
//!  0 panel id bytes (dummy, id1, id2, id3 packed LE)
//!  1 PIO full-screen fill µs          5 read-back pixels compared
//!  2 DMA full-screen blit µs          6 read-back pixels matching
//!  3 DMA blit ok (1/0)                7 first read-back bytes (packed)
//!  4 async DMA blit µs                8 LCM_CTL after init

#![no_std]
#![no_main]

use atj2157_hal::{self as hal, dma, executor, log, pac, persist, time, wdt};
use embassy_executor::Spawner;
use embassy_time::Timer;
use static_cell::StaticCell;
use yoton_bsp::display::{self, Backlight, Display};
use yoton_proto::result::Verdict;

fn rgb(r: u8, g: u8, b: u8) -> u16 {
    (u16::from(r >> 3) << 11) | (u16::from(g >> 2) << 5) | u16::from(b >> 3)
}

/// Colour bars on top, a grey ramp, then an RGB gradient; a white frame.
fn pattern(fb: &mut [u16]) {
    const BARS: [(u8, u8, u8); 8] = [
        (255, 255, 255),
        (255, 255, 0),
        (0, 255, 255),
        (0, 255, 0),
        (255, 0, 255),
        (255, 0, 0),
        (0, 0, 255),
        (0, 0, 0),
    ];
    for y in 0..128usize {
        for x in 0..128usize {
            let c = if y < 48 {
                let (r, g, b) = BARS[x / 16];
                rgb(r, g, b)
            } else if y < 64 {
                let v = (x * 2) as u8;
                rgb(v, v, v)
            } else {
                rgb((x * 2) as u8, ((y - 64) * 4) as u8, (255 - x * 2) as u8)
            };
            fb[y * 128 + x] = c;
        }
    }
    for i in 0..128 {
        fb[i] = 0xFFFF;
        fb[127 * 128 + i] = 0xFFFF;
        fb[i * 128] = 0xFFFF;
        fb[i * 128 + 127] = 0xFFFF;
    }
}

#[embassy_executor::task]
async fn run(test_id: u32) {
    let mut r = persist::Results::begin(test_id);
    let fb = display::take_framebuffer().unwrap_or_else(|| wdt::reboot(wdt::Target::Adfu));
    // SAFETY: channel 1 is the display's (01-hardware §6); nothing else
    // in this image uses it.
    let mut disp = Display::new(unsafe { dma::Channel::steal(1) });
    r.set(8, disp.lcm_ctl());
    let id = disp.read_id();
    r.set(0, u32::from_le_bytes(id));
    let bl = Backlight::new(12);

    let t0 = time::now_us();
    disp.fill_pio(0, 0, 128, 128, rgb(0, 0, 128));
    r.set(1, (time::now_us() - t0) as u32);

    pattern(fb.as_mut_slice());
    let t0 = time::now_us();
    let ok = disp.flush_blocking(fb.as_slice());
    r.set(2, (time::now_us() - t0) as u32);
    r.set(3, u32::from(ok));

    let t0 = time::now_us();
    disp.flush_rows(fb.as_slice(), 0, 128).await;
    r.set(4, (time::now_us() - t0) as u32);

    // Read back row 10 (inside the colour bars) and row 100 (gradient).
    let mut compared = 0;
    let mut matching = 0;
    for &y in &[10u32, 100] {
        let mut buf = [0u8; 1 + 3 * 32];
        disp.read_pixels(0, y, &mut buf);
        if y == 10 {
            r.set(7, u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]));
        }
        for x in 0..32usize {
            let p = fb[y as usize * 128 + x];
            let (er, eg, eb) = ((p >> 11) as u8, ((p >> 5) & 63) as u8, (p & 31) as u8);
            let (gr, gg, gb) = (buf[1 + 3 * x] >> 3, buf[2 + 3 * x] >> 2, buf[3 + 3 * x] >> 3);
            compared += 1;
            if (er, eg, eb) == (gr, gg, gb) {
                matching += 1;
            }
        }
    }
    r.set(5, compared);
    r.set(6, matching);
    // The read cycle may have disturbed the panel state; redraw.
    disp.flush_rows(fb.as_slice(), 0, 128).await;
    let _ = &bl;

    let mut msg = log::LineBuf::new();
    msg.push_str("id=").push_hex(u32::from_le_bytes(id)).push_str(" dma_ok=").push_u32(u32::from(ok));
    msg.push_str(" match=").push_u32(matching).push_str("/").push_u32(compared);
    msg.emit();
    r.flush(if ok { Verdict::Pass } else { Verdict::Fail }, msg.as_bytes());
    Timer::after_millis(200).await;
    wdt::reboot(wdt::Target::Adfu);
}

static MAIN: StaticCell<executor::MainExecutor> = StaticCell::new();

#[unsafe(no_mangle)]
extern "C" fn main() -> ! {
    log::init();
    hal::init();
    wdt::start(pac::rtc::wd::PERIOD_5S6);
    wdt::feed_from_idle(true);
    let test_id = persist::test_request().map_or(0, |(id, _)| id);
    MAIN.init(executor::MainExecutor::new()).run(|s: Spawner| {
        s.spawn(run(test_id).unwrap());
    })
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    hal::panic(info)
}
