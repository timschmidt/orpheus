//! `cargo xtask wasm [--demo] [--record] [--test [--browser chromium|firefox]]`
//! (WEB-2, 09 §4): Orpheus in the browser.
//!
//! Builds `sim/web` (the simulator's apps and services, the codecs, the DSP
//! chain, the file system, the library and the picture decoders) for
//! `wasm32-unknown-unknown` with the `web` profile, binds it with
//! `wasm-bindgen` and optimises it with `wasm-opt`, into
//! `target/orpheus/site/try/pkg/` (the page itself, `site/try/`, is copied
//! by `xtask site`, which keeps `try/pkg`), packs the committed demo card
//! (`sim/web/demo/card/`) into `try/pkg/demo.bin`, the page's one download
//! of demo data. Then checks the size budgets of `budgets.toml` `[web]`
//! (module + the page's scripts, gzip; the demo bundle, as served).
//!
//! The tools are pinned here: a `wasm-bindgen` or `wasm-opt` of another
//! version on `PATH` is not used; the pinned one is installed with
//! `cargo install --locked` into `target/orpheus/tools/` when missing.
//!
//! * `--demo` downloads the demo card's sources pinned in
//!   `sim/web/demo/sources.toml` (SHA-256 checked) and derives
//!   `sim/web/demo/card/` and the credits with `ffmpeg` ([`crate::demo`]).
//! * `--record` adds the sizes to `metrics/static.csv` (build id `web`).
//! * `--test` runs the page's self-test in a headless browser
//!   (`tests/web/try.mjs`, Node).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::args::Args;
use crate::util::{cargo, git_commit, git_date, log, out_dir, root, run};

/// `wasm-bindgen` CLI: the same version as the `wasm-bindgen` crate in
/// `Cargo.lock` (`sim/web/Cargo.toml` pins it).
pub const WASM_BINDGEN: &str = "0.2.130";
/// `wasm-opt` (Binaryen 116) through the `wasm-opt` crate.
pub const WASM_OPT: &str = "0.116.1";
const WASM_OPT_BINARYEN: &str = "version 116";
const TARGET: &str = "wasm32-unknown-unknown";
/// The demo card's bundle in `try/pkg/` (`site/try/app.js` fetches it).
const DEMO_BUNDLE: &str = "demo.bin";
/// Features the Rust target enables by default (wasm-opt must accept them).
const FEATURES: [&str; 6] = [
    "--enable-bulk-memory",
    "--enable-nontrapping-float-to-int",
    "--enable-sign-ext",
    "--enable-mutable-globals",
    "--enable-reference-types",
    "--enable-multivalue",
];

fn tools_dir() -> PathBuf {
    out_dir().join("tools")
}

fn version_of(bin: &Path) -> Option<String> {
    let o = Command::new(bin).arg("--version").output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// The pinned tool: on `PATH` if it is that version, else in
/// `target/orpheus/tools/bin` (installed there if missing).
fn tool(bin: &str, krate: &str, version: &str, reports: &str) -> Result<PathBuf> {
    let local = tools_dir().join("bin").join(bin);
    for cand in [PathBuf::from(bin), local.clone()] {
        if version_of(&cand).is_some_and(|v| v.contains(reports)) {
            return Ok(cand);
        }
    }
    log(&format!("installing {krate} {version} into {} (once)", tools_dir().display()));
    run(cargo()
        .args(["install", krate, "--version", version, "--locked", "--root"])
        .arg(tools_dir())
        .env_remove("RUSTC")
        .env_remove("RUSTC_WRAPPER"))?;
    if !version_of(&local).is_some_and(|v| v.contains(reports)) {
        bail!("{} does not report {reports}", local.display());
    }
    Ok(local)
}

/// Build `orpheus-install-web` into `site/install/pkg` and check its size
/// against `budgets.toml [web] install_gzip_bytes`.
fn install_module(bindgen: &Path, opt: &Path) -> Result<String> {
    run(cargo().args([
        "build",
        "-p",
        "orpheus-install-web",
        "--lib",
        "--target",
        TARGET,
        "--profile",
        "web",
    ]))?;
    let wasm = root().join("target").join(TARGET).join("web").join("orpheus_install_web.wasm");
    let pkg = out_dir().join("site").join("install").join("pkg");
    if pkg.exists() {
        std::fs::remove_dir_all(&pkg)?;
    }
    std::fs::create_dir_all(&pkg)?;
    run(Command::new(bindgen)
        .args(["--target", "web", "--no-typescript", "--out-name", "orpheus_install_web", "--out-dir"])
        .arg(&pkg)
        .arg(&wasm))?;
    let bg = pkg.join("orpheus_install_web_bg.wasm");
    run(Command::new(opt).arg("-Oz").args(FEATURES).arg(&bg).arg("-o").arg(&bg))?;
    let mut total = 0;
    for f in files(&pkg).into_iter().chain(files(&root().join("site/install"))) {
        let data = std::fs::read(&f)?;
        let gz = gzip_len(&data)?;
        total += gz;
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
        log(&format!("  {name:<28} {:>9} B  gzip {gz:>9} B", data.len()));
    }
    let limit = budget("install_gzip_bytes")?;
    if total > limit {
        bail!("installer over budget: {total} B gzip > {limit} B");
    }
    Ok(format!("installer {} KB gzip (budget {} KB)", total.div_ceil(1000), limit / 1000))
}

fn gzip_len(data: &[u8]) -> Result<u64> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    e.write_all(data)?;
    Ok(e.finish()?.len() as u64)
}

