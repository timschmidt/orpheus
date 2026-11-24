//! `xtask dev ram`: load a resident-only image at 0x100000 through ymon and
//! start it; in test mode, collect the results after its reboot (07 §5.3).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use yoton_host::bot;
use yoton_proto::devchan::{ADFU_PID, ADFU_VID};
use yoton_proto::hash::fnv1a;
use yoton_proto::mailbox::{self, FLAG_RETURN_ADFU, Mailbox};
use yoton_proto::mem::{IMAGE_BASE, RAM_IMAGE_MAX};
use yoton_proto::{fault, result};

use super::{hil, to_ymon};
use crate::args::{Args, num};
use crate::util::log;

pub fn cmd(args: &mut Args) -> Result<String> {
    let test = args.value("test");
    let wait = args.value("wait").map(|s| num(&s)).transpose()?.unwrap_or(60);
    let debug = args.flag("debug");
    let no_wait = args.flag("no-wait");
    let mut targs = Vec::new();
    while let Some(a) = args.value("arg") {
        targs.push(num(&a)?);
    }
    let img = args.positional("IMAGE.bin or bring-up name")?;
    let (path, elf) = if img.ends_with(".bin") {
        (PathBuf::from(&img), None)
    } else {
        let b = crate::build::build_bringup(&img, !debug)?;
        (b.bin, Some(b.elf))
    };
    let blob = std::fs::read(&path)?;
    if blob.len() > RAM_IMAGE_MAX as usize {
        bail!(
            "{} is {} bytes; RAM images must end below ymon ({RAM_IMAGE_MAX} bytes)",
            path.display(),
            blob.len()
        );
    }
    let name = test.clone().unwrap_or_else(|| img.clone());
    if no_wait {
        // Arm the test mailbox (the image's safety window) but return at once.
        let mut d = super::to_ymon()?;
        d.write(result::BASE, &[0u8; result::SIZE])?;
        d.write(fault::BASE, &[0u8; fault::SIZE])?;
        let t = test.as_deref().unwrap_or("live");
        d.write(mailbox::BASE, &Mailbox::test(fnv1a(t.as_bytes()), FLAG_RETURN_ADFU, &targs).to_bytes())?;
        d.write_verify(IMAGE_BASE, &blob, 0x1000)?;
        d.jump(IMAGE_BASE)?;
        return Ok("started (test mailbox armed, not waiting)".into());
    }
    run_image(
        &blob,
        test.as_deref().map(|t| (t, targs.as_slice())),
        Duration::from_secs(u64::from(wait)),
        elf,
        &name,
    )
}

pub fn run_image(
    blob: &[u8],
    test: Option<(&str, &[u32])>,
    wait: Duration,
    elf: Option<PathBuf>,
    name: &str,
) -> Result<String> {
    let mut d = to_ymon()?;
    // Fresh persistent area: stale results must not pass a new run.
    d.write(result::BASE, &[0u8; result::SIZE])?;
    d.write(fault::BASE, &[0u8; fault::SIZE])?;
    let mb = match test {
        Some((t, a)) => Mailbox::test(fnv1a(t.as_bytes()), FLAG_RETURN_ADFU, a),
        None => Mailbox::default(),
    };
    d.write(mailbox::BASE, &mb.to_bytes())?;
    d.write_verify(IMAGE_BASE, blob, 0x1000)?;
    log(&format!("loaded {} bytes at {IMAGE_BASE:#x}; starting", blob.len()));
    d.jump(IMAGE_BASE)?;
    d.close();
    let Some((tname, _)) = test else {
        return Ok("started".into());
    };
    let t0 = Instant::now();
    if !bot::wait_gone(ADFU_VID, ADFU_PID, wait) {
        bail!("test `{tname}` did not reboot within {wait:?} (device still in ADFU/ymon: image hung?)");
    }
    log(&format!("device left the bus after {:.1} s; waiting for ADFU", t0.elapsed().as_secs_f32()));
    let mut d = to_ymon()?;
    log(&format!("back in ADFU after {:.1} s", t0.elapsed().as_secs_f32()));
    hil::collect(&mut d, name, elf.as_deref())
}
