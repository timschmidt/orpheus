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
