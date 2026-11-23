use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Repository root (the directory holding the workspace `Cargo.toml`).
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Output directory for generated artefacts.
pub fn out_dir() -> PathBuf {
    let d = root().join("target").join("yoton");
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
