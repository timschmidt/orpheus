//! `xtask metrics [--elf PATH]` (07 §4.4, VER-10 host side): append this
//! commit's rows to the versioned history in `metrics/`.
//!
//! * `metrics/coverage.csv`: the per-crate rows of the last `xtask cover`
//!   (`target/orpheus/coverage/summary.csv`), tagged with the commit and
//!   source (`host`, `fuzz` or `host+fuzz`) they were measured on.
//! * `metrics/static.csv`: from the last `xtask build` (`target/orpheus/
//!   firmware.elf` and `layout.json`, or `--elf`): resident/bss/stack bytes,
//!   other regions, panic sites, page counts, slot bytes, and the 40 largest
//!   stack frames (prologue `push`/`vpush`/`sub sp` from `llvm-objdump`).
//!
//! Rows already present for the same commit (and source) are replaced, so a
//! re-run on one commit does not duplicate them. Columns: 07 §4.4.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use object::{Object, ObjectSection};

use crate::args::Args;
use crate::pagelink::{Layout, artefacts};
use crate::util::{git_commit, git_date, log, root};

pub const COVERAGE_HEADER: &str = "commit,date,source,crate,lines_total,lines_covered,regions_total,\
                                   regions_covered,regions_excluded,branches_total,branches_covered,files_without_data";
pub const STATIC_HEADER: &str = "commit,date,build_id,metric,key,value";

pub fn cmd(args: &mut Args) -> Result<String> {
    let elf = args.value("elf").map_or_else(|| artefacts("firmware").elf, PathBuf::from);
    let dir = root().join("metrics");
    std::fs::create_dir_all(&dir)?;
    let date = git_date();
    let mut done = Vec::new();

    // Coverage.
    let summary = crate::cover::cov_dir().join("summary.csv");
    if let Ok(text) = std::fs::read_to_string(&summary) {
        let mut rows = Vec::new();
        let mut key = None;
        for l in text.lines().skip(1) {
            // commit,source,crate,lt,lc,lpct,rt,rc,rx,rpct,bt,bc,bpct,nodata
            let f: Vec<&str> = l.split(',').collect();
            if f.len() < 14 {
                continue;
            }
            key = Some((f[0].to_string(), f[1].to_string()));
            rows.push(format!(
                "{},{date},{},{},{},{},{},{},{},{},{},{}",
                f[0], f[1], f[2], f[3], f[4], f[6], f[7], f[8], f[10], f[11], f[13]
            ));
        }
        if let Some((commit, source)) = key {
            let n = rows.len();
            upsert(&dir.join("coverage.csv"), COVERAGE_HEADER, &rows, |row| {
                let f: Vec<&str> = row.split(',').collect();
                f.len() > 2 && f[0] == commit && f[2] == source
            })?;
            done.push(format!("coverage: {n} rows ({commit}, {source})"));
        }
    } else {
        log(&format!(
            "metrics: no {} (run `cargo xtask cover` first); coverage not recorded",
            summary.display()
        ));
    }

    // Static firmware numbers.
    if elf.exists() {
        let commit = git_commit();
        let s = crate::size::measure(&elf)?;
        let layout = Layout::load(&elf.with_file_name("layout.json"))
            .ok()
            .filter(|l| Path::new(&l.elf) == elf.as_path());
        let build_id = layout.as_ref().map_or_else(|| "-".to_string(), |l| l.build_id.clone());
        let mut m: Vec<(String, String, u64)> = vec![
            ("resident_bytes".into(), String::new(), s.resident),
            ("bss_bytes".into(), String::new(), s.bss),
            ("stack_bytes".into(), String::new(), stack_section(&elf)?),
            ("panic_sites".into(), String::new(), s.panic_callers as u64),
        ];
        for (r, n) in &s.other {
            m.push(("region_bytes".into(), (*r).to_string(), *n));
        }
        if let Some(l) = &layout {
            m.push(("code_pages".into(), String::new(), u64::from(l.pages.code)));
            m.push(("rodata_pages".into(), String::new(), u64::from(l.pages.rodata)));
            m.push(("slot_bytes".into(), l.slot.layout.clone(), u64::from(l.slot.bytes)));
        }
        let frames = stack_frames(&elf)?;
        for (name, bytes) in frames.iter().take(40) {
            m.push(("stack_frame".into(), name.clone(), *bytes));
        }
        // The static worst case (`xtask stack`) needs the link map.
        let map = elf.with_extension("map");
        if map.exists() {
            match crate::stack::metrics(&elf, &map) {
                Ok(rows) => m.extend(rows.into_iter().map(|(k, v)| ("stack_static".to_string(), k, v))),
                Err(e) => log(&format!("metrics: stack analysis failed ({e:#}); stack_static not recorded")),
            }
        }
        let rows: Vec<String> = m
            .iter()
            .map(|(metric, key, v)| {
                format!("{commit},{date},{build_id},{metric},\"{}\",{v}", key.replace('"', "'"))
            })
            .collect();
        let n = rows.len();
        // The browser build's rows (build id `web`, `xtask wasm --record`) stay.
        upsert(&dir.join("static.csv"), STATIC_HEADER, &rows, |row| {
            let f: Vec<&str> = row.split(',').collect();
            f.first() == Some(&commit.as_str()) && f.get(2) != Some(&"web")
        })?;
        done.push(format!("static: {n} rows ({commit}, {})", elf.display()));
    } else {
        log(&format!(
            "metrics: no {} (run `cargo xtask build` first); static numbers not recorded",
            elf.display()
        ));
    }
    if done.is_empty() {
        bail!("nothing to record");
    }
    Ok(done.join("; "))
}

