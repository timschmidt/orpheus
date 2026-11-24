//! Shared helpers for the integration tests: image creation with the Linux
//! tools (`mkfs.fat`, `mkfs.exfat`), `fsck`, mtools, a deterministic RNG.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use orpheus_fs::host::{FileDisk, HostError, block_on};
use orpheus_fs::{CacheSlot, MountOptions, Volume};

pub type Vol<'c> = Volume<'c, FileDisk>;
pub type FsErr = orpheus_fs::Error<HostError>;

/// Per-test scratch directory under the cargo target dir.
pub fn scratch(name: &str) -> PathBuf {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("orpheus-fs").join(name);
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

pub fn have(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run(cmd: &mut Command) -> (bool, String) {
    let o = cmd.output().expect("spawn");
    let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&o.stderr));
    (o.status.success(), s)
}

/// File-system flavours used by the tests.
#[derive(Clone, Copy, Debug)]
pub enum Fmt {
    /// FAT with (bits, sectors per cluster, size in MiB).
    Fat(u8, u8, u32),
    /// exFAT with (cluster bytes, size in MiB).
    ExFat(u32, u32),
}

impl Fmt {
    pub fn is_exfat(self) -> bool {
        matches!(self, Fmt::ExFat(..))
    }
    pub fn label(self) -> String {
        match self {
            Fmt::Fat(b, s, m) => format!("fat{b}-s{s}-{m}m"),
            Fmt::ExFat(c, m) => format!("exfat-c{c}-{m}m"),
        }
    }
}

/// The standard matrix: FAT12/16/32 with several cluster sizes and exFAT.
pub fn matrix() -> Vec<Fmt> {
    vec![
        Fmt::Fat(12, 1, 2),
        Fmt::Fat(12, 8, 8),
        Fmt::Fat(16, 1, 16),
        Fmt::Fat(16, 8, 64),
        Fmt::Fat(32, 1, 64),
        Fmt::Fat(32, 8, 300),
        Fmt::ExFat(512, 16),
        Fmt::ExFat(4096, 64),
        Fmt::ExFat(32768, 128),
    ]
}

/// Create a sparse image and format it.
pub fn make_image(path: &Path, fmt: Fmt) {
    let mib = match fmt {
        Fmt::Fat(_, _, m) | Fmt::ExFat(_, m) => u64::from(m),
    };
    let f = std::fs::File::create(path).unwrap();
    f.set_len(mib << 20).unwrap();
    drop(f);
    let (ok, out) = match fmt {
        Fmt::Fat(bits, spc, _) => run(Command::new("mkfs.fat")
            .args(["-F", &bits.to_string(), "-s", &spc.to_string(), "-n", "ORPHEUSTEST", "-i", "1234ABCD"])
            .arg(path)),
        Fmt::ExFat(c, _) => run(Command::new("mkfs.exfat")
            .args(["-q", "-P", "none", "-L", "ORPHEUSTEST", "-c", &c.to_string()])
            .arg(path)),
    };
    assert!(ok, "mkfs failed for {fmt:?}: {out}");
}

/// `fsck -n`; returns (clean, output).
pub fn fsck(path: &Path, exfat: bool) -> (bool, String) {
    if exfat {
        run(Command::new("fsck.exfat").arg("-n").arg(path))
    } else {
        run(Command::new("fsck.fat").arg("-n").arg(path))
    }
}

pub fn assert_fsck(path: &Path, exfat: bool) {
    let (ok, out) = fsck(path, exfat);
    assert!(ok, "fsck reports problems on {}:\n{out}", path.display());
}

/// mtools: copy a host file into a FAT image.
pub fn mcopy_in(img: &Path, host: &Path, dest: &str) {
    let (ok, out) =
        run(Command::new("mcopy").arg("-n").arg("-o").arg("-i").arg(img).arg(host).arg(format!("::{dest}")));
    assert!(ok, "mcopy {} -> {dest}: {out}", host.display());
}

