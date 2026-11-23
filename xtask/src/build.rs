//! Firmware and bring-up image builds (device workspace in `fw/`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use object::{Object, ObjectSegment};

use crate::args::Args;
use crate::util::{cargo, log, out_dir, root, run};

pub struct Built {
    pub elf: PathBuf,
    pub bin: PathBuf,
    pub load_addr: u32,
    pub size: usize,
}

/// Build package `pkg` (binary `bin`) in the device workspace and write the
/// flat image next to the ELF in `target/yoton/`.
pub fn build_fw(pkg: &str, bin: &str, release: bool, features: Option<&str>) -> Result<Built> {
    let fw = root().join("fw");
    let mut c = cargo();
    c.current_dir(&fw).args(["build", "--package", pkg, "--bin", bin]);
    if release {
        c.arg("--release");
    }
    if let Some(f) = features {
        c.args(["--features", f]);
    }
    // Do not leak the host build's environment into the cross build.
    for (k, _) in std::env::vars() {
        if k.starts_with("CARGO_") && k != "CARGO_HOME" {
            c.env_remove(k);
        }
    }
    run(&mut c)?;
    let profile = if release { "release" } else { "debug" };
    let elf = fw.join("target/thumbv7em-none-eabihf").join(profile).join(bin);
    let (bytes, load_addr) = flatten(&elf)?;
    let bin_path = out_dir().join(format!("{bin}.bin"));
    std::fs::write(&bin_path, &bytes)?;
    let elf_copy = out_dir().join(format!("{bin}.elf"));
    std::fs::copy(&elf, &elf_copy)?;
    Ok(Built { elf: elf_copy, bin: bin_path, load_addr, size: bytes.len() })
}

/// Flat binary of all loadable segments with file contents, from the lowest
/// physical address.
pub fn flatten(elf: &Path) -> Result<(Vec<u8>, u32)> {
    let data = std::fs::read(elf).with_context(|| format!("read {}", elf.display()))?;
    let file = object::File::parse(&*data)?;
    let mut segs: Vec<(u64, &[u8])> = Vec::new();
    for s in file.segments() {
        let d = s.data()?;
        if d.is_empty() {
            continue;
        }
        // Physical (load) address: ELF segments expose it via file range +
        // address; for our images LMA == VMA for loaded data.
        segs.push((s.address(), d));
    }
    if segs.is_empty() {
        bail!("{}: no loadable data", elf.display());
    }
    segs.sort_by_key(|s| s.0);
    let base = segs[0].0;
    let end = segs.iter().map(|(a, d)| a + d.len() as u64).max().unwrap_or(base);
    if end - base > 8 * 1024 * 1024 {
        bail!("{}: segments span {:#x} bytes; wrong layout?", elf.display(), end - base);
    }
    let mut out = vec![0u8; (end - base) as usize];
    for (a, d) in segs {
        let o = (a - base) as usize;
        out[o..o + d.len()].copy_from_slice(d);
    }
    Ok((out, u32::try_from(base)?))
}

pub fn cmd_build(args: &mut Args) -> Result<String> {
    let release = !args.flag("debug");
    let bin = args.value("bin").unwrap_or_else(|| "firmware".into());
    let features = args.value("features");
    let b = build_fw("firmware", &bin, release, features.as_deref())?;
    log(&format!("{} -> {} ({} bytes at {:#x})", b.elf.display(), b.bin.display(), b.size, b.load_addr));
    Ok(format!("ok, {} bytes", b.size))
}

pub fn cmd_bringup(args: &mut Args) -> Result<String> {
    let release = !args.flag("debug");
    let name = args.positional("bring-up image name")?;
    let b = build_bringup(&name, release)?;
    Ok(format!("ok, {} ({} bytes at {:#x})", b.bin.display(), b.size, b.load_addr))
}

pub fn build_bringup(name: &str, release: bool) -> Result<Built> {
    let pkg = format!("bringup-{name}");
    let b = build_fw(&pkg, &pkg, release, None)?;
    let max = yoton_proto::mem::RAM_IMAGE_MAX as usize;
    if b.load_addr != yoton_proto::mem::IMAGE_BASE {
        bail!("{pkg} loads at {:#x}, expected {:#x}", b.load_addr, yoton_proto::mem::IMAGE_BASE);
    }
    if b.size > max {
        bail!("{pkg} is {} bytes; RAM images must stay below ymon ({max} bytes)", b.size);
    }
    Ok(b)
}
