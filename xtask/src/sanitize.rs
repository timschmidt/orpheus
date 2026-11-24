//! `xtask test asan` and `xtask test miri` (07 §3.5, VER-5).
//!
//! * `asan`: the host workspace's lib, bin and integration tests built by the
//!   nightly toolchain (`util::nightly`) with `-Zsanitizer=address` and
//!   `-Zbuild-std` (std instrumented too; mixing is refused by rustc) for
//!   `x86_64-unknown-linux-gnu` in `target/asan/`; LeakSanitizer is part of
//!   ASan on x86_64 Linux and on (`detect_leaks=1`). Then every cargo-fuzz
//!   target under `cargo fuzz run` (ASan is its default sanitizer) over its
//!   corpus directories with `-runs=0` (each input once, no fuzzing). The
//!   log goes to `target/orpheus/asan/`; each sanitizer report is listed
//!   with the test binary or fuzz target that produced it. `--lean` runs one
//!   package at a time without debug info and deletes each package's test
//!   binaries and scratch files after its run (for a nearly full disk).
//! * `miri [--crate NAME]`: `cargo miri test` for the crates in [`MIRI`]
//!   (the host-compilable crates with `unsafe`, and a sample of pure crates
//!   as a second oracle), each with a time box; skipped tests are named in
//!   the table with the reason.

use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use crate::cover::{corpus_dirs, fs_seeds, fuzz_dirs, package_has_lib, workspace_packages};
use crate::util::{log, nightly, nightly_cargo, out_dir, root};

const HOST: &str = "x86_64-unknown-linux-gnu";

/// Run `c` with stdout+stderr merged, echoed and saved to `log_path`;
/// returns (success, the output lines).
fn run_logged(c: &mut Command, log_path: &Path) -> Result<(bool, Vec<String>)> {
    let (r, w) = std::io::pipe()?;
    c.stdout(w.try_clone()?).stderr(w);
    let mut child: Child = c.spawn()?;
    // The command keeps its copies of the pipe's write end: close them, or
    // the reader below never sees end of file.
    c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let mut f = std::fs::File::create(log_path)?;
    let mut lines = Vec::new();
    for line in BufReader::new(r).lines() {
        let line = line.unwrap_or_default();
        eprintln!("{line}");
        let _ = writeln!(f, "{line}");
        lines.push(line);
    }
    Ok((child.wait()?.success(), lines))
}

/// Sanitizer reports in a log: (`where`, the `SUMMARY:` or `ERROR:` line).
/// `where` is the test binary (`Running …`) or the given default.
fn findings(lines: &[String], default_where: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cur = default_where.to_string();
    let mut last_test = String::new();
    for l in lines {
        let t = l.trim();
        if let Some(rest) = t.strip_prefix("Running ") {
            cur = rest.to_string();
        } else if t.starts_with("test ") && t.ends_with("...") {
            last_test = t.trim_start_matches("test ").trim_end_matches(" ...").to_string();
        } else if t.contains("ERROR: AddressSanitizer")
            || t.contains("ERROR: LeakSanitizer")
            || t.contains("runtime error:")
        {
            let at = if last_test.is_empty() {
                cur.clone()
            } else {
                format!("{cur} (last test started: {last_test})")
            };
            out.push((at, t.to_string()));
        } else if t.starts_with("SUMMARY: AddressSanitizer") || t.starts_with("SUMMARY: LeakSanitizer") {
            // One finding per report: the summary joins its `ERROR:` line.
            match out.last_mut() {
                Some((at, msg)) if at.starts_with(cur.as_str()) && !msg.contains("SUMMARY:") => {
                    msg.push_str(" / ");
                    msg.push_str(t);
                }
                _ => out.push((cur.clone(), t.to_string())),
            }
        }
    }
    out
}

