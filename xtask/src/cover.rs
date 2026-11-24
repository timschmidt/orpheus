//! `xtask cover [--crate NAME] [--fuzz|--fuzz-only] [--html] [--clean] [--report-only] [--add]`
//! (07 §3.6, VER-1):
//! LLVM source-based coverage of the host workspace.
//!
//! * The host workspace's lib, bin and integration tests (no doctests) are
//!   built with `-Cinstrument-coverage -Zcoverage-options=branch` by the
//!   nightly toolchain (`util::nightly`; branch coverage is nightly-only) in
//!   `target/cover/` and run with `LLVM_PROFILE_FILE` pointing at
//!   `target/orpheus/coverage/profraw/`.
//! * `--fuzz-only`: the fuzz corpus replays alone (source `fuzz`; VER-4's
//!   fuzz coverage per crate); a following `--add` run of the host tests
//!   gives the combined numbers.
//! * `--fuzz`: every cargo-fuzz target (`crates/**/fuzz/`, `sim/fuzz/`) is built with the
//!   same flags (plain `cargo build`, libFuzzer in "run these files" mode, no
//!   sanitizer) in `target/cover/fuzz/` and run over its corpus files:
//!   `tests/fuzz-corpus/<target>/`, `<fuzz dir>/corpus/<target>/`,
//!   `tests/regress/<target>/`, `<fuzz dir>/regress/<target>/`.
//! * `llvm-profdata merge`, then `llvm-cov export` (JSON for the numbers,
//!   LCOV) and, with `--html`, `llvm-cov show -format=html`, all with the
//!   llvm-tools of the same nightly. Dependencies are excluded by path: only
//!   files under `crates/`, `sim/` and `xtask/` count, and of those not the
//!   test code itself (`tests/`, `benches/`, `examples/` directories).
//! * Numbers per crate and total: lines (llvm-cov's per-file line summary),
//!   regions and branches computed here as the **union over all
//!   instantiations and binaries** of each source region (a region is
//!   covered if any binary executed it; a branch counts two outcomes).
//!   Approved entries of `coverage-exclusions.toml` are subtracted from the
//!   region and branch totals; entries still `approved-by = "pending
//!   owner"` are listed, not applied (D-9).
//! * Project source files that no instrumented binary contains are listed as
//!   "without data" (they count as uncovered for the M11 gate).
//!
//! Outputs in `target/orpheus/coverage/`: `summary.csv` (per crate + TOTAL),
//! `files.csv`, `functions.csv` (functions with uncovered regions),
//! `no-data.txt`, `lcov.info`, `html/` (with `--html`); `summary.csv` and
//! `files.csv` are also copied to `history/<commit>/`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::args::Args;
use crate::util::{git_commit, log, nightly, nightly_cargo, out_dir, root, run};

const HOST: &str = "x86_64-unknown-linux-gnu";

/// Regions, branches and lines of one unit (function, file, crate, total).
#[derive(Default, Clone, Copy, Debug)]
pub struct Counts {
    pub lines: u64,
    pub lines_cov: u64,
    pub regions: u64,
    pub regions_cov: u64,
    pub regions_excl: u64,
    pub branches: u64,
    pub branches_cov: u64,
}

impl Counts {
    fn add(&mut self, o: &Counts) {
        self.lines += o.lines;
        self.lines_cov += o.lines_cov;
        self.regions += o.regions;
        self.regions_cov += o.regions_cov;
        self.regions_excl += o.regions_excl;
        self.branches += o.branches;
        self.branches_cov += o.branches_cov;
    }
}

pub fn pct(c: u64, t: u64) -> f64 {
    if t == 0 { 100.0 } else { c as f64 * 100.0 / t as f64 }
}

pub struct Opts {
    pub krate: Option<String>,
    pub fuzz: bool,
    /// Only the fuzz corpus replays, no host tests (VER-4: fuzz coverage on
    /// its own; `--add` a host run afterwards for the combined numbers).
    pub fuzz_only: bool,
    pub html: bool,
    pub clean: bool,
    /// Recompute the reports from the last run's profile and binaries.
    pub report_only: bool,
    /// Merge this run into the last run's profile and binaries (one crate
    /// at a time on a small disk; a re-run of a failed suite).
    pub add: bool,
}

pub struct Report {
    pub source: String,
    pub total: Counts,
    pub no_data: Vec<String>,
    pub failures: Vec<String>,
}

