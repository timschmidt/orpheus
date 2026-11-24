//! `xtask metrics [--elf PATH]` (07 §4.4, VER-10 host side): append this
//! commit's rows to the versioned history in `metrics/`.
//!
//! * `metrics/coverage.csv`: the per-crate rows of the last `xtask cover`
//!   (`target/orpheus/coverage/summary.csv`), tagged with the commit and
//!   source (`host` or `host+fuzz`) they were measured on.
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
        let rows: Vec<String> = m
            .iter()
            .map(|(metric, key, v)| {
                format!("{commit},{date},{build_id},{metric},\"{}\",{v}", key.replace('"', "'"))
            })
            .collect();
        let n = rows.len();
        upsert(&dir.join("static.csv"), STATIC_HEADER, &rows, |row| {
            row.split(',').next() == Some(commit.as_str())
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
fn upsert(path: &Path, header: &str, rows: &[String], stale: impl Fn(&str) -> bool) -> Result<()> {
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

/// Bytes the prologue of each function reserves on the stack: registers
/// pushed (`push` 4 B each, `vpush` 4 B per `s`, 8 B per `d` register) plus
/// `sub sp, #imm` within the first 16 instructions. Largest first.
pub fn stack_frames(elf: &Path) -> Result<Vec<(String, u64)>> {
    let out = Command::new(objdump())
        .args(["-d", "--no-show-raw-insn", "--no-leading-addr"])
        .arg(elf)
        .output()
        .context("spawn llvm-objdump")?;
    if !out.status.success() {
        bail!("llvm-objdump: {}", String::from_utf8_lossy(&out.stderr));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(parse_frames(&text))
}

fn parse_frames(text: &str) -> Vec<(String, u64)> {
    let mut frames: BTreeMap<String, u64> = BTreeMap::new();
    let mut cur: Option<(String, u64, usize)> = None;
    let mut flush = |c: Option<(String, u64, usize)>| {
        if let Some((n, b, _)) = c
            && b > 0
        {
            let e = frames.entry(n).or_default();
            *e = (*e).max(b);
        }
    };
    for line in text.lines() {
        let t = line.trim();
        // Symbol headers: `<name>:` (or `00100000 <name>:` with addresses).
        let header = if line.starts_with(char::is_whitespace) {
            None
        } else {
            t.strip_suffix(">:").and_then(|s| s.split_once('<')).map(|(_, n)| n)
        };
        if let Some(name) = header {
            flush(cur.take());
            cur = Some((format!("{:#}", rustc_demangle::demangle(name)), 0, 0));
            continue;
        }
        let Some((_, bytes, n)) = cur.as_mut() else { continue };
        if t.is_empty() || t.starts_with('.') {
            continue;
        }
        *n += 1;
        if *n > 16 {
            continue;
        }
        let mut parts = t.splitn(2, char::is_whitespace);
        let op = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("").split(" @").next().unwrap_or("").trim();
        let regs = |r: &str| -> Vec<String> {
            let inner = r.trim_start_matches('{').split('}').next().unwrap_or("");
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
        };
        match op {
            "push" | "push.w" | "stmdb" => {
                let r = if op == "stmdb" { rest.split_once(',').map_or("", |x| x.1).trim() } else { rest };
                if op != "stmdb" || rest.starts_with("sp!") {
                    *bytes += 4 * regs(r).len() as u64;
                }
            }
            "vpush" | "vpush.64" | "vpush.32" => {
                *bytes += regs(rest).iter().map(|r| if r.starts_with('d') { 8 } else { 4 }).sum::<u64>();
            }
            "sub" | "sub.w" | "subw" | "sub.n" => {
                let ops: Vec<&str> = rest.split(',').map(str::trim).collect();
                if ops.first() == Some(&"sp")
                    && let Some(imm) = ops.last().and_then(|x| x.strip_prefix('#'))
                {
                    let v = if let Some(h) = imm.strip_prefix("0x") {
                        u64::from_str_radix(h, 16).unwrap_or(0)
                    } else {
                        imm.parse().unwrap_or(0)
                    };
                    *bytes += v;
                }
            }
            _ => {}
        }
    }
    flush(cur.take());
    let mut v: Vec<(String, u64)> = frames.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
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
}