pub fn asan(fuzz_only: bool, lean: bool) -> Result<String> {
    let tc = nightly()?;
    let dir = out_dir().join("asan");
    std::fs::create_dir_all(&dir)?;
    let target_dir = root().join("target").join("asan");
    let mut problems: Vec<String> = Vec::new();
    let mut found: Vec<(String, String)> = Vec::new();
    let asan_options = "detect_leaks=1:detect_stack_use_after_return=1:check_initialization_order=1:\
                        strict_init_order=1:abort_on_error=1:symbolize=1";

    if !fuzz_only {
        // (label, cargo package selection and target kinds)
        let groups: Vec<(String, Vec<String>)> = if lean {
            workspace_packages()
                .into_iter()
                .map(|p| {
                    let mut a = vec!["-p".to_string(), p.clone()];
                    if package_has_lib(&p) {
                        a.push("--lib".into());
                    }
                    a.extend(["--bins".into(), "--tests".into()]);
                    (p, a)
                })
                .collect()
        } else {
            vec![(
                "workspace".into(),
                ["--workspace", "--lib", "--bins", "--tests"].iter().map(|s| (*s).to_string()).collect(),
            )]
        };
        log(&format!(
            "asan: host tests (lib, bins, integration tests; -Zbuild-std){}",
            if lean { ", one package at a time" } else { "" }
        ));
        let host_log = dir.join("host.log");
        let _ = std::fs::remove_file(&host_log);
        for (label, sel) in &groups {
            let asan_cargo = |extra: &[&str]| {
                let mut c = nightly_cargo(&tc);
                c.current_dir(root())
                    .arg("test")
                    .args(sel)
                    .args(["--no-fail-fast", "-Zbuild-std", "--target", HOST, "--target-dir"])
                    .arg(&target_dir)
                    .args(extra)
                    .env("RUSTFLAGS", "-Zsanitizer=address")
                    .env("ASAN_OPTIONS", asan_options)
                    .env("CARGO_INCREMENTAL", "0")
                    // The test harness unwinds; with the profile's `panic = "abort"`
                    // -Zbuild-std builds a std that refuses to link.
                    .env("CARGO_PROFILE_DEV_PANIC", "unwind")
                    // Symbols name the frames; line tables cost GBs here.
                    .env("CARGO_PROFILE_DEV_DEBUG", if lean { "0" } else { "line-tables-only" })
                    .env("CARGO_PROFILE_TEST_DEBUG", if lean { "0" } else { "line-tables-only" })
                    .env_remove("RUSTC_WRAPPER");
                c
            };
            let exes = if lean {
                let built = asan_cargo(&["--no-run", "--message-format=json-render-diagnostics"]).output()?;
                crate::cover::executables(&built.stdout)
            } else {
                Vec::new()
            };
            let part = dir.join(format!("host-{label}.log"));
            let (ok, lines) = run_logged(&mut asan_cargo(&[]), &part)?;
            if let (Ok(t), Ok(mut f)) = (
                std::fs::read_to_string(&part),
                std::fs::OpenOptions::new().create(true).append(true).open(&host_log),
            ) {
                let _ = f.write_all(t.as_bytes());
            }
            let _ = std::fs::remove_file(&part);
            let f = findings(&lines, &format!("host tests ({label})"));
            if !ok && f.is_empty() {
                let failed: Vec<&String> = lines
                    .iter()
                    .filter(|l| l.contains("error: test failed") || l.contains("error: could not compile"))
                    .collect();
                problems.push(format!(
                    "host tests ({label}) failed without a sanitizer report: {}",
                    failed.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" | ")
                ));
            }
            found.extend(f);
            if lean {
                // Free the disk: this package's test binaries and the test
                // scratch files (the fuzz seed images stay for mount_walk).
                for e in exes {
                    let _ = std::fs::remove_file(e);
                }
                let tmp = target_dir.join(HOST).join("tmp");
                if let Ok(rd) = std::fs::read_dir(&tmp) {
                    for e in rd.filter_map(Result::ok) {
                        if e.file_name() != "orpheus-fs" {
                            let _ = std::fs::remove_dir_all(e.path());
                        }
                    }
                }
                if let Ok(rd) = std::fs::read_dir(tmp.join("orpheus-fs")) {
                    for e in rd.filter_map(Result::ok) {
                        if e.file_name() != "fuzz-seeds" {
                            let _ = std::fs::remove_dir_all(e.path());
                        }
                    }
                }
            }
        }
    }

    log("asan: fuzz targets over their corpora (cargo fuzz run … -runs=0)");
    let empty = target_dir.join("fuzz-out");
    let mut targets = 0;
    for fd in fuzz_dirs() {
        for t in &fd.targets {
            targets += 1;
            let out_corpus = empty.join(t);
            let _ = std::fs::remove_dir_all(&out_corpus);
            std::fs::create_dir_all(&out_corpus)?;
            let corpora = corpus_dirs(&fd, t);
            let mut c = nightly_cargo(&tc);
            c.current_dir(&fd.dir)
                .args(["fuzz", "run", "--fuzz-dir", ".", "--target", HOST, "--target-dir"])
                .arg(target_dir.join("fuzz"))
                .arg(t)
                .arg(&out_corpus)
                .args(&corpora)
                .args(["--", "-runs=0", "-rss_limit_mb=4096", "-timeout=120"])
                .env("ASAN_OPTIONS", asan_options)
                .env_remove("RUSTFLAGS")
                .env_remove("RUSTC_WRAPPER");
            if let Some(s) = fs_seeds(&target_dir) {
                c.env("ORPHEUS_FS_SEEDS", s);
            }
            let (ok, lines) = run_logged(&mut c, &dir.join(format!("fuzz-{t}.log")))?;
            let mut f = findings(&lines, &format!("fuzz target {t}"));
            // The input that crashed: libFuzzer names the artifact it saved.
            if let Some(a) = lines.iter().find(|l| l.contains("Test unit written to")) {
                for x in &mut f {
                    x.0 = format!("{} [{}]", x.0, a.trim());
                }
            }
            if !ok && f.is_empty() {
                let tail: Vec<&str> = lines.iter().rev().take(4).map(String::as_str).collect();
                problems.push(format!(
                    "fuzz target {t} failed: {}",
                    tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
                ));
            }
            found.extend(f);
            let n: usize = corpora.iter().map(|d| std::fs::read_dir(d).map_or(0, Iterator::count)).sum();
            log(&format!("asan: fuzz target {t}: {n} corpus inputs{}", if ok { ", clean" } else { "" }));
        }
    }
    let _ = std::fs::remove_dir_all(&empty);

    let mut report = String::new();
    for (w, l) in &found {
        report.push_str(&format!("{w}: {l}\n"));
    }
    std::fs::write(dir.join("findings.txt"), &report)?;
    if !found.is_empty() || !problems.is_empty() {
        for (w, l) in &found {
            log(&format!("  FINDING {w}: {l}"));
        }
        for p in &problems {
            log(&format!("  PROBLEM {p}"));
        }
        bail!(
            "{} sanitizer finding(s), {} other failure(s) (logs in {})",
            found.len(),
            problems.len(),
            dir.display()
        );
    }
    Ok(format!(
        "ASan+LSan clean: {}{targets} fuzz targets over their corpora (logs in {})",
        if fuzz_only { "" } else { "host workspace tests, " },
        dir.display()
    ))
}

