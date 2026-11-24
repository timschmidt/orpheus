//! Links saved by the linker wrapper `tools/orpheus-ld` (configured in
//! `fw/.cargo/config.toml`), run again with another linker script.
//!
//! rustc links every device binary through the wrapper, which records the
//! complete `rust-lld -flavor gnu` command line and copies the input objects
//! (the fat-LTO object and `symbols.o` are temporary files rustc deletes) to
//! `fw/target/thumbv7em-none-eabihf/<profile>/orpheus-link/<crate>/`. Linking
//! again from there takes well under a second and needs no recompilation.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::util::{root, run};

/// The pass-1 script name every paged binary links with.
pub const FLAT_SCRIPT: &str = "paged-flat.ld";

pub struct SavedLink {
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

/// The fw target directory of a profile.
pub fn profile_dir(release: bool) -> PathBuf {
    root().join("fw/target/thumbv7em-none-eabihf").join(if release { "release" } else { "debug" })
}

impl SavedLink {
    pub fn load(release: bool, bin: &str) -> Result<Self> {
        let dir = profile_dir(release).join("orpheus-link").join(bin.replace('-', "_"));
        if dir.join("unsupported").exists() {
            bail!("{}: {}", dir.display(), std::fs::read_to_string(dir.join("unsupported"))?.trim());
        }
        let args_path = dir.join("args");
        let text = std::fs::read_to_string(&args_path).with_context(|| {
            format!(
                "no saved link at {} (is fw/.cargo/config.toml using tools/orpheus-ld?)",
                args_path.display()
            )
        })?;
        let args: Vec<String> = text.lines().map(str::to_string).collect();
        let cwd =
            std::fs::read_to_string(dir.join("cwd")).map_or_else(|_| root().join("fw"), |s| s.trim().into());
        if !args.iter().any(|a| a.starts_with("-T") && a.ends_with(FLAT_SCRIPT)) {
            bail!("{bin} is not linked with -T{FLAT_SCRIPT}; it is not a paged image");
        }
        Ok(Self { args, cwd })
    }

    /// Directory holding the shared paged-layout scripts (the `-L` path
    /// where `paged-flat.ld` lives).
    pub fn script_dir(&self) -> Result<PathBuf> {
        let mut it = self.args.iter();
        while let Some(a) = it.next() {
            let d = if a == "-L" {
                it.next().map(PathBuf::from)
            } else {
                a.strip_prefix("-L").map(PathBuf::from)
            };
            if let Some(d) = d {
                let d = if d.is_absolute() { d } else { self.cwd.join(d) };
                if d.join(FLAT_SCRIPT).exists() {
                    return Ok(d);
                }
            }
        }
        bail!("no -L directory with {FLAT_SCRIPT} in the saved link")
    }

    /// Link again: `script` replaces `-Tpaged-flat.ld` (None keeps it),
    /// output to `out`, map to `map`, relocations kept for the analysis.
    pub fn link(&self, script: Option<&Path>, out: &Path, map: &Path) -> Result<()> {
        let mut args: Vec<String> = Vec::new();
        let mut it = self.args.iter().peekable();
        while let Some(a) = it.next() {
            if a == "-o" {
                it.next();
                continue;
            }
            if a == "-L"
                && let Some(d) = it.peek()
                && !Path::new(d.as_str()).exists()
            {
                // rustc's temporary raw-dylibs directory, gone by now.
                it.next();
                continue;
            }
            if a.starts_with("-T")
                && a.ends_with(FLAT_SCRIPT)
                && let Some(s) = script
            {
                args.push(format!("-T{}", s.display()));
                continue;
            }
            args.push(a.clone());
        }
        args.push("-o".into());
        args.push(out.display().to_string());
        args.push("--emit-relocs".into());
        args.push(format!("-Map={}", map.display()));
        let mut c = Command::new(rust_lld()?);
        c.current_dir(&self.cwd).args(&args);
        run(&mut c).with_context(|| format!("relink {}", out.display()))
    }
}

/// `rust-lld` of the pinned toolchain.
pub fn rust_lld() -> Result<PathBuf> {
    let out = Command::new("rustc").current_dir(root()).args(["--print", "sysroot"]).output()?;
    let sysroot = String::from_utf8(out.stdout)?.trim().to_string();
    let out = Command::new("rustc").current_dir(root()).arg("-vV").output()?;
    let vv = String::from_utf8(out.stdout)?;
    let host =
        vv.lines().find_map(|l| l.strip_prefix("host: ")).context("rustc -vV: no host")?.trim().to_string();
    let p = Path::new(&sysroot).join("lib/rustlib").join(host).join("bin/rust-lld");
    if !p.exists() {
        bail!("{} not found", p.display());
    }
    Ok(p)
}
