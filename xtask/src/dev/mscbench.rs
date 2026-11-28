//! `cargo xtask dev msc-bench` (PERF-SD, 06 §2.4): the card reader's
//! throughput and integrity on the player, through files only.
//!
//! ```text
//! cargo xtask dev msc-bench [--mib N] [--file PATH] [--check] [--keep-dev]
//! ```
//!
//! Switches the player to the card reader (`stop`, `usb msc`), waits for
//! the kernel's block device (INQUIRY `orpheus` / `YM03 microSD`), mounts
//! its first partition with `udisksctl` (no root), then:
//!
//! 1. reads an existing file (`--file`, a path on the card; default the
//!    demo set's 19 MB FLAC) with direct I/O: the read rate;
//! 2. writes `N` MiB (default 64) of a deterministic pseudo-random
//!    pattern to `/.orpheus/test/perf/perf.bin` with direct I/O and an
//!    `fsync`: the write rate (CLAUDE.md rule 5: only inside `/.orpheus/`,
//!    a file this test creates; never the block device);
//! 3. reads it back with direct I/O and compares every byte with the
//!    pattern: the read-back rate and the integrity verdict;
//! 4. deletes the file and the folder, unmounts, switches back (`usb
//!    dev`) and checks the player answers `status` again.
//!
//! Rates are MB/s = 10^6 B/s (as `dd` prints them). `--check` compares
//! them with `budgets.toml [msc]` and fails below a floor (or on any
//! integrity error). The firmware's pager counters are read before and
//! after each phase (line loads per MiB moved: the code-cache cost of the
//! pipeline). `prof --during "msc-bench …" --only write` captures one
//! phase (the phases call `prof::hook`).

use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use orpheus_host::devchan::DevChannel;
use orpheus_proto::devchan::REQ_TEST;

use super::prof::{self, Phase};
use crate::args::{Args, num};
use crate::util::{log, root};

/// INQUIRY of the card reader (`fw/firmware/src/usbdev.rs` `run_msc`).
const VENDOR: &str = "orpheus";
const MODEL: &str = "YM03 microSD";
/// The demo set's largest single file (19,963,388 B).
const DEFAULT_FILE: &str = "Music/Bach/Goldberg Variations/05 Variatio 4 (complete, 96 kHz 24 bit).flac";
const TEST_DIR: &str = ".orpheus/test/perf";
/// Transfer size of each direct read or write (`dd bs=1M`).
const CHUNK: usize = 1 << 20;

/// A buffer for direct I/O: 4 KiB aligned.
struct Aligned {
    raw: Vec<u8>,
    off: usize,
}

impl Aligned {
    fn new(len: usize) -> Self {
        let raw = vec![0u8; len + 4096];
        let off = raw.as_ptr().align_offset(4096);
        Self { raw, off }
    }
    fn buf(&mut self) -> &mut [u8] {
        let end = self.raw.len() - 4096 + self.off;
        &mut self.raw[self.off..end]
    }
}

