//! `xtask dev usbtest [--cycles N]`: WP-USB-1 acceptance on the player.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use orpheus_host::devchan::DevChannel;
use orpheus_host::nusb;
use orpheus_host::{bot, image};
use orpheus_proto::devchan::{ADFU_PID, ADFU_VID, DEV_PID, DEV_VID, REBOOT_ADFU};

use crate::args::{Args, num};
use crate::util::{log, out_dir};

pub fn cmd(args: &mut Args) -> Result<String> {
    let cycles = args.value("cycles").map(|s| num(&s)).transpose()?.unwrap_or(20);
    let b = crate::build::build_bringup("usb", true)?;
    let blob = std::fs::read(&b.bin)?;
    super::ram::run_image(&blob, None, Duration::from_secs(5), None, "usb")?;
    let t0 = Instant::now();
    let d = DevChannel::open(Duration::from_secs(15))?;
    log(&format!("enumerated after {:.2} s, speed {:?}", t0.elapsed().as_secs_f32(), d.speed()));
    let info = d.info()?;
    log(&format!(
        "INFO: version {} uptime {} ms flags {:#x} setups {}",
        info.version, info.uptime_ms, info.flags, info.reserved
    ));
    if !matches!(d.speed(), Some(nusb::Speed::High)) {
        bail!("not high speed: {:?}", d.speed());
    }
    // Control IN transfers of every size class against the loaded image.
    let mut fails = 0;
    for &n in &[
        1usize, 2, 7, 8, 63, 64, 65, 127, 128, 129, 255, 256, 511, 512, 513, 1000, 1023, 1024, 2048, 4095,
        4096,
    ] {
        let got = d.control_in(orpheus_proto::devchan::REQ_PEEK, 0x0000, 0x0010, n as u16)?;
        if got.len() != n || got[..] != blob[..n] {
            log(&format!("  PEEK {n} bytes: MISMATCH (got {})", got.len()));
            fails += 1;
        }
    }
    log(&format!("control IN size classes: {} mismatches", fails));
    // Bulk read speed of the channel: the whole image region by 4 KiB.
    let t = Instant::now();
    let img = d.peek(0x0010_0000, blob.len())?;
    let dt = t.elapsed().as_secs_f64();
    log(&format!("PEEK {} bytes in {:.3} s ({:.0} KiB/s)", img.len(), dt, img.len() as f64 / 1024.0 / dt));
    if img != blob {
        fails += 1;
        log("  image read-back mismatch");
    }
    let fb = d.frame()?;
    image::write_rgb565_png(&fb, 128, 128, &out_dir().join("usb-frame.png"))?;
    log(&format!("stats: {:?}", d.stats(0)?));
    // Re-enumeration cycles via soft detach.
    let mut ok_cycles = 0;
    let mut d = d;
    for i in 0..cycles {
        let old = d.info.device_address();
        d.control_out(orpheus_proto::devchan::REQ_REBOOT, 0x10, 0, &[])?;
        drop(d);
        // Re-enumeration gives the device a new bus address.
        let t = Instant::now();
        loop {
            match bot::find(DEV_VID, DEV_PID) {
                Some(n) if n.device_address() != old => break,
                _ if t.elapsed() > Duration::from_secs(5) => bail!("cycle {i}: no re-enumeration"),
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        d = DevChannel::open(Duration::from_secs(5))?;
        if matches!(d.speed(), Some(nusb::Speed::High)) && d.info().is_ok() {
            ok_cycles += 1;
        } else {
            log(&format!("cycle {i}: speed {:?}", d.speed()));
        }
    }
    log(&format!("re-enumeration: {ok_cycles}/{cycles} at high speed"));
    d.reboot(REBOOT_ADFU);
    if bot::wait_for(ADFU_VID, ADFU_PID, Duration::from_secs(20)).is_none() {
        bail!("device did not return to ADFU");
    }
    if fails == 0 && ok_cycles == cycles {
        Ok(format!("PASS: high speed, all control sizes correct, {ok_cycles}/{cycles} re-enumerations"))
    } else {
        bail!("{fails} failures, {ok_cycles}/{cycles} re-enumerations")
    }
}

/// How long the player may take to answer again after a provoked failure
/// (a recovery: about 3 s to notice, re-enumeration).
const ANSWER_AGAIN: Duration = Duration::from_secs(20);

/// `xtask dev usb-cancel [--rounds N]` (USB-W1): provoke the soak's USB
/// failure on purpose against the running firmware and check that the
/// player answers again by itself. Every round:
///
/// 1. for each stage (`setup`, `data`, `status`): `usb delay <stage>
///    2500` makes the firmware hold the next request (`status`, TEST OUT)
///    2.5 s at that stage, as a late USB task would; the host gives up
///    after 2 s ("Cancelled") and at once sends a PEEK of the fault record
///    — the incident's sequence (STATUS 2026-11-24, bug USB-W1);
/// 2. 20 PEEKs of 1 KiB and 20 TEST OUTs with a 1 ms timeout, back to back:
///    the host cancels them mid-transfer;
///
/// after each, INFO must answer within [`ANSWER_AGAIN`]. Prints the USB
/// counters before and after. Never powers the player off; it keeps
/// running (no reboot needed).
pub fn cancel_cmd(args: &mut Args) -> Result<String> {
    use orpheus_host::transport::{Control, Transport, block_on};
    use orpheus_proto::devchan::{REQ_PEEK, REQ_TEST};
    use orpheus_proto::fault;
    let rounds = args.value("rounds").map(|s| num(&s)).transpose()?.unwrap_or(3);
    let mut d = DevChannel::open(Duration::from_secs(5))?;
    let first = usb_counters(&d)?;
    log(&format!("before: {}", super::soak::usb_line(&first)));
    let mut worst = Duration::ZERO;
    let peek = |d: &DevChannel| {
        d.control_in(REQ_PEEK, fault::BASE as u16, (fault::BASE >> 16) as u16, 64).map(|b| b.len())
    };
    for round in 1..=rounds {
        for stage in ["setup", "data", "status"] {
            d.control_out(REQ_TEST, 0, 0, format!("usb delay {stage} 2500").as_bytes())?;
            let held = d.control_out(REQ_TEST, 0, 0, b"status");
            let p = peek(&d);
            let (nd, dt) = answer_again(d)?;
            d = nd;
            worst = worst.max(dt);
            log(&format!(
                "round {round} {stage:>6}: held request {}, PEEK right after {}, INFO again after {:.1} s",
                held.as_ref().map_or_else(|e| format!("failed ({e:#})"), |()| "ok".into()),
                p.as_ref().map_or_else(|e| format!("failed ({e:#})"), |n| format!("{n} B")),
                dt.as_secs_f32()
            ));
        }
        for i in 0..20u16 {
            let a = 0x0010_0000u32 + u32::from(i) * 1024;
            let c = Control { request: REQ_PEEK, value: a as u16, index: (a >> 16) as u16 };
            let _ = block_on(d.link.t.control_in(c, 1024, 1));
            let c = Control { request: REQ_TEST, value: 0, index: 0 };
            let _ = block_on(d.link.t.control_out(c, b"status", 1));
        }
        let (nd, dt) = answer_again(d)?;
        d = nd;
        worst = worst.max(dt);
        log(&format!(
            "round {round}   cuts: 40 requests cut after 1 ms, INFO again after {:.1} s",
            dt.as_secs_f32()
        ));
    }
    let last = usb_counters(&d)?;
    log(&format!("after:  {}", super::soak::usb_line(&last)));
    Ok(format!(
        "PASS: answered after every provoked failure (worst {:.1} s); {} recoveries, {} transfers given up",
        worst.as_secs_f32(),
        last.reinits.saturating_sub(first.reinits),
        last.aborts.saturating_sub(first.aborts)
    ))
}

fn usb_counters(d: &DevChannel) -> Result<super::soak::UsbCounters> {
    orpheus_proto::devchan::usb::Counters::from_words(&d.stats(0)?)
        .ok_or_else(|| anyhow::anyhow!("the firmware has no USB counters (older than USB-W1)"))
}

/// Wait until INFO answers, opening the channel again as needed (a
/// recovery re-enumerates the player).
fn answer_again(d: DevChannel) -> Result<(DevChannel, Duration)> {
    let t = Instant::now();
    let mut d = d;
    loop {
        if d.info().is_ok() {
            return Ok((d, t.elapsed()));
        }
        if t.elapsed() > ANSWER_AGAIN {
            bail!(
                "the player stopped answering USB for {} s (USB-W1 not fixed): wait two minutes for the \
                 watchdog, then stop and ask the user (CLAUDE.md rule 7)",
                ANSWER_AGAIN.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(500));
        if let Ok(n) = DevChannel::open(Duration::from_secs(2)) {
            d = n;
        }
    }
}