pub fn cmd(args: &mut Args) -> Result<String> {
    let o = Opts {
        krate: args.value("crate"),
        fuzz: args.flag("fuzz") || args.flag("fuzz-only"),
        fuzz_only: args.flag("fuzz-only"),
        html: args.flag("html"),
        clean: args.flag("clean"),
        report_only: args.flag("report-only"),
        add: args.flag("add"),
    };
    let r = run_cover(&o)?;
    let t = r.total;
    let verdict = format!(
        "coverage {}: regions {:.2} % ({}/{}, {} excluded), lines {:.2} %, branches {:.2} %; {} files without data; \
         reports in {}",
        r.source,
        pct(t.regions_cov, t.regions),
        t.regions_cov,
        t.regions,
        t.regions_excl,
        pct(t.lines_cov, t.lines),
        pct(t.branches_cov, t.branches),
        r.no_data.len(),
        cov_dir().display()
    );
    if !r.failures.is_empty() {
        bail!("{verdict}; but {} run(s) failed: {}", r.failures.len(), r.failures.join("; "));
    }
    Ok(verdict)
}

/// The coverage step of `xtask gate` (07 §6): reports the numbers; with
/// `strict` (`gate --m11`) fails below 100 % of non-excluded regions or when
/// a project source file has no coverage data.
pub fn gate_step(strict: bool) -> Result<String> {
    let r = run_cover(&Opts {
        krate: None,
        fuzz: strict,
        fuzz_only: false,
        html: false,
        clean: false,
        report_only: false,
        add: false,
    })?;
    let t = r.total;
    let s = format!(
        "regions {:.2} % ({}/{}), lines {:.2} %, branches {:.2} %, {} files without data",
        pct(t.regions_cov, t.regions),
        t.regions_cov,
        t.regions,
        pct(t.lines_cov, t.lines),
        pct(t.branches_cov, t.branches),
        r.no_data.len()
    );
    if !r.failures.is_empty() {
        bail!("{s}; test/fuzz runs failed: {}", r.failures.join("; "));
    }
    if strict && (t.regions_cov < t.regions || !r.no_data.is_empty()) {
        bail!("{s}: below 100 % of non-excluded regions (M11)");
    }
    Ok(if strict { s } else { format!("{s}; report only until M11 (`gate --m11` enforces 100 %)") })
}

pub fn cov_dir() -> PathBuf {
    out_dir().join("coverage")
}

/// `<sysroot>/lib/rustlib/<host>/bin/<tool>` of the toolchain `tc`.
pub fn llvm_tool(tc: &str, tool: &str) -> Result<PathBuf> {
    let out = Command::new("rustc").arg(format!("+{tc}")).args(["--print", "sysroot"]).output()?;
    let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let p = Path::new(&sysroot).join("lib").join("rustlib").join(HOST).join("bin").join(tool);
    if !p.exists() {
        bail!("{} missing: `rustup component add llvm-tools --toolchain {tc}`", p.display());
    }
    Ok(p)
}

fn instrument_env(c: &mut Command, profraw: &Path, extra_cfg: &str) {
    c.env("RUSTFLAGS", format!("-Cinstrument-coverage -Zcoverage-options=branch {extra_cfg}"))
        .env("LLVM_PROFILE_FILE", profraw.join("cov-%p-%8m.profraw"))
        .env("CARGO_INCREMENTAL", "0")
        // Coverage needs no debug info; it only costs disk.
        .env("CARGO_PROFILE_DEV_DEBUG", "0")
        .env("CARGO_PROFILE_TEST_DEBUG", "0")
        .env("CARGO_PROFILE_RELEASE_DEBUG", "0")
        .env_remove("RUSTC_WRAPPER");
}

/// Executables named by `cargo … --message-format=json` output.
pub fn executables(json: &[u8]) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for line in String::from_utf8_lossy(json).lines() {
        let Ok(m) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if m["reason"] == "compiler-artifact"
            && let Some(e) = m["executable"].as_str()
        {
            v.push(PathBuf::from(e));
        }
    }
    v
}