/// The test pattern: MiB `k` of the file (xorshift64*, seeded per MiB, so
/// any MiB is made without the ones before it).
fn fill(buf: &mut [u8], k: u64) {
    let mut x = (k + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    for w in buf.chunks_exact_mut(8) {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        w.copy_from_slice(&x.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
    }
}

fn mbps(bytes: u64, t: Duration) -> f64 {
    bytes as f64 / t.as_secs_f64().max(1e-9) / 1e6
}

/// The card reader's block device (`/dev/sdX`), if the kernel has it.
fn find_block() -> Option<String> {
    let rd = std::fs::read_dir("/sys/block").ok()?;
    for e in rd.flatten() {
        let p = e.path();
        let rd = |f: &str| std::fs::read_to_string(p.join(f)).unwrap_or_default();
        if rd("device/vendor").trim() == VENDOR
            && rd("device/model").trim().starts_with(MODEL)
            && rd("size").trim().parse::<u64>().unwrap_or(0) > 0
        {
            return Some(e.file_name().to_string_lossy().into_owned());
        }
    }
    None
}

fn wait_for<T>(what: &str, timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Result<T> {
    let t0 = Instant::now();
    loop {
        if let Some(v) = f() {
            return Ok(v);
        }
        if t0.elapsed() > timeout {
            bail!("{what}: not within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// A personality switch: the player re-enumerates, often before the
/// request's status stage, so a transfer error is only a note.
fn switch(text: &str) -> Result<()> {
    let d = DevChannel::open(Duration::from_secs(10))?;
    if let Err(e) = d.control_out(REQ_TEST, 0, 0, text.as_bytes()) {
        log(&format!("   ({text}: {e}; checking the result instead)"));
    }
    drop(d);
    std::thread::sleep(Duration::from_millis(1500));
    Ok(())
}

fn test_cmd(text: &str) -> Result<String> {
    let d = DevChannel::open(Duration::from_secs(10))?;
    d.control_out(REQ_TEST, 0, 0, text.as_bytes())?;
    std::thread::sleep(Duration::from_millis(400));
    Ok(String::from_utf8_lossy(&d.control_in(REQ_TEST, 0, 0, 1024)?).into_owned())
}

/// Pager line loads and uptime (ms) from STATS group 0, if the channel
/// answers (it does in the card reader: the developer channel stays).
fn counters() -> Option<(u32, u32)> {
    let d = DevChannel::open(Duration::from_secs(3)).ok()?;
    let w = d.stats(0).ok()?;
    Some((*w.get(4)?, *w.get(8)?))
}

/// Mount `part` (the desktop may already have): its mount point.
fn mount(part: &str) -> Result<PathBuf> {
    let dev = format!("/dev/{part}");
    let find = || -> Option<PathBuf> {
        let o = Command::new("findmnt").args(["-nro", "TARGET", &dev]).output().ok()?;
        let s = String::from_utf8_lossy(&o.stdout).lines().next()?.trim().to_string();
        (!s.is_empty()).then(|| PathBuf::from(s.replace("\\x20", " ")))
    };
    // A desktop automount may be on its way.
    std::thread::sleep(Duration::from_secs(2));
    if let Some(m) = find() {
        return Ok(m);
    }
    let o = Command::new("udisksctl")
        .args(["mount", "--no-user-interaction", "-b", &dev])
        .output()
        .context("udisksctl")?;
    if let Some(m) = find() {
        return Ok(m);
    }
    bail!("could not mount {dev}: {}", String::from_utf8_lossy(&o.stderr).trim())
}

fn unmount(part: &str) {
    let _ = Command::new("sync").status();
    let _ = Command::new("udisksctl")
        .args(["unmount", "--no-user-interaction", "-b", &format!("/dev/{part}")])
        .output();
}

const O_DIRECT: i32 = libc::O_DIRECT;

/// Read `path` with direct I/O in [`CHUNK`] pieces; `check(k, piece)` per
/// MiB on a thread of its own (the next read is not held up by the check:
/// the player would wait for the host). Bytes and time.
fn read_direct(path: &Path, mut check: impl FnMut(u64, &[u8]) + Send) -> Result<(u64, Duration)> {
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_DIRECT)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    let (full_tx, full_rx) = std::sync::mpsc::sync_channel::<(u64, Aligned, usize)>(2);
    let (free_tx, free_rx) = std::sync::mpsc::sync_channel::<Aligned>(3);
    for _ in 0..3 {
        let _ = free_tx.send(Aligned::new(CHUNK));
    }
    std::thread::scope(|sc| -> Result<(u64, Duration)> {
        sc.spawn(move || {
            while let Ok((k, mut a, n)) = full_rx.recv() {
                check(k, &a.buf()[..n]);
                if free_tx.send(a).is_err() {
                    break;
                }
            }
        });
        let (mut total, mut k) = (0u64, 0u64);
        let t0 = Instant::now();
        loop {
            let mut a = free_rx.recv()?;
            let buf = a.buf();
            let mut got = 0;
            // Direct reads may return short at the file's end only.
            while got < CHUNK {
                let n = f.read(&mut buf[got..])?;
                if n == 0 {
                    break;
                }
                got += n;
            }
            if got == 0 {
                break;
            }
            total += got as u64;
            let _ = full_tx.send((k, a, got));
            k += 1;
            if got < CHUNK {
                break;
            }
        }
        let t = t0.elapsed();
        drop(full_tx);
        Ok((total, t))
    })
}

/// Write `mib` MiB of the pattern with direct I/O, then `fsync`.
fn write_direct(path: &Path, mib: u64) -> Result<Duration> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(O_DIRECT)
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    let mut a = Aligned::new(CHUNK);
    // The pattern is made outside the timed part: 1 GiB takes ≈ 1 s.
    let mut spent = Duration::ZERO;
    for k in 0..mib {
        fill(a.buf(), k);
        let t0 = Instant::now();
        f.write_all(a.buf())?;
        spent += t0.elapsed();
    }
    let t0 = Instant::now();
    f.sync_all()?;
    Ok(spent + t0.elapsed())
}

#[derive(Default)]
struct Floors {
    read_mbps: f64,
    write_mbps: f64,
    readback_mbps: f64,
}

fn floors() -> Result<Floors> {
    let t: toml::Table = toml::from_str(&std::fs::read_to_string(root().join("budgets.toml"))?)?;
    let m = t.get("msc").and_then(|v| v.as_table()).ok_or_else(|| anyhow!("budgets.toml: no [msc]"))?;
    let g = |k: &str| -> Result<f64> {
        m.get(k)
            .and_then(toml::Value::as_integer)
            .map(|v| v as f64 / 1000.0)
            .ok_or_else(|| anyhow!("budgets.toml: [msc] {k} missing"))
    };
    Ok(Floors { read_mbps: g("read_kbps_min")?, write_mbps: g("write_kbps_min")?, readback_mbps: g("readback_kbps_min")? })
}

/// The firmware's card reader stage counters (`usb msc stats`), labelled.
fn stages(reset: bool) -> Option<String> {
    let r = test_cmd(if reset { "usb msc stats reset" } else { "usb msc stats" }).ok()?;
    let v: Vec<u64> = r.strip_prefix("msc ")?.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    if v.len() < 14 {
        return None;
    }
    let ms = |x: u64| x as f64 / 1000.0;
    Some(format!(
        "wall {} ms: waiting for CBWs {:.0} ms ({}), IN data {:.0} ms ({}), OUT data {:.0} ms ({}), CSWs {:.0} ms, card {:.0} ms ({} ops, max {} us), {} line loads, {} SD commands, {} busy polls{}",
        v[0],
        ms(v[1]),
        v[6],
        ms(v[2]),
        v[7],
        ms(v[3]),
        v[8],
        ms(v[4]),
        ms(v[5]),
        v[9],
        v[10],
        v[11],
        v[12],
        v[13],
        if v.len() >= 18 && (v[14] | v[15]) != 0 {
            format!(", SD timeouts {} CRC errors {} (last STATE {:#x} CMD {:#x})", v[14], v[15], v[16], v[17])
        } else {
            String::new()
        }
    ))
}

/// One phase: run it between profiler hooks and pager counters.
fn phase<T>(i: usize, name: &str, f: impl FnOnce() -> Result<T>) -> Result<(T, Option<u32>)> {
    let c0 = counters();
    let _ = stages(true);
    prof::hook(false, Phase::Before(i, name));
    let r = f();
    prof::hook(false, Phase::After(i, name));
    if let Some(s) = stages(false) {
        log(&format!("   {name} stages: {s}"));
    }
    let c1 = counters();
    let loads = c0.zip(c1).map(|((a, _), (b, _))| b.wrapping_sub(a));
    Ok((r?, loads))
}

pub fn cmd(args: &mut Args) -> Result<String> {
    let mib = u64::from(args.value("mib").map(|s| num(&s)).transpose()?.unwrap_or(64));
    if mib == 0 || mib > 4096 {
        bail!("--mib: 1 to 4096");
    }
    let file = args.value("file").unwrap_or_else(|| DEFAULT_FILE.to_string());
    let check = args.flag("check");
    let floors = if check { floors()? } else { Floors::default() };
    if find_block().is_some() {
        bail!("a card reader block device is already present: switch back first (`usb dev`)");
    }
    let _ = test_cmd("stop")?;
    switch("usb msc")?;
    let dev = wait_for("the card reader's block device", Duration::from_secs(30), find_block)?;
    let sys = PathBuf::from("/sys/block").join(&dev);
    let q = |f: &str| std::fs::read_to_string(sys.join("queue").join(f)).unwrap_or_default().trim().to_string();
    log(&format!(
        "   /dev/{dev}: max_sectors_kb {} (hw {}), {} sectors",
        q("max_sectors_kb"),
        q("max_hw_sectors_kb"),
        std::fs::read_to_string(sys.join("size")).unwrap_or_default().trim()
    ));
    let part = wait_for("its partition", Duration::from_secs(10), || {
        std::fs::read_dir(&sys).ok()?.flatten().find_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            (n.starts_with(&dev) && n != dev).then_some(n)
        })
    })?;
    let result = (|| -> Result<String> {
        let mnt = mount(&part)?;
        log(&format!("   mounted /dev/{part} at {}", mnt.display()));
        prof::hook(false, Phase::Start);
        let mut out = Vec::new();
        let mut bad_floor = Vec::new();
        let mut note = |name: &str, bytes: u64, t: Duration, loads: Option<u32>, floor: f64| {
            let r = mbps(bytes, t);
            let per = loads.map_or(String::new(), |l| {
                format!(", {:.0} line loads/MiB", f64::from(l) / (bytes as f64 / f64::from(1u32 << 20)))
            });
            log(&format!("   {name}: {bytes} B in {:.2} s = {r:.2} MB/s{per}", t.as_secs_f64()));
            out.push(format!("{name} {r:.2} MB/s"));
            if check && r < floor {
                bad_floor.push(format!("{name} {r:.2} < {floor:.2} MB/s"));
            }
        };
        let src = mnt.join(&file);
        if src.exists() {
            let ((n, t), l) = phase(0, "read file", || read_direct(&src, |_, _| {}))?;
            note("read file", n, t, l, floors.read_mbps);
        } else {
            log(&format!("   read file: not run (no {file} on the card)"));
        }
        let dir = mnt.join(TEST_DIR);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("perf.bin");
        let _ = std::fs::remove_file(&path);
        let (t, l) = phase(1, "write", || write_direct(&path, mib))?;
        note("write", mib << 20, t, l, floors.write_mbps);
        let mut want = Aligned::new(CHUNK);
        let mut bad = Vec::new();
        let ((n, t), l) = phase(2, "read back", || {
            read_direct(&path, |k, got| {
                let w = want.buf();
                fill(w, k);
                if got != &w[..got.len()] {
                    bad.push(k);
                }
            })
        })?;
        note("read back", n, t, l, floors.readback_mbps);
        prof::hook(false, Phase::End);
        std::fs::remove_file(&path)?;
        let _ = std::fs::remove_dir(&dir);
        unmount(&part);
        let integrity = if n != mib << 20 {
            format!("INTEGRITY: read back {n} of {} B", mib << 20)
        } else if bad.is_empty() {
            format!("integrity ok ({mib} MiB)")
        } else {
            format!("INTEGRITY: {} MiB differ (first at MiB {})", bad.len(), bad[0])
        };
        if !bad.is_empty() || n != mib << 20 {
            bail!("{}; {integrity}", out.join(", "));
        }
        if !bad_floor.is_empty() {
            bail!("{}; {integrity}; below budgets.toml [msc]: {}", out.join(", "), bad_floor.join(", "));
        }
        Ok(format!("{}; {integrity}", out.join(", ")))
    })();
    if result.is_err() {
        unmount(&part);
    }
    switch("usb dev")?;
    wait_for("the block device to go", Duration::from_secs(20), || find_block().is_none().then_some(()))?;
    std::thread::sleep(Duration::from_secs(2));
    let s = test_cmd("status")?;
    let state = s.split_whitespace().next().unwrap_or("").to_string();
    let r = result?;
    Ok(format!("{r}; player {state} after"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_differs_per_mib_and_is_repeatable() {
        let (mut a, mut b) = (vec![0u8; 4096], vec![0u8; 4096]);
        fill(&mut a, 3);
        fill(&mut b, 3);
        assert_eq!(a, b);
        fill(&mut b, 4);
        assert_ne!(a, b);
        assert!(a.chunks(512).collect::<std::collections::HashSet<_>>().len() == 8);
    }

    #[test]
    fn budgets_have_msc_floors() {
        let f = floors().unwrap();
        assert!(f.read_mbps > 0.0 && f.write_mbps > 0.0 && f.readback_mbps > 0.0);
    }
}