/// mtools: create a directory in a FAT image.
pub fn mmd(img: &Path, dir: &str) {
    let (ok, out) = run(Command::new("mmd").arg("-i").arg(img).arg(format!("::{dir}")));
    assert!(ok, "mmd {dir}: {out}");
}

/// mtools: delete a file.
pub fn mdel(img: &Path, file: &str) {
    let (ok, out) = run(Command::new("mdel").arg("-i").arg(img).arg(format!("::{file}")));
    assert!(ok, "mdel {file}: {out}");
}

/// mtools: read a file out of a FAT image.
pub fn mread(img: &Path, file: &str) -> Vec<u8> {
    let o = Command::new("mtype").arg("-i").arg(img).arg(format!("::{file}")).output().unwrap();
    assert!(o.status.success(), "mtype {file}: {}", String::from_utf8_lossy(&o.stderr));
    o.stdout
}

/// `dump.exfat -s dir`: the raw text listing of a directory.
pub fn exfat_dump_dir(img: &Path, dir: &str) -> String {
    let (_, out) = run(Command::new("dump.exfat").arg("-s").arg(dir).arg(img));
    out
}

/// Deterministic xorshift RNG.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

/// Names exercising several scripts.
pub fn unicode_name(i: usize) -> String {
    const PARTS: &[&str] = &[
        "Пётр Ильич Чайковский",
        "Σύμφωνα με τον Αριστοτέλη",
        "Ünïcødé Ŝtrïng ĂăĄą",
        "日本語のファイル名",
        "Mixed Case Long File Name",
        "émoji 😀 and more",
        "Ǆemal Ǉubica",
        "ẞtraße",
    ];
    format!("{} {:05}.flac", PARTS[i % PARTS.len()], i)
}

pub fn cache() -> Vec<CacheSlot> {
    vec![CacheSlot::EMPTY; 8]
}

/// Mount an image file read/write (default options).
pub fn mount<'c>(path: &Path, cache: &'c mut [CacheSlot]) -> Vol<'c> {
    let mut dev = FileDisk::open(path).unwrap();
    dev.elide_zero_writes = true;
    block_on(Volume::mount(dev, cache, MountOptions::default())).expect("mount")
}

pub fn unmount(vol: Vol<'_>) {
    let (_, r) = block_on(vol.unmount());
    r.expect("unmount");
}

/// Read a whole file through orpheus-fs.
pub fn read_all(vol: &mut Vol<'_>, path: &str) -> Result<Vec<u8>, FsErr> {
    block_on(async {
        let mut f = vol.open(path, orpheus_fs::OpenMode::Read).await?;
        let mut out = vec![0u8; f.size() as usize];
        let n = vol.read_exact_or_eof(&mut f, &mut out).await?;
        assert_eq!(n as u64, f.size());
        Ok(out)
    })
}

/// Write a whole file through orpheus-fs in chunks of varying size.
pub fn write_all(vol: &mut Vol<'_>, path: &str, data: &[u8], rng: &mut Rng) -> Result<(), FsErr> {
    block_on(async {
        let mut f = vol.open(path, orpheus_fs::OpenMode::Truncate).await?;
        let mut off = 0;
        while off < data.len() {
            let n = match rng.below(4) {
                0 => 1 + rng.below(700) as usize,
                1 => 512 * (1 + rng.below(16) as usize),
                2 => 1 + rng.below(70_000) as usize,
                _ => 4096,
            }
            .min(data.len() - off);
            vol.write(&mut f, &data[off..off + n]).await?;
            off += n;
        }
        vol.close(f).await
    })
}

/// List a directory: (name, is_dir, size).
pub fn list(vol: &mut Vol<'_>, path: &str) -> Vec<(String, bool, u64)> {
    block_on(async {
        let mut cur = vol.open_dir(path).await.expect("open_dir");
        let mut e = orpheus_fs::DirEntry::new();
        let mut v = Vec::new();
        while vol.read_dir(&mut cur, &mut e).await.expect("read_dir") {
            v.push((e.name().to_string(), e.is_dir(), e.size()));
        }
        v
    })
}