pub fn run_cover(o: &Opts) -> Result<Report> {
    let tc = nightly()?;
    let cov_tool = llvm_tool(&tc, "llvm-cov")?;
    let out = cov_dir();
    let merged = out.join("merged.profdata");
    let objects_list = out.join("objects.txt");
    let source_file = out.join("source.txt");
    let target_dir = root().join("target").join("cover");
    let previous = || -> Result<(Vec<PathBuf>, String)> {
        let list = std::fs::read_to_string(&objects_list)
            .with_context(|| format!("{} (run `xtask cover` first)", objects_list.display()))?;
        if !merged.exists() {
            bail!("{} missing (run `xtask cover` first)", merged.display());
        }
        let src = std::fs::read_to_string(&source_file).unwrap_or_else(|_| "host".into());
        Ok((list.lines().map(PathBuf::from).collect(), src.trim().to_string()))
    };
    let this_source = match (o.fuzz_only, o.fuzz) {
        (true, _) => "fuzz",
        (false, true) => "host+fuzz",
        (false, false) => "host",
    };
    let (mut objects, failures, source) = if o.report_only {
        let (objs, src) = previous()?;
        (objs, Vec::new(), src)
    } else if o.add {
        let (mut objs, src) = previous()?;
        let r = measure(o, &tc, &target_dir, &merged, true)?;
        objs.extend(r.0);
        let host = src.contains("host") || !o.fuzz_only;
        let fuzz = src.contains("fuzz") || o.fuzz;
        let src = match (host, fuzz) {
            (true, true) => "host+fuzz",
            (true, false) => "host",
            _ => "fuzz",
        };
        (objs, r.1, src.to_string())
    } else {
        let r = measure(o, &tc, &target_dir, &merged, false)?;
        (r.0, r.1, this_source.to_string())
    };
    objects.sort();
    objects.dedup();
    objects.retain(|p| p.exists());
    std::fs::write(&objects_list, objects.iter().map(|p| format!("{}\n", p.display())).collect::<String>())?;
    std::fs::write(&source_file, &source)?;
    // `--add` reports everything, not just the added crate.
    let report_crate = if o.add { None } else { o.krate.clone() };
    if objects.is_empty() {
        bail!("no instrumented binaries");
    }

    // 4. Reports.
    let ignore =
        r"(/\.cargo/(registry|git)/|/\.rustup/|^/rustc/|/target/|/tests/|/benches/|/examples/|/tests\.rs$)";
    let cov = |sub: &str| {
        let mut c = Command::new(&cov_tool);
        c.arg(sub).arg(format!("-instr-profile={}", merged.display()));
        c.arg(format!("-ignore-filename-regex={ignore}"));
        for (i, ob) in objects.iter().enumerate() {
            if i > 0 {
                c.arg("-object");
            }
            c.arg(ob);
        }
        if let Some(k) = &report_crate
            && let Some(dir) = crate_dir(k)
        {
            c.arg(dir);
        }
        c
    };
    let lcov = cov("export").arg("-format=lcov").output()?;
    std::fs::write(out.join("lcov.info"), &lcov.stdout)?;
    if o.html {
        let html = out.join("html");
        let _ = std::fs::remove_dir_all(&html);
        run(cov("show")
            .args([
                "-format=html",
                "-show-branches=count",
                "-show-line-counts-or-regions",
                "-show-expansions",
            ])
            .arg(format!("-output-dir={}", html.display())))?;
    }
    let json = cov("export").arg("-format=text").output()?;
    if !json.status.success() {
        bail!("llvm-cov export: {}", String::from_utf8_lossy(&json.stderr));
    }
    let export: Export = serde_json::from_slice(&json.stdout).context("parse llvm-cov export")?;
    drop(json);
    let report = analyse(&export, report_crate.as_deref(), &source, failures)?;
    if o.clean {
        let _ = std::fs::remove_dir_all(&target_dir);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::has_fn_body;

    #[test]
    fn fn_bodies() {
        assert!(!has_fn_body("pub mod a;\npub const T: [u8; 2] = [1, 2];\n// fn x() {}\n"));
        assert!(!has_fn_body("pub trait D {\n    fn open(\n        a: u8,\n    ) -> u8;\n}\n"));
        assert!(has_fn_body("pub fn run<T: X>(t: T) -> u8 {\n    1\n}\n"));
        assert!(!has_fn_body("let x = define_fn ();"));
    }
}

/// Steps 1–3: build and run the instrumented tests (and fuzz replays),
/// merge the profiles into `merged`. Returns the binaries and the failures.
fn measure(
    o: &Opts,
    tc: &str,
    target_dir: &Path,
    merged: &Path,
    add: bool,
) -> Result<(Vec<PathBuf>, Vec<String>)> {
    let profdata_tool = llvm_tool(tc, "llvm-profdata")?;
    let out = cov_dir();
    let profraw = out.join("profraw");
    let _ = std::fs::remove_dir_all(&profraw);
    std::fs::create_dir_all(&profraw)?;
    let mut failures = Vec::new();

    let mut objects = Vec::new();
    // 1. Host tests.
    let sel: Vec<String> = match &o.krate {
        Some(k) => vec!["-p".into(), k.clone()],
        None => vec!["--workspace".into()],
    };
    // `--lib` is an error for a package without one (`xtask`).
    let has_lib = o.krate.as_deref().is_none_or(package_has_lib);
    let targets: &[&str] = if has_lib { &["--lib", "--bins", "--tests"] } else { &["--bins", "--tests"] };
    let test_cmd = |extra: &[&str]| {
        let mut c = nightly_cargo(tc);
        c.current_dir(root())
            .arg("test")
            .args(&sel)
            .args(targets)
            .args(["--target", HOST, "--target-dir"])
            .arg(target_dir)
            .args(extra);
        instrument_env(&mut c, &profraw, "");
        c
    };
    if !o.fuzz_only {
        log("cover: building the instrumented host tests");
        let built = test_cmd(&["--no-run", "--message-format=json-render-diagnostics"]).output()?;
        if !built.status.success() {
            bail!("building the instrumented tests failed:\n{}", String::from_utf8_lossy(&built.stderr));
        }
        objects = executables(&built.stdout);
        log(&format!("cover: running {} test binaries", objects.len()));
        if let Err(e) = run(&mut test_cmd(&["--no-fail-fast"])) {
            failures.push(format!("host tests: {e:#}"));
        }
    }

    // 2. Fuzz corpora.
    if o.fuzz {
        let (objs, fails) = fuzz_replay(tc, target_dir, &profraw)?;
        objects.extend(objs);
        failures.extend(fails);
    }
    objects.sort();
    objects.dedup();
    objects.retain(|p| p.exists());

    // 3. Merge.
    let raws: Vec<PathBuf> = std::fs::read_dir(&profraw)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "profraw"))
        .collect();
    if raws.is_empty() {
        bail!("no .profraw files were written");
    }
    let mut inputs = raws;
    let previous = out.join("previous.profdata");
    if add {
        std::fs::rename(merged, &previous)?;
        inputs.push(previous.clone());
    }
    let mut m = Command::new(&profdata_tool);
    m.args(["merge", "-sparse", "-o"]).arg(merged);
    let list = out.join("profraw.list");
    std::fs::write(&list, inputs.iter().map(|p| format!("{}\n", p.display())).collect::<String>())?;
    m.arg(format!("-f={}", list.display()));
    run(&mut m)?;
    let _ = std::fs::remove_file(&previous);
    let _ = std::fs::remove_dir_all(&profraw);
    let _ = std::fs::remove_file(&list);
    Ok((objects, failures))
}

