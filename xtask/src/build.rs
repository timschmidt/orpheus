//! Firmware and bring-up image builds (device workspace in `fw/`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::args::Args;
use crate::util::{cargo, out_dir, root, run};

pub struct Built {
    pub elf: PathBuf,
    pub bin: PathBuf,
    pub load_addr: u32,
    pub size: usize,
}

/// Build package `pkg` (binary `bin`) in the device workspace and write the
/// flat image next to the ELF in `target/orpheus/` (RAM-only images).
pub fn build_fw(pkg: &str, bin: &str, release: bool, features: Option<&str>) -> Result<Built> {
    let elf = cargo_build(pkg, bin, release, features)?;
    let (bytes, load_addr) = flatten(&elf)?;
    let bin_path = out_dir().join(format!("{bin}.bin"));
    std::fs::write(&bin_path, &bytes)?;
    let elf_copy = out_dir().join(format!("{bin}.elf"));
    std::fs::copy(&elf, &elf_copy)?;
    Ok(Built { elf: elf_copy, bin: bin_path, load_addr, size: bytes.len() })
}

/// `cargo build` of package `pkg` (binary `bin`) in the device workspace;
/// returns the ELF cargo produced.
pub fn cargo_build(pkg: &str, bin: &str, release: bool, features: Option<&str>) -> Result<PathBuf> {
    let fw = root().join("fw");
    let mut c = cargo();
    c.current_dir(&fw).args(["build", "--package", pkg, "--bin", bin]);
    if release {
        c.arg("--release");
    }
    if let Some(f) = features {
        c.args(["--features", f]);
    }
    // Short source paths in panic locations: they are linker-generated
    // strings in the resident image (≈ 14.6 KB with full paths). Cargo
    // concatenates this array with the rustflags of `fw/.cargo/config.toml`.
    let cargo_home = std::env::var("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cargo"));
    c.arg("--config").arg(format!(
        "target.thumbv7em-none-eabihf.rustflags=[\"--remap-path-prefix={}=o\",\"--remap-path-prefix={}=c\"]",
        root().display(),
        cargo_home.display()
    ));
    // Do not leak the host build's environment into the cross build.
    for (k, _) in std::env::vars() {
        if k.starts_with("CARGO_") && k != "CARGO_HOME" {
            c.env_remove(k);
        }
    }
    run(&mut c)?;
    let profile = if release { "release" } else { "debug" };
    Ok(fw.join("target/thumbv7em-none-eabihf").join(profile).join(bin))
}

/// Flat binary of all loadable segments with file contents, from the lowest
/// physical address.
pub fn flatten(elf: &Path) -> Result<(Vec<u8>, u32)> {
    let data = std::fs::read(elf).with_context(|| format!("read {}", elf.display()))?;
    use object::read::elf::{ElfFile32, ProgramHeader};
    let file = ElfFile32::<object::Endianness>::parse(&*data)?;
    let endian = file.endian();
    let mut segs: Vec<(u64, &[u8])> = Vec::new();
    for ph in file.elf_program_headers() {
        if ph.p_type(endian) != object::elf::PT_LOAD {
            continue;
        }
        let d = ph.data(endian, &*data).map_err(|()| anyhow::anyhow!("bad segment"))?;
        if d.is_empty() {
            continue;
        }
        // Load (physical) address: differs from the run address for code
        // copied at run time (stage 2's trampoline).
        segs.push((u64::from(ph.p_paddr(endian)), d));
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
    // The firmware is a paged image: cargo links pass 1, pagelink the rest
    // (target/orpheus/firmware.yfw, resident.bin, pages.bin, layout.json).
    let mut forwarded = vec!["--bin".to_string(), args.value("bin").unwrap_or_else(|| "firmware".into())];
    if args.flag("debug") {
        forwarded.push("--debug".into());
    }
    for key in ["features", "layout"] {
        if let Some(v) = args.value(key) {
            forwarded.push(format!("--{key}"));
            forwarded.push(v);
        }
    }
    crate::pagelink::cmd(&mut Args::from_vec(forwarded))
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
    let max = orpheus_proto::mem::RAM_IMAGE_MAX as usize;
    if b.load_addr != orpheus_proto::mem::IMAGE_BASE {
        bail!("{pkg} loads at {:#x}, expected {:#x}", b.load_addr, orpheus_proto::mem::IMAGE_BASE);
    }
    if b.size > max {
        bail!("{pkg} is {} bytes; RAM images must stay below ymon ({max} bytes)", b.size);
    }
    Ok(b)
}
