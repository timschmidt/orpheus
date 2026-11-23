//! Reading and judging the results of an on-device test.

use std::path::Path;

use anyhow::{Result, bail};
use yoton_host::adfu::Adfu;
use yoton_host::image;
use yoton_proto::fault::{FaultRecord, cfsr_bits};
use yoton_proto::logring;
use yoton_proto::mailbox::BootInfo;
use yoton_proto::mem::{FB_BASE, FB_H, FB_SIZE, FB_W};
use yoton_proto::result::Verdict;

use super::read_persistent;
use crate::util::{log, out_dir};

fn describe_fault(f: &FaultRecord) -> String {
    let kind = f.kind().map_or_else(|| format!("kind {:#x}", f.kind), |k| k.name().to_string());
    let bits: Vec<_> = cfsr_bits(f.cfsr).collect();
    format!(
        "{kind} pc={:#010x} lr={:#010x} sp={:#010x} ipsr={} cfsr={:#x} [{}] mmfar={:#x} bfar={:#x} hfsr={:#x} {}",
        f.pc,
        f.lr,
        f.sp,
        f.ipsr,
        f.cfsr,
        bits.join(","),
        f.mmfar,
        f.bfar,
        f.hfsr,
        f.text_str()
    )
}

/// Print the persistent area and return a verdict line (error if no PASS).
pub fn print_results(d: &mut Adfu, elf: Option<&Path>) -> Result<String> {
    let (r, f, mb) = read_persistent(d)?;
    if let Some(i) = BootInfo::from_word(mb.boot_info) {
        log(&format!("boot info: path {} attempt {} keys {:#x}", i.path, i.attempt, i.keys));
    }
    if let Some(f) = &f {
        log(&format!("FAULT RECORD: {}", describe_fault(f)));
        if let Some(elf) = elf {
            symbolise(elf, f.pc);
            symbolise(elf, f.lr);
        }
    }
    // Log ring: try SRAM3 then the SRAM0 fallback.
    for base in [logring::ALT_BASE, logring::BASE] {
        if let Ok(area) = d.read(base, logring::SIZE) {
            let mut out = vec![0u8; logring::CAP];
            if let Some((n, lost)) = logring::unroll(&area, &mut out) {
                log(&format!("log ring at {base:#x}: {n} bytes{}", if lost { " (wrapped)" } else { "" }));
                let text: String = out[..n]
                    .iter()
                    .map(|&c| {
                        if c == 0 {
                            '\n'
                        } else if c.is_ascii_graphic() || c == b' ' {
                            c as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                for line in text.lines().filter(|l| !l.is_empty()) {
                    log(&format!("  | {line}"));
                }
            }
        }
    }
    let Some(r) = r else {
        bail!("no result block (the test did not start, or crashed before writing one)");
    };
    log(&format!("result: {:?} seq {} \"{}\"", r.verdict, r.seq, r.message_str()));
    for (i, c) in r.counters().iter().enumerate() {
        log(&format!("  counter[{i:2}] = {c:>10} ({c:#010x})"));
    }
    match r.verdict {
        Some(Verdict::Pass) if f.is_none() => Ok(format!("PASS: {}", r.message_str())),
        Some(Verdict::Pass) => bail!("PASS reported but a fault record exists"),
        Some(Verdict::Skip) => Ok(format!("SKIP: {}", r.message_str())),
        v => bail!("{v:?}: {}", r.message_str()),
    }
}

/// Results + screenshot archived under `target/yoton/hil/<name>/`.
pub fn collect(d: &mut Adfu, name: &str, elf: Option<&Path>) -> Result<String> {
    let dir = out_dir().join("hil").join(name);
    std::fs::create_dir_all(&dir)?;
    let fb = d.read(FB_BASE, FB_SIZE as usize)?;
    let png = dir.join("screen.png");
    image::write_rgb565_png(&fb, FB_W, FB_H, &png)?;
    log(&format!("screen -> {}", png.display()));
    let r = d.read(yoton_proto::result::BASE, yoton_proto::result::SIZE)?;
    std::fs::write(dir.join("result.bin"), r)?;
    print_results(d, elf)
}

fn symbolise(elf: &Path, addr: u32) {
    let out = std::process::Command::new("arm-none-eabi-addr2line")
        .args(["-f", "-C", "-e"])
        .arg(elf)
        .arg(format!("{addr:#x}"))
        .output();
    if let Ok(o) = out {
        let s = String::from_utf8_lossy(&o.stdout);
        log(&format!("  {addr:#010x}: {}", s.split_whitespace().collect::<Vec<_>>().join(" ")));
    }
}