// ---------------------------------------------------------------------------
// Fuzz replay

pub struct FuzzDir {
    pub dir: PathBuf,
    pub targets: Vec<String>,
}

/// Every cargo-fuzz project under `crates/` and `sim/` (`fuzz/Cargo.toml`
/// with `cargo-fuzz = true`) and its targets (`fuzz_targets/*.rs`).
pub fn fuzz_dirs() -> Vec<FuzzDir> {
    let mut v = Vec::new();
    let mut stack = vec![root().join("crates"), root().join("sim")];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if !p.is_dir() || p.file_name().is_some_and(|n| n == "target" || n == "src") {
                continue;
            }
            if p.file_name().is_some_and(|n| n == "fuzz")
                && std::fs::read_to_string(p.join("Cargo.toml"))
                    .is_ok_and(|t| t.contains("cargo-fuzz = true"))
            {
                let mut targets: Vec<String> = std::fs::read_dir(p.join("fuzz_targets"))
                    .map(|rd| {
                        rd.filter_map(Result::ok)
                            .filter_map(|e| {
                                e.file_name().to_str().and_then(|n| n.strip_suffix(".rs")).map(str::to_string)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                targets.sort();
                v.push(FuzzDir { dir: p, targets });
            } else {
                stack.push(p);
            }
        }
    }
    v.sort_by(|a, b| a.dir.cmp(&b.dir));
    v
}

/// Corpus directories of one target, in the order libFuzzer should see them.
pub fn corpus_dirs(fd: &FuzzDir, t: &str) -> Vec<PathBuf> {
    [
        root().join("tests").join("fuzz-corpus").join(t),
        fd.dir.join("corpus").join(t),
        root().join("tests").join("regress").join(t),
        fd.dir.join("regress").join(t),
    ]
    .into_iter()
    .filter(|d| d.is_dir())
    .collect()
}

fn corpus_files(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let mut stack: Vec<PathBuf> = dirs.to_vec();
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                v.push(p);
            }
        }
    }
    v.sort();
    v
}