/// Crates run under Miri: (package, test-target args, tests skipped with
/// the reason, time box in seconds). Miri runs with isolation on, so tests
/// that read files or the clock are skipped here; host-only FFI (the SCSI
/// `ioctl` in `orpheus-host`) cannot run under Miri at all.
/// One Miri row: package, cargo test-target args, `(test name, reason)`
/// pairs skipped, time box in seconds.
pub type MiriCrate = (&'static str, &'static [&'static str], &'static [(&'static str, &'static str)], u64);

pub const MIRI: &[MiriCrate] = &[
    // `unsafe`: volatile register access (`Reg`); the tests use the
    // address arithmetic and field codecs, never a real register.
    ("atj2157-pac", &["--lib"], &[], 300),
    // Uses `unsafe` (SG_IO ioctl) only on a device path the unit tests do not take.
    (
        "orpheus-host",
        &["--lib"],
        &[("image::tests::png_roundtrip", "writes a temporary file (isolation); covered by ASan")],
        300,
    ),
    // Pure crates (second oracle on UB; `#![forbid(unsafe_code)]`).
    ("orpheus-pager", &["--lib"], &[], 300),
    ("orpheus-proto", &["--lib"], &[], 300),
    (
        "orpheus-otp",
        &["--lib"],
        &[
            (
                "kdf::tests::stepping_equals_one_shot",
                "PBKDF2 with thousands of HMAC rounds: over 10 min under Miri; ASan runs it",
            ),
            ("uri::tests::fuzz_smoke", "thousands of mutated URIs: over 5 min under Miri; ASan runs it"),
        ],
        900,
    ),
    (
        "orpheus-settings",
        &["--lib"],
        &[(
            "tests::power_cut_at_every_step",
            "remounts after every write step: over 5 min under Miri; ASan runs it",
        )],
        300,
    ),
    ("orpheus-input", &["--lib"], &[], 300),
    ("orpheus-power", &["--lib"], &[], 300),
    ("orpheus-sdcard", &["--lib"], &[], 300),
    (
        "orpheus-usb",
        &["--lib"],
        &[(
            "msc::extent::tests::iso_reads_across_runs_with_padding",
            "reads a multi-MB ISO image sector by sector: over 10 min under Miri; ASan runs it",
        )],
        600,
    ),
    ("orpheus-bt", &["--lib"], &[], 600),
    ("orpheus-ogg", &["--lib"], &[], 600),
    ("orpheus-meta", &["--lib"], &[], 600),
    ("orpheus-audio", &["--lib"], &[], 600),
];

fn wait_timeout(child: &mut Child, limit: Duration) -> Result<Option<bool>> {
    let start = Instant::now();
    loop {
        if let Some(st) = child.try_wait()? {
            return Ok(Some(st.success()));
        }
        if start.elapsed() > limit {
            // The child leads its own process group (cargo, cargo-miri and
            // the interpreter): end all of them.
            let _ = Command::new("kill").arg("-KILL").arg(format!("-{}", child.id())).status();
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

pub fn miri(only: Option<&str>) -> Result<String> {
    let tc = nightly()?;
    let ok = nightly_cargo(&tc).args(["miri", "--version"]).output().is_ok_and(|o| o.status.success());
    if !ok {
        bail!("Miri is not installed: `rustup component add miri rust-src --toolchain {tc}`");
    }
    let dir = out_dir().join("miri");
    std::fs::create_dir_all(&dir)?;
    let target_dir: PathBuf = root().join("target").join("miri");
    let mut lines = Vec::new();
    let mut failed = Vec::new();
    for (krate, targets, skips, secs) in MIRI {
        if only.is_some_and(|o| o != *krate) {
            continue;
        }
        let log_path = dir.join(format!("{krate}.log"));
        let mut c = nightly_cargo(&tc);
        c.current_dir(root())
            .args(["miri", "test", "-p", krate])
            .args(*targets)
            .arg("--target-dir")
            .arg(&target_dir)
            .arg("--");
        for (name, _) in *skips {
            c.args(["--skip", name]);
        }
        c.env_remove("RUSTFLAGS").env_remove("RUSTC_WRAPPER");
        let f = std::fs::File::create(&log_path)?;
        c.stdout(f.try_clone()?).stderr(f);
        let start = Instant::now();
        std::os::unix::process::CommandExt::process_group(&mut c, 0);
        let mut child = c.spawn()?;
        let r = wait_timeout(&mut child, Duration::from_secs(*secs))?;
        let took = start.elapsed().as_secs();
        let text = std::fs::read_to_string(&log_path).unwrap_or_default();
        let ub = text.lines().find(|l| l.contains("Undefined Behavior"));
        let summary = text.lines().filter(|l| l.starts_with("test result: ")).collect::<Vec<_>>().join("; ");
        let verdict = match (r, ub) {
            (_, Some(u)) => {
                failed.push(format!("{krate}: {}", u.trim()));
                format!("FINDING: {}", u.trim())
            }
            (Some(true), None) => format!("clean ({summary})"),
            (Some(false), None) => {
                let e = text
                    .lines()
                    .find(|l| l.contains("error:") || l.contains("panicked"))
                    .unwrap_or("failed")
                    .trim()
                    .to_string();
                failed.push(format!("{krate}: {e}"));
                format!("FAILED: {e}")
            }
            (None, None) => format!("time box of {secs} s reached (partial: {summary})"),
        };
        log(&format!("miri: {krate:<18} {took:>4} s  {verdict}"));
        lines.push(format!("{krate}: {verdict}"));
    }
    std::fs::write(dir.join("summary.txt"), lines.join("\n") + "\n")?;
    if !failed.is_empty() {
        bail!("Miri: {} crate(s) with findings or failures: {}", failed.len(), failed.join("; "));
    }
    Ok(format!("Miri clean on {} crate(s) (logs in {})", lines.len(), dir.display()))
}
