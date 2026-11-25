//! `xtask dev usbimage-test`: USB-6 acceptance on the player (06 §3.4).
//!
//! For a disk image and an ISO (generated, or `--img FILE` / `--iso
//! FILE`): upload it into `/.orpheus/images/` (CLAUDE.md rule 5), present
//! it with `usb image <path> disk|cd ro`, wait for the block device the
//! kernel makes of it, read the whole device, compare its SHA-256 with the
//! file's (a disk drops a partial last sector; a CD pads to 2048 bytes
//! with zeros), check the firmware's counters, then detach (`usb dev`) and
//! wait for the device to go.
//!
//! `--rw`: also present the disk image writable (test images in test mode
//! only), write a pattern over its first 64 KiB through the block device,
//! detach, present it read-only again and check the whole image reads back
//! with the pattern; the rest of the file unchanged.
//!
//! `--qemu`: boot the ISO from the block device in QEMU (`--iso` should be
//! a bootable image; `--banner TEXT` is what its serial console must
//! print within `--boot-secs`). Without `qemu-system-x86_64` it is
//! reported as not run.
//!
//! Needs the udev rule (`tools/udev/99-orpheus.rules`: the player's block
//! devices readable and writable by the logged-in user).

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use orpheus_host::devchan::DevChannel;
use orpheus_proto::devchan::REQ_TEST;
use sha2::{Digest, Sha256};

use crate::args::{Args, num};
use crate::util::{log, out_dir};

/// INQUIRY vendor of an image export (`orpheus_usb::msc::image::Export::unit`).
const VENDOR: &str = "Orpheus";
const MODEL: &str = "USB Drive";
const DIR: &str = "/.orpheus/images";

/// A deterministic test pattern (no two 512-byte sectors alike).
fn pattern(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2_654_435_761) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Send a firmware test command and poll its reply until it starts with
/// one of `want` (or `timeout`).
fn test_cmd(d: &DevChannel, text: &str, want: &[&str], timeout: Duration) -> Result<String> {
    d.control_out(REQ_TEST, 0, 0, text.as_bytes())?;
    let t0 = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let r = String::from_utf8_lossy(&d.control_in(REQ_TEST, 0, 0, 1024)?).into_owned();
        if want.iter().any(|w| r.starts_with(w)) {
            return Ok(r);
        }
        if t0.elapsed() > timeout {
            bail!("`{text}`: no `{want:?}` reply within {timeout:?} (last `{r}`)");
        }
    }
}

/// The kernel's block device for the player's image export, if present
/// (`/dev/sdX` for a disk, `/dev/srN` for a CD).
fn find_block() -> Option<(PathBuf, u64)> {
    let rd = std::fs::read_dir("/sys/block").ok()?;
    for e in rd.flatten() {
        let p = e.path();
        let rd = |f: &str| std::fs::read_to_string(p.join(f)).unwrap_or_default();
        if rd("device/vendor").trim() == VENDOR && rd("device/model").trim().starts_with(MODEL) {
            let sectors: u64 = rd("size").trim().parse().unwrap_or(0);
            return Some((Path::new("/dev").join(e.file_name()), sectors * 512));
        }
    }
    None
}

