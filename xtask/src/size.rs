//! `xtask size [--check] [--bin NAME] [--elf PATH]` (07 §4.2): sizes of
//! the ELF by region and, for a paged image, per crate and placement from
//! its `layout.json`; `--check` enforces `budgets.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use object::{Object, ObjectSection, ObjectSymbol, SectionKind};

use crate::args::Args;
use crate::pagelink::{Layout, artefacts};
use crate::util::{log, root};

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
                // The stack has its own budget (and fills the rest of SRAM0).
                if sec.name() != Ok(".stack") {
                    s.bss += size;
                }
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

/// Bytes per crate and placement, from `layout.json`.
#[derive(Default)]
struct CrateSizes {
    resident_code: u64,
    resident_rodata: u64,
    paged_code: u64,
    paged_rodata: u64,
    ram: u64,
}

impl CrateSizes {
    fn total(&self) -> u64 {
        self.resident_code + self.resident_rodata + self.paged_code + self.paged_rodata + self.ram
    }
}

fn paged_report(l: &Layout) -> Vec<String> {
    let mut by_crate: BTreeMap<&str, CrateSizes> = BTreeMap::new();
    for s in &l.sections {
        let c = by_crate.entry(s.krate.as_str()).or_default();
        let n = u64::from(s.size);
        match (s.placement.as_str(), s.kind.as_str()) {
            ("paged", "code") => c.paged_code += n,
            ("paged", _) => c.paged_rodata += n,
            ("ram", _) => c.ram += n,
            (_, "code") => c.resident_code += n,
            _ => c.resident_rodata += n,
        }
    }
    let mut rows: Vec<(&str, CrateSizes)> = by_crate.into_iter().collect();
    rows.sort_by_key(|(name, c)| (std::cmp::Reverse(c.total()), *name));
    let mut out = vec![
        format!(
            "  pages: code {}/{} ({} B in sections), rodata {}/{} ({} B); slot {} B ({} layout)",
            l.pages.code,
            l.pages.code_max,
            l.pages.code_bytes,
            l.pages.rodata,
            l.pages.rodata_max,
            l.pages.rodata_bytes,
            l.slot.bytes,
            l.slot.layout
        ),
        format!(
            "  {:<28}{:>9}{:>9}{:>9}{:>9}{:>9}",
            "crate", "res.code", "res.ro", "pg.code", "pg.ro", "ram"
        ),
    ];
    for (name, c) in rows.iter().take(25) {
        out.push(format!(
            "  {name:<28}{:>9}{:>9}{:>9}{:>9}{:>9}",
            c.resident_code, c.resident_rodata, c.paged_code, c.paged_rodata, c.ram
        ));
    }
    if rows.len() > 25 {
        out.push(format!("  ({} more crates)", rows.len() - 25));
    }
    out
}

pub fn cmd(args: &mut Args) -> Result<String> {
    let check = args.flag("check");
    let bin = args.value("bin").unwrap_or_else(|| "firmware".into());
    let explicit = args.value("elf").map(PathBuf::from);
    let elf = explicit.clone().unwrap_or_else(|| artefacts(&bin).elf);
    if !elf.exists() {
        bail!("{} does not exist; run `cargo xtask build` first", elf.display());
    }
    // A paged image has a layout.json next to it.
    let layout_path = explicit.as_ref().map_or_else(
        || artefacts(&bin).layout,
        |e| artefacts(e.file_stem().and_then(|s| s.to_str()).unwrap_or_default()).layout,
    );
    let layout = Layout::load(&layout_path).ok().filter(|l| Path::new(&l.elf) == elf.as_path());
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
    if let Some(l) = &layout {
        for line in paged_report(l) {
            log(&line);
        }
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
    if let Some(l) = &layout {
        let section = if is_bringup { "bringup" } else { "firmware" };
        let slot_key = if l.slot.layout == "full" { "slot_bytes_full" } else { "slot_bytes_dual" };
        for (what, val, key) in [
            ("code pages", u64::from(l.pages.code), "code_pages"),
            ("rodata pages", u64::from(l.pages.rodata), "rodata_pages"),
            ("slot", u64::from(l.slot.bytes), slot_key),
        ] {
            if let Some(max) = budget(&b, section, key)
                && val > max
            {
                fails.push(format!("{what} {val} > budget {max}"));
            }
        }
    }
    if fails.is_empty() {
        Ok(format!("within budgets (resident {} B, bss {} B)", s.resident, s.bss))
    } else {
        bail!("{}", fails.join("; "))
    }
}
