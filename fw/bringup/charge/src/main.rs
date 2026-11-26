//! `charge` — WP-PWR-2 bring-up image.
//!
//! Runs the charger state machine once per second for `arg0` seconds
//! (default 30), showing state, battery and charger registers; stops the
//! charger before rebooting (Rockbox and ADFU do not run the state machine).
//!
//! Counters:
//!  0 final state (ChargeState as index)   5 CHG_DET at end
//!  1 battery raw at start                 6 cable present (1/0)
//!  2 battery raw at end                   7 current code at end
//!  3 max charge phase seen                8 DC5V ADC raw
//!  4 CHG_CTL while charging (last)        9 seconds charged

#![no_std]
#![no_main]

use atj2157_hal::{self as hal, dma, executor, log, pac, persist, wdt};
use embassy_executor::Spawner;
use embassy_time::{Duration, Ticker};
use orpheus_power::gauge;
use orpheus_proto::result::Verdict;
use static_cell::StaticCell;
use yoton_bsp::charger::{self, ChargeState, Charger};
use yoton_bsp::display::{self, Backlight, Display};
use yoton_bsp::tiny;

fn state_name(s: ChargeState) -> &'static [u8] {
    match s {
        ChargeState::NotPresent => b"NO CABLE",
        ChargeState::Detecting => b"DETECTING",
        ChargeState::NoBattery => b"NO BATTERY",
        ChargeState::Trickle => b"TRICKLE 50MA",
        ChargeState::Charging => b"CHARGING 200MA",
        ChargeState::Full => b"FULL",
        ChargeState::Fault => b"FAULT",
    }
}

fn state_index(s: ChargeState) -> u32 {
    match s {
        ChargeState::NotPresent => 0,
        ChargeState::Detecting => 1,
        ChargeState::NoBattery => 2,
        ChargeState::Trickle => 3,
        ChargeState::Charging => 4,
        ChargeState::Full => 5,
        ChargeState::Fault => 6,
    }
}

#[embassy_executor::task]
async fn run(test_id: u32, secs: u32) {
    let mut r = persist::Results::begin(test_id);
    let fb = display::take_framebuffer().unwrap_or_else(|| wdt::reboot(wdt::Target::Adfu));
    // SAFETY: channel 1 is the display's.
    let mut disp = Display::new(unsafe { dma::Channel::steal(1) });
    let _bl = Backlight::new(display::Duty::DEFAULT);
    charger::init();
    let mut c = Charger::new();
    let raw0 = charger::battery_raw();
    r.set(1, u32::from(raw0));
    let (mut max_phase, mut charged, mut ctl_chg) = (0u32, 0u32, 0u32);
    let mut ticker = Ticker::every(Duration::from_secs(1));
    for left in (0..secs).rev() {
        let i = charger::inputs();
        let cmd = c.step(i);
        charger::apply(&mut c, cmd).await;
        let (ctl, det) = charger::regs();
        let phase = pac::pmu::chg_det::CHGPHASE.get(det);
        max_phase = max_phase.max(phase);
        if matches!(c.state(), ChargeState::Charging | ChargeState::Trickle) {
            charged += 1;
            ctl_chg = ctl;
        }
        let mut b = [0u8; 10];
        let mut h = [0u8; 8];
        let f = fb.as_mut_slice();
        tiny::fill_rect(f, 0, 0, 128, 128, 0);
        tiny::text(f, 2, 2, 1, 0xFFFF, b"CHARGER");
        tiny::text(f, 90, 2, 1, tiny::rgb(90, 90, 90), tiny::dec(left, &mut b));
        tiny::text(f, 2, 16, 1, tiny::rgb(255, 200, 0), state_name(c.state()));
        let mv = gauge::raw_to_mv(i.battery_raw);
        let x = tiny::text(f, 2, 32, 2, 0xFFFF, tiny::dec(mv, &mut b));
        tiny::text(f, x, 32, 2, 0xFFFF, b"MV");
        let x = tiny::text(f, 2, 54, 1, 0xFFFF, b"RAW ");
        tiny::text(f, x, 54, 1, 0xFFFF, tiny::dec(u32::from(i.battery_raw), &mut b));
        let x = tiny::text(f, 2, 66, 1, 0xFFFF, b"PHASE ");
        let x = tiny::text(f, x, 66, 1, 0xFFFF, tiny::dec(phase, &mut b));
        let x = tiny::text(f, x, 66, 1, 0xFFFF, b" I ");
        tiny::text(f, x, 66, 1, 0xFFFF, tiny::dec(u32::from(i.current_step), &mut b));
        let x = tiny::text(f, 2, 82, 1, tiny::rgb(120, 120, 120), b"CTL ");
        tiny::text(f, x, 82, 1, tiny::rgb(120, 120, 120), tiny::hex(ctl, &mut h));
        let x = tiny::text(f, 2, 94, 1, tiny::rgb(120, 120, 120), b"DET ");
        tiny::text(f, x, 94, 1, tiny::rgb(120, 120, 120), tiny::hex(det, &mut h));
        disp.flush_rows(f, 0, 128).await;
        ticker.next().await;
    }
    let (ctl, det) = charger::regs();
    r.set(0, state_index(c.state()));
    r.set(2, u32::from(charger::battery_raw()));
    r.set(3, max_phase);
    r.set(4, ctl_chg);
    r.set(5, det);
    r.set(6, u32::from(charger::cable_present()));
    r.set(7, u32::from(c.current_code()));
    r.set(8, u32::from(charger::dc5v_raw()));
    r.set(9, charged);
    // Leave the charger off: nothing supervises it after this image.
    charger::stop_now();
    let ok = !charger::cable_present()
        || matches!(c.state(), ChargeState::Charging | ChargeState::Trickle | ChargeState::Full)
            && pac::pmu::chg_ctl::CHG_CURRENT.get(ctl_chg) <= 4;
    let mut msg = log::LineBuf::new();
    msg.push_str("state=").push_u32(state_index(c.state())).push_str(" bat=").push_u32(u32::from(raw0));
    msg.push_str("->").push_u32(u32::from(charger::battery_raw())).push_str(" ctl=").push_hex(ctl);
    r.flush(if ok { Verdict::Pass } else { Verdict::Fail }, msg.as_bytes());
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
        persist::test_request().map_or((0, 30), |(id, m)| (id, if m.args[0] == 0 { 30 } else { m.args[0] }));
    MAIN.init(executor::MainExecutor::new()).run(|s: Spawner| {
        s.spawn(run(test_id, secs).unwrap());
    })
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    hal::panic(info)
}
