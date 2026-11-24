use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Repository root (the directory holding the workspace `Cargo.toml`).
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Output directory for generated artefacts.
pub fn out_dir() -> PathBuf {
    let d = root().join("target").join("orpheus");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Run a command, inheriting stdio; fail on a non-zero exit.
pub fn run(cmd: &mut Command) -> Result<()> {
    let shown = format!("{cmd:?}");
    let st = cmd.status().with_context(|| format!("spawn {shown}"))?;
    if !st.success() {
        bail!("{shown} exited with {st}");
    }
    Ok(())
}

pub fn cargo() -> Command {
    Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
}

pub fn log(s: &str) {
    eprintln!("{s}");
}

/// The nightly toolchain for coverage (branch coverage), sanitizers and Miri:
/// `$ORPHEUS_NIGHTLY`, default `nightly-2026-11-05` (07 §3.5–3.6).
pub fn nightly() -> Result<String> {
    let tc = std::env::var("ORPHEUS_NIGHTLY").unwrap_or_else(|_| "nightly-2026-11-05".into());
    let ok = Command::new("rustc")
        .arg(format!("+{tc}"))
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        bail!("toolchain {tc} is not installed (`rustup toolchain install {tc}`, or set ORPHEUS_NIGHTLY)");
    }
    Ok(tc)
}

/// `cargo +<tc>` through the rustup proxy, without the outer cargo's
/// `RUSTC`/`RUSTDOC` (they name the pinned stable toolchain). The caller
/// adds the sub-command; `+<tc>` is already the first argument.
pub fn nightly_cargo(tc: &str) -> Command {
    let mut c = Command::new("cargo");
    c.arg(format!("+{tc}")).env_remove("RUSTC").env_remove("RUSTDOC").env_remove("RUSTC_WRAPPER");
    c
}

fn git(args: &[&str]) -> String {
    Command::new("git")
        .current_dir(root())
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Short hash of HEAD, with `-dirty` when tracked files differ from it.
pub fn git_commit() -> String {
    let mut c = git(&["rev-parse", "--short=12", "HEAD"]);
    if c.is_empty() {
        c = "unknown".into();
    }
    let dirty = Command::new("git")
        .current_dir(root())
        .args(["diff", "--quiet", "HEAD", "--"])
        .status()
        .is_ok_and(|s| !s.success());
    if dirty {
        c.push_str("-dirty");
    }
    c
}

/// Committer date of HEAD (`YYYY-MM-DD`).
pub fn git_date() -> String {
    git(&["show", "-s", "--format=%cs", "HEAD"])
}