/// Seed images for `orpheus-fs`'s `mount_walk` target (written by its
/// `fuzz_smoke` test into `<tmp>/orpheus-fs/fuzz-seeds`).
pub fn fs_seeds(target_dir: &Path) -> Option<PathBuf> {
    // `CARGO_TARGET_TMPDIR` is `<target dir>/<triple>/tmp` with `--target`.
    [target_dir.join(HOST).join("tmp"), target_dir.join("tmp"), root().join("target").join("tmp")]
        .into_iter()
        .map(|d| d.join("orpheus-fs").join("fuzz-seeds"))
        .find(|d| d.join("fat12.img").exists())
}

fn fuzz_replay(tc: &str, target_dir: &Path, profraw: &Path) -> Result<(Vec<PathBuf>, Vec<String>)> {
    let fuzz_target = target_dir.join("fuzz");
    let mut objects = Vec::new();
    let mut failures = Vec::new();
    for fd in fuzz_dirs() {
        let mut c = nightly_cargo(tc);
        c.current_dir(&fd.dir)
            .args([
                "build",
                "--bins",
                "--target",
                HOST,
                "--message-format=json-render-diagnostics",
                "--target-dir",
            ])
            .arg(&fuzz_target);
        instrument_env(&mut c, profraw, "--cfg fuzzing");
        let built = c.output()?;
        if !built.status.success() {
            failures.push(format!("build {}: {}", fd.dir.display(), String::from_utf8_lossy(&built.stderr)));
            continue;
        }
        let exes = executables(&built.stdout);
        for t in &fd.targets {
            let Some(exe) = exes.iter().find(|e| e.file_name().is_some_and(|n| n == t.as_str())) else {
                failures.push(format!("fuzz target {t}: no executable"));
                continue;
            };
            let files = corpus_files(&corpus_dirs(&fd, t));
            log(&format!("cover: fuzz target {t}: {} corpus files", files.len()));
            // A binary that never ran has no profile: leave it out (its
            // functions would only show up as "mismatched data").
            if !files.is_empty() {
                objects.push(exe.clone());
            }
            // libFuzzer starts fuzzing when given no files: never do that.
            for chunk in files.chunks(256) {
                let mut r = Command::new(exe);
                r.args(["-timeout=60", "-rss_limit_mb=4096"]).args(chunk);
                r.env("LLVM_PROFILE_FILE", profraw.join("cov-%p-%8m.profraw"));
                if let Some(s) = fs_seeds(target_dir) {
                    r.env("ORPHEUS_FS_SEEDS", s);
                }
                let st = r.output()?;
                if !st.status.success() {
                    let err = String::from_utf8_lossy(&st.stderr);
                    let tail: Vec<&str> = err.lines().rev().take(6).collect();
                    failures.push(format!(
                        "fuzz target {t} failed on its corpus: {}",
                        tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
                    ));
                }
            }
        }
    }
    Ok((objects, failures))
}

// ---------------------------------------------------------------------------
// Analysis of the llvm-cov JSON export

#[derive(Deserialize)]
struct Export {
    data: Vec<ExportData>,
}

#[derive(Deserialize)]
struct ExportData {
    files: Vec<ExportFile>,
    functions: Vec<ExportFn>,
}

#[derive(Deserialize)]
struct ExportFile {
    filename: String,
    summary: FileSummary,
}

#[derive(Deserialize)]
struct FileSummary {
    lines: Stat,
}

#[derive(Deserialize)]
struct Stat {
    count: u64,
    covered: u64,
}

#[derive(Deserialize)]
struct ExportFn {
    name: String,
    /// `[line_start, col_start, line_end, col_end, count, file_id, expanded_file_id, kind]`
    regions: Vec<Vec<u64>>,
    /// `[line_start, col_start, line_end, col_end, true_count, false_count, file_id, expanded_file_id, kind]`
    #[serde(default)]
    branches: Vec<Vec<u64>>,
    filenames: Vec<String>,
}

/// One `[[exclude]]` entry of `coverage-exclusions.toml`.
#[derive(Deserialize, Debug, Clone)]
pub struct Exclusion {
    pub path: String,
    #[serde(default)]
    pub function: Option<String>,
    pub reason: String,
    #[serde(rename = "approved-by")]
    pub approved_by: String,
}

impl Exclusion {
    pub fn approved(&self) -> bool {
        let a = self.approved_by.trim();
        !a.is_empty() && !a.eq_ignore_ascii_case("pending owner")
    }
}

#[derive(Deserialize, Default)]
struct ExclusionFile {
    #[serde(default)]
    exclude: Vec<Exclusion>,
}

pub fn exclusions() -> Result<Vec<Exclusion>> {
    let p = root().join("coverage-exclusions.toml");
    let Ok(t) = std::fs::read_to_string(&p) else { return Ok(Vec::new()) };
    let f: ExclusionFile = toml::from_str(&t).with_context(|| format!("parse {}", p.display()))?;
    for e in &f.exclude {
        if e.reason.trim().is_empty() {
            bail!("{}: exclusion of {} has no reason", p.display(), e.path);
        }
    }
    Ok(f.exclude)
}

