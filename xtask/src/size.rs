//! `xtask size [--check] [--elf PATH]` (07 §4.2).

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use object::{Object, ObjectSection, ObjectSymbol, SectionKind};

use crate::args::Args;
use crate::util::{log, out_dir, root};

#[derive(Default, Debug)]
pub struct Sizes {
    /// Loaded bytes in SRAM0 (text, rodata, data).
    pub resident: u64,
    /// Zero-initialised / uninitialised bytes in SRAM0.
    pub bss: u64,
    /// Bytes per region name (SRAM1/2/3, paged).
    pub other: BTreeMap<&'static str, u64>,
    pub uses_core_fmt: bool,
    pub panic_callers: usize,
    pub largest: Vec<(String, u64)>,
}

fn region(addr: u64) -> &'static str {
    match addr {
        0x0010_0000..0x0013_8000 => "sram0",
        0x0015_8000..0x0016_0000 => "sram1",
        0x0020_0000..0x0030_0000 => "sram2/paged-code",
        0x0030_0000..0x0040_0000 => "sram3/paged-rodata",
        _ => "other",
    }
}

pub fn measure(elf: &PathBuf) -> Result<Sizes> {
    let data = std::fs::read(elf).with_context(|| format!("read {}", elf.display()))?;
    let f = object::File::parse(&*data)?;
    let mut s = Sizes::default();
    for sec in f.sections() {
        let size = sec.size();
        if size == 0 || sec.address() == 0 {
            continue;
        }
        let r = region(sec.address());
        let nobits = matches!(sec.kind(), SectionKind::UninitializedData | SectionKind::UninitializedTls);
        if r == "sram0" {
            if nobits {
                s.bss += size;
            } else {
                s.resident += size;
            }
        } else {
            *s.other.entry(r).or_default() += size;
        }
    }
    let mut syms: Vec<(String, u64)> = Vec::new();
    for sym in f.symbols() {
        let Ok(name) = sym.name() else { continue };
        if name.contains("core3fmt5write") || name.contains("core::fmt::write") {
            s.uses_core_fmt = true;
        }
        if name.contains("core9panicking") {
            s.panic_callers += 1;
        }
        if sym.size() > 0 {
            syms.push((name.to_string(), sym.size()));
        }
    }
    syms.sort_by_key(|s| std::cmp::Reverse(s.1));
    syms.truncate(10);
    s.largest = syms;
    Ok(s)
}

fn budgets() -> Result<toml::Table> {
    let p = root().join("budgets.toml");
    let t = std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
    Ok(toml::from_str(&t)?)
}

fn budget(t: &toml::Table, section: &str, key: &str) -> Option<u64> {
    t.get(section)?.get(key)?.as_integer().and_then(|v| u64::try_from(v).ok())
}

pub fn cmd(args: &mut Args) -> Result<String> {
    let check = args.flag("check");
    let elf = args.value("elf").map_or_else(|| out_dir().join("firmware.elf"), PathBuf::from);
    if !elf.exists() {
        bail!("{} does not exist; run `cargo xtask build` first", elf.display());
    }
    let s = measure(&elf)?;
    log(&format!("{}", elf.display()));
    log(&format!("  resident (SRAM0 loaded)   {:>8} bytes", s.resident));
    log(&format!("  bss (SRAM0)               {:>8} bytes", s.bss));
    for (r, n) in &s.other {
        log(&format!("  {r:<26}{n:>8} bytes"));
    }
    log(&format!("  core::fmt linked          {}", s.uses_core_fmt));
    log(&format!("  panic symbols             {}", s.panic_callers));
    log("  largest symbols:");
    for (n, z) in &s.largest {
        log(&format!("    {z:>7}  {n}"));
    }
    if !check {
        return Ok(format!("resident {} B, bss {} B", s.resident, s.bss));
    }
    let b = budgets()?;
    let is_bringup = elf.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("bringup-"));
    let mut fails = Vec::new();
    if is_bringup {
        if let Some(max) = budget(&b, "bringup", "image_bytes")
            && s.resident > max
        {
            fails.push(format!("bring-up image {} > {max}", s.resident));
        }
    } else {
        for (what, val, key) in [("resident", s.resident, "resident_bytes"), ("bss", s.bss, "bss_bytes")] {
            if let Some(max) = budget(&b, "firmware", key)
                && val > max
            {
                fails.push(format!("{what} {val} > budget {max}"));
            }
        }
        if s.uses_core_fmt {
            fails.push("core::fmt::write is linked into the firmware".into());
        }
    }
    if fails.is_empty() {
        Ok(format!("within budgets (resident {} B, bss {} B)", s.resident, s.bss))
    } else {
        bail!("{}", fails.join("; "))
    }
}