/// Files of a directory (not recursive), sorted.
fn files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn budget(key: &str) -> Result<u64> {
    let t: toml::Table = toml::from_str(&std::fs::read_to_string(root().join("budgets.toml"))?)?;
    t.get("web")
        .and_then(|w| w.get(key))
        .and_then(toml::Value::as_integer)
        .and_then(|v| u64::try_from(v).ok())
        .with_context(|| format!("budgets.toml: no [web] {key}"))
}

pub fn cmd(args: &mut Args) -> Result<String> {
    let regen = args.flag("demo");
    let record = args.flag("record");
    let test = args.flag("test");
    let browser = args.value("browser").unwrap_or_else(|| "chromium".into());
    let mut notes = Vec::new();
    if regen {
        notes.push(format!("demo: {}", crate::demo::regenerate()?));
    }
    let bindgen = tool("wasm-bindgen", "wasm-bindgen-cli", WASM_BINDGEN, WASM_BINDGEN)?;
    let opt = tool("wasm-opt", "wasm-opt", WASM_OPT, WASM_OPT_BINARYEN)?;
    run(cargo().args(["build", "-p", "orpheus-web", "--lib", "--target", TARGET, "--profile", "web"]))?;
    let wasm = root().join("target").join(TARGET).join("web").join("orpheus_web.wasm");
    let pkg = out_dir().join("site").join("try").join("pkg");
    if pkg.exists() {
        std::fs::remove_dir_all(&pkg)?;
    }
    std::fs::create_dir_all(&pkg)?;
    run(Command::new(&bindgen)
        .args(["--target", "web", "--no-typescript", "--out-name", "orpheus_web", "--out-dir"])
        .arg(&pkg)
        .arg(&wasm))?;
    let bg = pkg.join("orpheus_web_bg.wasm");
    run(Command::new(&opt).arg("-O3").args(FEATURES).arg(&bg).arg("-o").arg(&bg))?;
    // The demo card, fetched by the page next to the module (its own budget).
    let bundle = crate::demo::bundle()?;
    std::fs::write(pkg.join(DEMO_BUNDLE), &bundle)?;

    // Sizes: the module with its data, its glue, the page's own scripts.
    let mut total_gz = 0;
    let mut rows = Vec::new();
    let page = root().join("site/try");
    for f in files(&pkg).into_iter().chain(files(&page)) {
        if f.file_name().is_some_and(|n| n == DEMO_BUNDLE) {
            continue;
        }
        let data = std::fs::read(&f)?;
        let gz = gzip_len(&data)?;
        total_gz += gz;
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
        log(&format!("  {name:<24} {:>9} B  gzip {gz:>9} B", data.len()));
        rows.push((name, data.len() as u64, gz));
    }
    let wasm_raw = std::fs::metadata(&bg)?.len();
    let limit = budget("total_gzip_bytes")?;
    let demo_limit = budget("demo_bytes")?;
    let demo_gz = gzip_len(&bundle)?;
    log(&format!("  {DEMO_BUNDLE:<24} {:>9} B  gzip {demo_gz:>9} B", bundle.len()));
    let verdict = format!(
        "{} (module {} KB, {} KB with the page's files gzip, budget {} KB; demo card {} KB, budget {} KB)",
        pkg.display(),
        wasm_raw.div_ceil(1000),
        total_gz.div_ceil(1000),
        limit / 1000,
        (bundle.len() as u64).div_ceil(1000),
        demo_limit / 1000
    );
    if record {
        let commit = git_commit();
        let date = git_date();
        let row = |metric: &str, key: &str, v: u64| format!("{commit},{date},web,{metric},\"{key}\",{v}");
        let mut lines = Vec::new();
        for (name, raw, gz) in &rows {
            lines.push(row("web_file_gzip_bytes", name, *gz));
            lines.push(row("web_file_bytes", name, *raw));
        }
        lines.push(row("web_total_gzip_bytes", "", total_gz));
        lines.push(row("web_demo_bytes", DEMO_BUNDLE, bundle.len() as u64));
        crate::metrics::upsert(
            &root().join("metrics/static.csv"),
            crate::metrics::STATIC_HEADER,
            &lines,
            |row| {
                let f: Vec<&str> = row.split(',').collect();
                f.len() > 2 && f[0] == commit && f[2] == "web"
            },
        )?;
        notes.push(format!("{} rows in metrics/static.csv", lines.len()));
    }
    if total_gz > limit || bundle.len() as u64 > demo_limit {
        bail!("over budget: {verdict}");
    }
    // The browser installer (WEB-3): orpheus-host over WebUSB.
    notes.push(install_module(&bindgen, &opt)?);
    if test {
        run(Command::new("node").arg(root().join("tests/web/install.mjs")))?;
        notes.push("installer mock-device suite passed".into());
        // Node 20 has WebSocket behind a flag; 22 and later by default.
        let major = Command::new("node")
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .trim_start_matches('v')
                    .split('.')
                    .next()?
                    .parse::<u32>()
                    .ok()
            })
            .context("node (Node.js) is needed for --test")?;
        let mut node = Command::new("node");
        if major < 22 {
            node.arg("--experimental-websocket");
        }
        run(node.arg(root().join("tests/web/try.mjs")).args(["--browser", &browser]))?;
        notes.push(format!("self-test passed in {browser}"));
    }
    if !notes.is_empty() {
        return Ok(format!("{verdict}; {}", notes.join("; ")));
    }
    Ok(verdict)
}