/// Workspace member directories (relative to the root) and package names,
/// plus the cargo-fuzz projects.
fn members() -> Vec<(String, String)> {
    let mut v = Vec::new();
    let mut dirs: Vec<PathBuf> = vec![root().join("sim"), root().join("xtask")];
    for base in ["crates", "crates/codecs"] {
        if let Ok(rd) = std::fs::read_dir(root().join(base)) {
            dirs.extend(rd.filter_map(Result::ok).map(|e| e.path()));
        }
    }
    dirs.extend(fuzz_dirs().into_iter().map(|f| f.dir));
    for d in dirs {
        let Ok(t) = std::fs::read_to_string(d.join("Cargo.toml")) else { continue };
        let Ok(v2) = toml::from_str::<toml::Table>(&t) else { continue };
        let Some(name) = v2.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str()) else {
            continue;
        };
        let rel = d.strip_prefix(root()).unwrap_or(&d).to_string_lossy().into_owned();
        v.push((rel, name.to_string()));
    }
    // Longest prefix first, so `crates/codecs/mp3` wins over `crates/codecs`.
    v.sort_by_key(|(d, _)| std::cmp::Reverse(d.len()));
    v
}

/// Host-workspace package names (no fuzz projects), sorted.
pub fn workspace_packages() -> Vec<String> {
    let fuzz: Vec<PathBuf> = fuzz_dirs().into_iter().map(|f| f.dir).collect();
    let mut v: Vec<String> =
        members().into_iter().filter(|(d, _)| !fuzz.contains(&root().join(d))).map(|(_, n)| n).collect();
    v.sort();
    v
}

/// Whether a package has a library target (`cargo test --lib` is an error
/// otherwise).
pub fn package_has_lib(name: &str) -> bool {
    crate_dir(name).is_none_or(|d| {
        d.join("src").join("lib.rs").exists()
            || std::fs::read_to_string(d.join("Cargo.toml")).is_ok_and(|t| t.contains("[lib]"))
    })
}

fn crate_dir(name: &str) -> Option<PathBuf> {
    members().into_iter().find(|(_, n)| n == name).map(|(d, _)| root().join(d))
}

fn crate_of(rel: &str, members: &[(String, String)]) -> Option<String> {
    members.iter().find(|(d, _)| rel.starts_with(&format!("{d}/"))).map(|(_, n)| n.clone())
}

/// Project source files that count: `*.rs` under the members, without
/// `tests/`, `benches/`, `examples/`, `target/` (and without the fuzz
/// projects unless `fuzz`).
fn project_sources(fuzz: bool) -> Vec<String> {
    let mut v = Vec::new();
    let mut stack = vec![root().join("crates"), root().join("sim"), root().join("xtask")];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if matches!(
                    name.as_str(),
                    "target" | "tests" | "benches" | "examples" | "corpus" | "artifacts"
                ) || (!fuzz && name == "fuzz")
                {
                    continue;
                }
                stack.push(p);
            } else if name.ends_with(".rs")
                && name != "tests.rs"
                && let Ok(rel) = p.strip_prefix(root())
            {
                v.push(rel.to_string_lossy().into_owned());
            }
        }
    }
    v.sort();
    v
}

/// Test code is the instrument, not the subject: files under a `tests/`,
/// `benches/` or `examples/` directory, and `tests.rs` modules.
fn is_test_code(rel: &str) -> bool {
    rel.split('/').any(|c| matches!(c, "tests" | "benches" | "examples" | "tests.rs"))
}

/// Whether Rust source text has a function with a body (a `fn` whose
/// signature ends in `{`, not `;`), comments ignored; macro-generated code
/// is not seen.
fn has_fn_body(text: &str) -> bool {
    let code: String =
        text.lines().map(|l| l.split_once("//").map_or(l, |(a, _)| a)).collect::<Vec<_>>().join("\n");
    let mut rest = code.as_str();
    while let Some(i) = rest.find("fn ") {
        let before = rest[..i].chars().next_back();
        rest = &rest[i + 3..];
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        match (rest.find('{'), rest.find(';')) {
            (Some(b), Some(s)) if b < s => return true,
            (Some(_), None) => return true,
            _ => {}
        }
    }
    false
}

type Span = (u64, u64, u64, u64);

#[derive(Default)]
struct FnAcc {
    name: String,
    file: String,
    line: u64,
    regions: BTreeMap<Span, bool>,
    branches: BTreeMap<Span, (bool, bool)>,
}