fn wait_block(timeout: Duration) -> Result<(PathBuf, u64)> {
    let t0 = Instant::now();
    loop {
        // A CD reports its size once the medium is read (after the unit
        // attention): wait for a non-zero size.
        if let Some((dev, size)) = find_block()
            && size > 0
            && dev.exists()
        {
            return Ok((dev, size));
        }
        if t0.elapsed() > timeout {
            bail!("no `{VENDOR} {MODEL}` block device within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn wait_gone(timeout: Duration) -> Result<()> {
    let t0 = Instant::now();
    while find_block().is_some() {
        if t0.elapsed() > timeout {
            bail!("the image's block device is still there after {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

fn read_device(dev: &Path) -> Result<Vec<u8>> {
    let mut f = std::fs::File::open(dev).with_context(|| {
        format!("open {} (udev rule installed? tools/udev/99-orpheus.rules)", dev.display())
    })?;
    let mut out = Vec::new();
    f.read_to_end(&mut out)?;
    Ok(out)
}

/// What the computer must see for `file` presented as `cd` or a disk.
fn expected(file: &[u8], cd: bool) -> Vec<u8> {
    let mut v = file.to_vec();
    if cd {
        v.resize(file.len().next_multiple_of(2048), 0);
    } else {
        v.truncate(file.len() / 512 * 512);
    }
    v
}

/// A command that switches the personality: the player detaches and
/// enumerates again, so it gets no reply; the channel is opened afresh.
fn switch(text: &str) -> Result<()> {
    let d = DevChannel::open(Duration::from_secs(10))?;
    d.control_out(REQ_TEST, 0, 0, text.as_bytes())?;
    drop(d);
    // Let the soft detach happen before anyone opens the channel again.
    std::thread::sleep(Duration::from_millis(1500));
    Ok(())
}

/// `usb image status` on a freshly opened channel.
fn status() -> Result<String> {
    let d = DevChannel::open(Duration::from_secs(10))?;
    test_cmd(&d, "usb image status", &["image "], Duration::from_secs(3))
}

/// Back to the developer personality; the block device goes, the card
/// returns to the player.
fn detach() -> Result<()> {
    switch("usb dev")?;
    wait_gone(Duration::from_secs(20))
}

struct Case {
    name: String,
    data: Vec<u8>,
    cd: bool,
}

/// Present `remote`, read the device, compare; detach.
fn present_and_compare(remote: &str, c: &Case, mode: &str) -> Result<String> {
    let medium = if c.cd { "cd" } else { "disk" };
    if find_block().is_some() {
        bail!("an image block device is present before the test: detach it first (`usb dev`)");
    }
    switch(&format!("usb image {remote} {medium} {mode}"))?;
    let (dev, size) = match wait_block(Duration::from_secs(30)) {
        Ok(x) => x,
        Err(e) => {
            let st = status().unwrap_or_else(|e| format!("no status: {e:#}"));
            let _ = detach();
            bail!("{e:#}; player: {st}");
        }
    };
    log(&format!("   {}: {} bytes ({})", dev.display(), size, c.name));
    let t0 = Instant::now();
    let got = read_device(&dev);
    let secs = t0.elapsed().as_secs_f64();
    let st = status().unwrap_or_else(|e| format!("no status: {e:#}"));
    log(&format!("   player: {st}"));
    detach()?;
    let got = got?;
    let want = expected(&c.data, c.cd);
    let (hg, hw) = (sha(&got), sha(&want));
    log(&format!(
        "   read {} bytes in {secs:.2} s ({:.2} MB/s), sha256 {hg}",
        got.len(),
        got.len() as f64 / secs / 1e6
    ));
    if got.len() != want.len() || hg != hw {
        bail!("{}: device {} bytes sha256 {hg}, file {} bytes sha256 {hw}", c.name, got.len(), want.len());
    }
    Ok(format!("{}: {} bytes, sha256 equal, {secs:.2} s", c.name, got.len()))
}

/// `--rw`: write a pattern over the first 64 KiB of the writable test
/// image, then check it read-only.
fn rw_case(remote: &str, c: &Case) -> Result<String> {
    let n = (64 * 1024).min(c.data.len() / 512 * 512);
    switch(&format!("usb image {remote} disk rw"))?;
    let (dev, _) = match wait_block(Duration::from_secs(30)) {
        Ok(x) => x,
        Err(e) => {
            let st = status().unwrap_or_else(|e| format!("no status: {e:#}"));
            let _ = detach();
            bail!("{e:#}; player: {st}");
        }
    };
    let patch = pattern(n, 0xC0FFEE);
    let wrote = (|| -> Result<u64> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .open(&dev)
            .with_context(|| format!("open {} for writing", dev.display()))?;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&patch)?;
        f.sync_all()?;
        Ok(f.seek(SeekFrom::End(0))?)
    })();
    let st = status().unwrap_or_else(|e| format!("no status: {e:#}"));
    log(&format!("   player: {st}"));
    detach()?;
    log(&format!("   wrote {n} bytes at 0 of {}", wrote?));
    let mut data = c.data.clone();
    data[..n].copy_from_slice(&patch);
    let after = Case { name: format!("{} after rw", c.name), data, cd: false };
    present_and_compare(remote, &after, "ro")
}

/// `--qemu`: boot the presented ISO from its block device.
fn qemu_case(remote: &str, banner: &str, secs: u64) -> Result<String> {
    if which("qemu-system-x86_64").is_none() {
        return Ok("qemu: not run (qemu-system-x86_64 not found)".into());
    }
    switch(&format!("usb image {remote} cd ro"))?;
    let (dev, _) = match wait_block(Duration::from_secs(30)) {
        Ok(x) => x,
        Err(e) => {
            let _ = detach();
            return Err(e);
        }
    };
    let log_path = out_dir().join("usbimage-qemu.log");
    let _ = std::fs::remove_file(&log_path);
    let child = std::process::Command::new("qemu-system-x86_64")
        .args(["-m", "256", "-display", "none", "-no-reboot", "-boot", "d", "-serial"])
        .arg(format!("file:{}", log_path.display()))
        .arg("-drive")
        .arg(format!("file={},media=cdrom,readonly=on,format=raw", dev.display()))
        .spawn();
    let mut seen = false;
    let t0 = Instant::now();
    if let Ok(mut child) = child {
        while t0.elapsed() < Duration::from_secs(secs) {
            std::thread::sleep(Duration::from_millis(500));
            if std::fs::read_to_string(&log_path).unwrap_or_default().contains(banner) {
                seen = true;
                break;
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    detach()?;
    if !seen {
        bail!("qemu: `{banner}` not on the serial console within {secs} s (log {})", log_path.display());
    }
    Ok(format!("qemu: booted, `{banner}` seen after {:.1} s", t0.elapsed().as_secs_f32()))
}

fn which(prog: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?.to_str()?.split(':').map(|d| Path::new(d).join(prog)).find(|p| p.exists())
}

pub fn cmd(args: &mut Args) -> Result<String> {
    let img = args.value("img");
    let iso = args.value("iso");
    let rw = args.flag("rw");
    let qemu = args.flag("qemu");
    let banner = args.value("banner").unwrap_or_else(|| "Linux".into());
    let boot_secs = args.value("boot-secs").map(|s| num(&s)).transpose()?.unwrap_or(120);
    let load = |p: Option<String>, name: &str, len: usize, seed: u32| -> Result<(String, Vec<u8>)> {
        match p {
            Some(p) => {
                let n = Path::new(&p).file_name().and_then(|n| n.to_str()).unwrap_or(name).to_string();
                Ok((n, std::fs::read(&p).with_context(|| format!("read {p}"))?))
            }
            None => Ok((name.to_string(), pattern(len, seed))),
        }
    };
    // Generated: 256 KiB + 300 bytes (a partial last sector) and an ISO of
    // 300 KiB + 100 bytes (padded to whole 2048-byte blocks).
    let (img_name, img_data) = load(img, "usb6-test.img", 256 * 1024 + 300, 1)?;
    let (iso_name, iso_data) = load(iso, "usb6-test.iso", 300 * 1024 + 100, 2)?;
    let cases = [
        Case { name: img_name, data: img_data, cd: false },
        Case { name: iso_name, data: iso_data, cd: true },
    ];
    let mut report = Vec::new();
    for c in &cases {
        let remote = format!("{DIR}/{}", c.name);
        log(&format!("== put {} ({} bytes)", remote, c.data.len()));
        log(&format!("   {}", super::put(&c.data, &remote)?));
        log(&format!("== present {} as {}", remote, if c.cd { "cd" } else { "disk" }));
        report.push(present_and_compare(&remote, c, "ro")?);
        log(&format!("   ok: {}", report.last().map_or("", String::as_str)));
    }
    if rw {
        let c = &cases[0];
        let remote = format!("{DIR}/{}", c.name);
        // Fresh copy: the read-back check compares with the original.
        super::put(&c.data, &remote)?;
        log(&format!("== present {remote} writable"));
        report.push(rw_case(&remote, c)?);
        log(&format!("   ok: {}", report.last().map_or("", String::as_str)));
    }
    if qemu {
        let c = &cases[1];
        log("== boot the ISO in QEMU");
        report.push(qemu_case(&format!("{DIR}/{}", c.name), &banner, u64::from(boot_secs))?);
        log(&format!("   {}", report.last().map_or("", String::as_str)));
    }
    Ok(report.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_media() {
        let f = pattern(5000, 3);
        assert_eq!(expected(&f, true).len(), 6144);
        assert!(expected(&f, true)[5000..].iter().all(|&b| b == 0));
        assert_eq!(expected(&f, false).len(), 4608);
        assert_ne!(pattern(512, 1)[..], pattern(1024, 1)[512..]);
    }
}