/// Replace the rows `stale` matches, append `rows`.
pub fn upsert(path: &Path, header: &str, rows: &[String], stale: impl Fn(&str) -> bool) -> Result<()> {
    let old = std::fs::read_to_string(path).unwrap_or_default();
    let mut out = String::from(header);
    out.push('\n');
    for l in old.lines().skip(1).filter(|l| !l.is_empty() && !stale(l)) {
        out.push_str(l);
        out.push('\n');
    }
    for r in rows {
        out.push_str(r);
        out.push('\n');
    }
    std::fs::write(path, out).with_context(|| format!("write {}", path.display()))
}

fn stack_section(elf: &Path) -> Result<u64> {
    let data = std::fs::read(elf)?;
    let f = object::File::parse(&*data)?;
    Ok(f.section_by_name(".stack").map_or(0, |s| s.size()))
}

/// `llvm-objdump`: the toolchain's (rust-toolchain.toml lists llvm-tools),
/// else the system one.
fn objdump() -> PathBuf {
    let sysroot = Command::new("rustc")
        .current_dir(root())
        .args(["--print", "sysroot"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let p = Path::new(&sysroot).join("lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-objdump");
    if p.exists() { p } else { PathBuf::from("llvm-objdump") }
}

/// `llvm-objdump -d` of `elf` (with addresses, without raw bytes).
pub fn disassemble(elf: &Path) -> Result<String> {
    let out = Command::new(objdump())
        .args(["-d", "--no-show-raw-insn"])
        .arg(elf)
        .output()
        .context("spawn llvm-objdump")?;
    if !out.status.success() {
        bail!("llvm-objdump: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Bytes the prologue of each function reserves on the stack: registers
/// pushed (`push` 4 B each, `vpush` 4 B per `s`, 8 B per `d` register) plus
/// `sub sp, #imm` within the first 16 instructions. Largest first.
pub fn stack_frames(elf: &Path) -> Result<Vec<(String, u64)>> {
    Ok(parse_frames(&disassemble(elf)?))
}

fn parse_frames(text: &str) -> Vec<(String, u64)> {
    let mut frames: BTreeMap<String, u64> = BTreeMap::new();
    for f in parse_functions(text) {
        if f.prologue > 0 {
            let e = frames.entry(f.name).or_default();
            *e = (*e).max(f.prologue);
        }
    }
    let mut v: Vec<(String, u64)> = frames.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

/// One function of a disassembly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnFrame {
    /// Start address (`None` in a listing without addresses).
    pub addr: Option<u32>,
    /// Demangled name without hash.
    pub name: String,
    /// Bytes reserved in the first 16 instructions (the `stack_frame` metric).
    pub prologue: u64,
    /// Deepest the function itself takes the stack below its entry SP: the
    /// running sum of every SP adjustment in address order (`push`,
    /// `str rX, [sp, #-4]!`, `vpush`, `sub sp` add; `pop`, `vpop`,
    /// `add sp`, `ldr rX, [sp], #4`, `ldm sp!` subtract; clamped at 0), so a
    /// prologue that shrink-wrapping moved behind an early exit, or a second
    /// prologue copy, still counts.
    pub peak: u64,
    /// A `bl`/`b` to its own start (direct recursion: relocations do not
    /// show branches within one section).
    pub self_call: bool,
}

/// Every function of an `llvm-objdump -d` listing (with or without leading
/// addresses).
pub fn parse_functions(text: &str) -> Vec<FnFrame> {
    let mut out = Vec::new();
    // (function, raw symbol, instructions seen, running depth, depth at r7)
    let mut cur: Option<(FnFrame, String, usize, i64, Option<i64>)> = None;
    for line in text.lines() {
        let t = line.trim();
        // Symbol headers: `<name>:` (or `00100000 <name>:` with addresses).
        let header = if line.starts_with(char::is_whitespace) {
            None
        } else {
            t.strip_suffix(">:").and_then(|s| s.split_once('<'))
        };
        if let Some((addr, name)) = header {
            if let Some((f, ..)) = cur.take() {
                out.push(f);
            }
            let f = FnFrame {
                addr: u32::from_str_radix(addr.trim(), 16).ok(),
                name: format!("{:#}", rustc_demangle::demangle(name)),
                prologue: 0,
                peak: 0,
                self_call: false,
            };
            cur = Some((f, name.to_string(), 0, 0, None));
            continue;
        }
        let Some((f, raw, n, depth, fp)) = cur.as_mut() else { continue };
        // Drop the address column (`  100234:`).
        let t = match t.split_once(char::is_whitespace) {
            Some((a, rest))
                if a.len() > 1 && a.ends_with(':') && u32::from_str_radix(&a[..a.len() - 1], 16).is_ok() =>
            {
                rest.trim()
            }
            _ => t,
        };
        if t.is_empty() || t.starts_with('.') || t.starts_with('<') {
            continue;
        }
        *n += 1;
        let mut parts = t.splitn(2, char::is_whitespace);
        let op = parts.next().unwrap_or("");
        let full = parts.next().unwrap_or("");
        let rest = full.split(" @").next().unwrap_or("").trim();
        let delta = sp_delta(op, rest);
        if delta > 0 && *n <= 16 {
            f.prologue += delta.unsigned_abs();
        }
        *depth = (*depth + delta).max(0);
        // The frame pointer: `add r7, sp, #n` records the depth at r7,
        // `sub sp, r7, #n` / `mov sp, r7` (epilogues) restore from it.
        let ops: Vec<&str> = rest.split(',').map(str::trim).collect();
        let base = op.split('.').next().unwrap_or(op);
        match (base, ops.as_slice()) {
            ("add" | "addw", ["r7", "sp", k]) => *fp = imm(k).map(|k| *depth - k),
            ("sub" | "subw", ["sp", "r7", k]) => {
                if let (Some(at), Some(k)) = (*fp, imm(k)) {
                    *depth = (at + k).max(0);
                }
            }
            ("mov", ["sp", "r7"]) => {
                if let Some(at) = *fp {
                    *depth = at.max(0);
                }
            }
            _ => {}
        }
        f.peak = f.peak.max(depth.unsigned_abs());
        if matches!(op, "bl" | "b" | "b.w" | "b.n")
            && let Some(target) = full.split_once('<').map(|x| x.1.trim_end_matches('>'))
            && target == raw.as_str()
        {
            f.self_call = true;
        }
    }
    if let Some((f, ..)) = cur.take() {
        out.push(f);
    }
    out
}

/// Registers of a list `{r4, r5, d8-d9}`.
fn reg_list(r: &str) -> Vec<String> {
    let inner = r.trim().trim_start_matches('{').split('}').next().unwrap_or("");
    let mut v = Vec::new();
    for item in inner.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some((a, b)) = item.split_once('-') {
            let (pa, na) = a.split_at(1);
            let (_, nb) = b.split_at(1);
            if let (Ok(x), Ok(y)) = (na.parse::<u32>(), nb.parse::<u32>()) {
                for i in x..=y {
                    v.push(format!("{pa}{i}"));
                }
            }
        } else {
            v.push(item.to_string());
        }
    }
    v
}

/// `#123`, `#0x7b`, `#-4`.
fn imm(s: &str) -> Option<i64> {
    let s = s.trim().strip_prefix('#')?;
    let (neg, s) = s.strip_prefix('-').map_or((false, s), |r| (true, r));
    let v =
        if let Some(h) = s.strip_prefix("0x") { i64::from_str_radix(h, 16).ok()? } else { s.parse().ok()? };
    Some(if neg { -v } else { v })
}

/// Bytes one instruction moves SP down (negative: up). Conditional forms
/// (`popeq`) count like the plain ones; `sub sp, r7, #n` (frame-pointer
/// epilogue) and other SP writes from another register do not count.
fn sp_delta(op: &str, rest: &str) -> i64 {
    let base = op.split('.').next().unwrap_or(op);
    let base = ["vpush", "vpop", "push", "pop"].into_iter().find(|b| base.starts_with(b)).unwrap_or(base);
    let regs = |r: &str| reg_list(r).iter().map(|r| if r.starts_with('d') { 8 } else { 4 }).sum::<i64>();
    let ops: Vec<&str> = rest.split(',').map(str::trim).collect();
    let sp_sp = ops.first() == Some(&"sp") && (ops.len() == 2 || ops.get(1) == Some(&"sp"));
    let after_sp = || rest.split_once(',').map_or("", |x| x.1);
    match base {
        "push" | "vpush" => regs(rest),
        "pop" | "vpop" => -regs(rest),
        "stmdb" | "stmfd" if rest.starts_with("sp!") => regs(after_sp()),
        "ldm" | "ldmia" | "ldmfd" if rest.starts_with("sp!") => -regs(after_sp()),
        // `str rX, [sp, #-4]!` (a one-register push).
        "str" if rest.ends_with("]!") && rest.contains("[sp, #-") => {
            rest.split("[sp, ").nth(1).and_then(|x| imm(x.trim_end_matches(['!', ']']))).map_or(0, |v| -v)
        }
        // `ldr rX, [sp], #4` (a one-register pop).
        "ldr" if rest.contains("[sp], #") => -rest.split("], ").nth(1).and_then(imm).unwrap_or(0),
        "sub" | "subw" if sp_sp => ops.last().and_then(|x| imm(x)).unwrap_or(0),
        "add" | "addw" if sp_sp => -ops.last().and_then(|x| imm(x)).unwrap_or(0),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::parse_frames;

    #[test]
    fn prologue_bytes() {
        let text = "\
<foo>:
\tpush\t{r4, r5, r6, r7, lr}
\tadd\tr7, sp, #12
\tpush.w\t{r8, r9, r10}
\tvpush\t{d8-d9}
\tsub.w\tsp, sp, #1032
\tbl\t0x100 <bar>

<bar>:
\tpush\t{r7, lr}
\tsub\tsp, #8
\tbx\tlr

<leaf>:
\tbx\tlr
";
        let f = parse_frames(text);
        assert_eq!(f, vec![("foo".to_string(), 20 + 12 + 16 + 1032), ("bar".to_string(), 16)]);
    }

    /// With addresses (what `xtask stack` reads): a shrink-wrapped prologue
    /// behind an early return, a second prologue copy, one-register pushes,
    /// conditional pops, the frame-pointer epilogue, direct recursion.
    #[test]
    fn peak_with_addresses() {
        let text = "\
Disassembly of section .text:

00100000 <wrapped>:
  100000:      \tcmp\tr0, #0x0
  100002:      \tit\teq
  100004:      \tbxeq\tlr
  100006:      \tpush\t{r4, r5, r6, r7, lr}
  100008:      \tadd\tr7, sp, #0xc
  10000a:      \tstr\tr11, [sp, #-4]!
  10000e:      \tsub\tsp, #0x100
  100010:      \tbl\t0x100000 <wrapped>
  100014:      \tsub.w\tsp, r7, #0x10
  100018:      \tpopeq\t{r4, r5, r6, r7, pc}
  10001a:      \tldr\tr11, [sp], #4
  10001e:      \tpop\t{r4, r5, r6, r7, pc}
  100020:      \tpush\t{r4, lr}
  100022:      \tsubw\tsp, sp, #0x204
  100026:      \taddw\tsp, sp, #0x204
  10002a:      \tpop\t{r4, pc}
  10002c: 00 00 00 00 \t.word\t0x00000000

00100030 <leaf>:
  100030:      \tbx\tlr
";
        let f = super::parse_functions(text);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].addr, Some(0x100000));
        assert_eq!(f[0].name, "wrapped");
        assert_eq!(f[0].prologue, 20 + 4 + 0x100 + 8 + 0x204);
        // `sub sp, r7, #0x10` restores the depth to 24 (r7 = entry - 8),
        // the pops take it to 0, the second copy reaches 8 + 0x204.
        assert_eq!(f[0].peak, 8 + 0x204);
        assert!(f[0].self_call);
        assert_eq!((f[1].name.as_str(), f[1].peak, f[1].self_call), ("leaf", 0, false));
    }
}