fn demangle(s: &str) -> String {
    format!("{:#}", rustc_demangle::demangle(s))
}

fn analyse(e: &Export, krate: Option<&str>, source: &str, failures: Vec<String>) -> Result<Report> {
    let rootp = format!("{}/", root().display());
    let mem = members();
    let excl = exclusions()?;
    let (approved, pending): (Vec<Exclusion>, Vec<Exclusion>) =
        excl.into_iter().partition(Exclusion::approved);
    let wanted = |rel: &str| -> Option<String> {
        if !(rel.starts_with("crates/") || rel.starts_with("sim/") || rel.starts_with("xtask/"))
            || is_test_code(rel)
        {
            return None;
        }
        let c = crate_of(rel, &mem)?;
        match krate {
            Some(k) if k != c => None,
            _ => Some(c),
        }
    };
    let file_excluded = |rel: &str| approved.iter().any(|x| x.function.is_none() && rel == x.path);
    let fn_excluded = |rel: &str, name: &str| {
        approved.iter().any(|x| x.path == rel && x.function.as_deref().is_some_and(|f| name.contains(f)))
    };

    // Group function records (all instantiations, all binaries) by their
    // source position; union the region and branch outcomes by span.
    let mut fns: HashMap<(String, u64, u64), FnAcc> = HashMap::new();
    for d in &e.data {
        for f in &d.functions {
            let Some(first) = f.regions.iter().find(|r| r.len() >= 8 && r[7] == 0) else { continue };
            let Some(fname) = f.filenames.get(first[5] as usize) else { continue };
            let Some(rel) = fname.strip_prefix(&rootp) else { continue };
            if wanted(rel).is_none() {
                continue;
            }
            let acc = fns.entry((rel.to_string(), first[0], first[1])).or_insert_with(|| FnAcc {
                name: demangle(&f.name),
                file: rel.to_string(),
                line: first[0],
                ..FnAcc::default()
            });
            for r in &f.regions {
                // Code regions of the function's own file only.
                if r.len() < 8 || r[7] != 0 || f.filenames.get(r[5] as usize) != Some(fname) {
                    continue;
                }
                let c = acc.regions.entry((r[0], r[1], r[2], r[3])).or_default();
                *c |= r[4] > 0;
            }
            for b in &f.branches {
                if b.len() < 9 || f.filenames.get(b[6] as usize) != Some(fname) {
                    continue;
                }
                let c = acc.branches.entry((b[0], b[1], b[2], b[3])).or_default();
                c.0 |= b[4] > 0;
                c.1 |= b[5] > 0;
            }
        }
    }

    // Per file.
    let mut files: BTreeMap<String, Counts> = BTreeMap::new();
    let mut fn_rows: Vec<(String, String, u64, String, u64, u64)> = Vec::new();
    for acc in fns.values() {
        let excluded = file_excluded(&acc.file) || fn_excluded(&acc.file, &acc.name);
        let fc = files.entry(acc.file.clone()).or_default();
        let rt = acc.regions.len() as u64;
        let rc = acc.regions.values().filter(|c| **c).count() as u64;
        if excluded {
            fc.regions_excl += rt;
            continue;
        }
        fc.regions += rt;
        fc.regions_cov += rc;
        fc.branches += 2 * acc.branches.len() as u64;
        fc.branches_cov += acc.branches.values().map(|(t, f)| u64::from(*t) + u64::from(*f)).sum::<u64>();
        if rc < rt {
            let krate = wanted(&acc.file).unwrap_or_default();
            fn_rows.push((krate, acc.file.clone(), acc.line, acc.name.clone(), rt, rc));
        }
    }
    for d in &e.data {
        for f in &d.files {
            let Some(rel) = f.filename.strip_prefix(&rootp) else { continue };
            if wanted(rel).is_none() || file_excluded(rel) {
                continue;
            }
            let fc = files.entry(rel.to_string()).or_default();
            fc.lines = f.summary.lines.count;
            fc.lines_cov = f.summary.lines.covered;
        }
    }

    // Sources with function bodies that no instrumented binary contains
    // (files of declarations, constants and tables have no regions).
    let no_data: Vec<String> = project_sources(source.contains("fuzz"))
        .into_iter()
        .filter(|f| wanted(f).is_some() && !files.contains_key(f) && !file_excluded(f))
        .filter(|f| std::fs::read_to_string(root().join(f)).is_ok_and(|t| has_fn_body(&t)))
        .collect();

    let mut per_crate: BTreeMap<String, Counts> = BTreeMap::new();
    let mut total = Counts::default();
    for (f, c) in &files {
        if let Some(k) = wanted(f) {
            per_crate.entry(k).or_default().add(c);
        }
        total.add(c);
    }
    let mut no_data_per_crate: BTreeMap<String, usize> = BTreeMap::new();
    for f in &no_data {
        if let Some(k) = wanted(f) {
            *no_data_per_crate.entry(k.clone()).or_default() += 1;
            per_crate.entry(k).or_default();
        }
    }

    // Write the reports.
    let out = cov_dir();
    let commit = git_commit();
    let mut csv = String::from(
        "commit,source,crate,lines_total,lines_covered,line_pct,regions_total,regions_covered,regions_excluded,\
         region_pct,branches_total,branches_covered,branch_pct,files_without_data\n",
    );
    let row = |csv: &mut String, name: &str, c: &Counts, nd: usize| {
        let _ = writeln!(
            csv,
            "{commit},{source},{name},{},{},{:.2},{},{},{},{:.2},{},{},{:.2},{nd}",
            c.lines,
            c.lines_cov,
            pct(c.lines_cov, c.lines),
            c.regions,
            c.regions_cov,
            c.regions_excl,
            pct(c.regions_cov, c.regions),
            c.branches,
            c.branches_cov,
            pct(c.branches_cov, c.branches)
        );
    };
    let mut sorted: Vec<(&String, &Counts)> = per_crate.iter().collect();
    sorted.sort_by(|a, b| pct(a.1.regions_cov, a.1.regions).total_cmp(&pct(b.1.regions_cov, b.1.regions)));
    for (k, c) in &sorted {
        row(&mut csv, k, c, no_data_per_crate.get(*k).copied().unwrap_or(0));
    }
    row(&mut csv, "TOTAL", &total, no_data.len());
    std::fs::write(out.join("summary.csv"), &csv)?;

    let mut fcsv = String::from(
        "crate,file,lines_total,lines_covered,regions_total,regions_covered,regions_excluded,branches_total,\
         branches_covered\n",
    );
    for (f, c) in &files {
        let _ = writeln!(
            fcsv,
            "{},{f},{},{},{},{},{},{},{}",
            wanted(f).unwrap_or_default(),
            c.lines,
            c.lines_cov,
            c.regions,
            c.regions_cov,
            c.regions_excl,
            c.branches,
            c.branches_cov
        );
    }
    std::fs::write(out.join("files.csv"), &fcsv)?;

    fn_rows.sort_by(|a, b| {
        a.0.cmp(&b.0).then((b.4 - b.5).cmp(&(a.4 - a.5))).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2))
    });
    let mut fn_csv = String::from("crate,file,line,regions_total,regions_covered,function\n");
    for (k, f, l, n, t, c) in &fn_rows {
        let _ = writeln!(fn_csv, "{k},{f},{l},{t},{c},\"{}\"", n.replace('"', "'"));
    }
    std::fs::write(out.join("functions.csv"), &fn_csv)?;
    std::fs::write(out.join("no-data.txt"), no_data.iter().map(|f| format!("{f}\n")).collect::<String>())?;
    let hist = out.join("history").join(&commit);
    std::fs::create_dir_all(&hist)?;
    std::fs::write(hist.join("summary.csv"), &csv)?;
    std::fs::write(hist.join("files.csv"), &fcsv)?;

    log(&format!(
        "{:<28}{:>9}{:>9}{:>9}{:>8}",
        "crate (worst first)", "regions", "lines", "branches", "no-data"
    ));
    for (k, c) in &sorted {
        log(&format!(
            "{k:<28}{:>8.2}%{:>8.2}%{:>8.2}%{:>8}",
            pct(c.regions_cov, c.regions),
            pct(c.lines_cov, c.lines),
            pct(c.branches_cov, c.branches),
            no_data_per_crate.get(*k).copied().unwrap_or(0)
        ));
    }
    let pending_names: Vec<String> = pending
        .iter()
        .map(|x| format!("{} {}", x.path, x.function.as_deref().unwrap_or("(whole file)")))
        .collect();
    if !pending_names.is_empty() {
        log(&format!(
            "cover: {} exclusion(s) pending the owner's review, not applied: {}",
            pending_names.len(),
            pending_names.join(", ")
        ));
    }
    let used: BTreeSet<&str> = approved.iter().map(|x| x.path.as_str()).collect();
    for p in used {
        if !root().join(p).exists() {
            log(&format!("cover: warning: exclusion names a missing file {p}"));
        }
    }
    Ok(Report { source: source.to_string(), total, no_data, failures })
}
