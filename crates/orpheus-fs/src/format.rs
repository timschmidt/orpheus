//! Creating a file system: FAT32 or exFAT on the volume's own region (a
//! quick format: structures only, file data is not overwritten).
//!
//! The region is where [`crate::Volume::mount`] would look: the first
//! FAT/exFAT partition of the MBR, or the whole device when sector 0 is a
//! boot sector. The partition table itself is never written (CLAUDE.md
//! rule 5: no repartitioning); a card whose sector 0 is neither is
//! refused ([`Error::NoFilesystem`]), except an all-zero sector 0 (a
//! superfloppy whose format was cut), which is the whole device again.
//! The file system follows the SD Association's file-system specification by size: FAT32 up to 32 GiB
//! (SDHC), exFAT above (SDXC); and exFAT below 33 MiB, where FAT32's
//! minimum of 65,525 clusters does not fit (the simulator's card).
//!
//! Order: the boot sectors are zeroed first and written last, so a cut
//! format leaves a volume that does not mount (and can be formatted
//! again), never the old boot sector over the new, empty tables.

use crate::block::{BlockDevice, SECTOR_SIZE, Sector};
use crate::volume::{FsKind, region_of};
use crate::{Error, Result};

/// Volume label written by [`format`].
pub const LABEL: &str = "ORPHEUS";

/// Above this many sectors (32 GiB) a card gets exFAT, at or below FAT32.
pub const EXFAT_ABOVE: u32 = 1 << 26;

/// The exFAT recommended up-case table as mkfs.exfat stores it: these four
/// identity runs compressed (`0xFFFF`, length), every other code unit
/// literal. With [`crate::unicode::upcase16`] it is the 5,836-byte table of
/// checksum `0xE619D30D` (`tables::STD_UPCASE_*`), so a formatted card
/// takes the fast up-case path on mount.
const UPCASE_RUNS: [(u16, u16); 4] = [(0x0587, 6134), (0x2185, 843), (0x24EA, 1862), (0x2D26, 53787)];

/// What [`format`] made.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Formatted {
    /// The file system.
    pub kind: FsKind,
    /// First sector of the volume (the partition's start).
    pub start: u32,
    /// Sectors in the volume.
    pub sectors: u32,
    /// Bytes per cluster.
    pub cluster_bytes: u32,
    /// Clusters in the data area / cluster heap.
    pub clusters: u32,
}

/// Format the volume on `dev` (see the module documentation). `kind`
/// `None` chooses by size; `Some(FsKind::Fat32 | FsKind::ExFat)` forces
/// one. `buf` (at least one sector) is the work and zero-fill buffer:
/// larger buffers mean fewer, longer writes. `serial` is the volume
/// serial number.
pub async fn format<D: BlockDevice>(
    dev: &mut D,
    kind: Option<FsKind>,
    serial: u32,
    buf: &mut [Sector],
) -> Result<Formatted, D::Error> {
    if buf.is_empty() {
        return Err(Error::BufferTooSmall);
    }
    let dev_sectors = dev.sector_count();
    if dev_sectors == 0 {
        return Err(Error::NoFilesystem);
    }
    dev.read(0, &mut buf[..1]).await.map_err(Error::Io)?;
    // An all-zero sector 0 is a superfloppy whose format was cut (its boot
    // sector is zeroed first): the whole device again.
    let (start, sectors) = if buf[0].iter().all(|&b| b == 0) {
        (0, dev_sectors)
    } else {
        region_of(&buf[0], dev_sectors, None)?
    };
    // FAT32 to 32 GiB where its 65,525 clusters fit (from 33 MiB), exFAT
    // above and below.
    let fits = sectors <= EXFAT_ABOVE && fat32_layout(start, sectors).is_some();
    let kind = kind.unwrap_or(if fits { FsKind::Fat32 } else { FsKind::ExFat });
    let f = match kind {
        FsKind::ExFat => exfat(dev, start, sectors, serial, buf).await?,
        FsKind::Fat32 => fat32(dev, start, sectors, serial, buf).await?,
        FsKind::Fat12 | FsKind::Fat16 => return Err(Error::Unsupported),
    };
    dev.sync().await.map_err(Error::Io)?;
    Ok(f)
}

fn put16(s: &mut Sector, o: usize, v: u16) {
    s[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(s: &mut Sector, o: usize, v: u32) {
    s[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// `x` rounded up to a multiple of `a` (a power of two).
const fn align_up(x: u32, a: u32) -> u32 {
    (x + a - 1) & !(a - 1)
}

/// Zero `count` sectors from `lba` in writes of up to `buf.len()` sectors.
async fn zero<D: BlockDevice>(
    dev: &mut D,
    mut lba: u32,
    mut count: u32,
    buf: &mut [Sector],
) -> Result<(), D::Error> {
    for s in buf.iter_mut() {
        *s = [0; SECTOR_SIZE];
    }
    while count > 0 {
        let n = count.min(u32::try_from(buf.len()).unwrap_or(u32::MAX));
        dev.write(lba, &buf[..n as usize]).await.map_err(Error::Io)?;
        lba += n;
        count -= n;
    }
    Ok(())
}

async fn write1<D: BlockDevice>(dev: &mut D, lba: u32, s: &Sector) -> Result<(), D::Error> {
    dev.write(lba, core::slice::from_ref(s)).await.map_err(Error::Io)
}

// ------------------------------------------------------------------ FAT32

/// FAT32 clusters must number at least this (fewer is FAT16 by definition).
const FAT32_MIN_CLUSTERS: u32 = 65_525;

/// The FAT32 layout of `sectors` from `start`: (sectors per cluster,
/// reserved sectors, sectors per FAT, clusters). Clusters of 32 KiB (the
/// SD specification's for SDHC), halved until there are enough of them;
/// the data area starts on a cluster boundary of the device.
fn fat32_layout(start: u32, sectors: u32) -> Option<(u32, u32, u32, u32)> {
    let mut spc = 64u32;
    loop {
        // Sectors per FAT for the most clusters there could be, then the
        // reserved area grown so that the data area is aligned.
        let rough = sectors.saturating_sub(32) / spc;
        let fat = (rough + 2).div_ceil(128);
        let data = start + 32 + 2 * fat;
        let reserved = 32 + (align_up(data, spc) - data);
        let clusters = sectors.saturating_sub(reserved + 2 * fat) / spc;
        if (FAT32_MIN_CLUSTERS + 16..0x0FFF_FFF0).contains(&clusters) {
            return Some((spc, reserved, fat, clusters));
        }
        if spc == 1 || clusters >= 0x0FFF_FFF0 {
            return None;
        }
        spc /= 2;
    }
}

async fn fat32<D: BlockDevice>(
    dev: &mut D,
    start: u32,
    sectors: u32,
    serial: u32,
    buf: &mut [Sector],
) -> Result<Formatted, D::Error> {
    let (spc, reserved, fat, clusters) = fat32_layout(start, sectors).ok_or(Error::Unsupported)?;
    // The old boot sectors go first; then the reserved area, both FATs
    // and the root directory's cluster (2) are zeroed.
    zero(dev, start, reserved + 2 * fat + spc, buf).await?;
    let s = &mut buf[0];
    // FAT: media, end of chain, the root directory's cluster.
    *s = [0; SECTOR_SIZE];
    put32(s, 0, 0x0FFF_FFF8);
    put32(s, 4, 0x0FFF_FFFF);
    put32(s, 8, 0x0FFF_FFFF);
    write1(dev, start + reserved, s).await?;
    write1(dev, start + reserved + fat, s).await?;
    // The root directory: the volume label.
    *s = [0; SECTOR_SIZE];
    label83(s);
    s[11] = 0x08;
    write1(dev, start + reserved + 2 * fat, s).await?;
    // FSInfo (1, backup 7): the root's cluster is the one used.
    *s = [0; SECTOR_SIZE];
    put32(s, 0, 0x4161_5252);
    put32(s, 484, 0x6141_7272);
    put32(s, 488, clusters - 1);
    put32(s, 492, 3);
    put32(s, 508, 0xAA55_0000);
    write1(dev, start + 1, s).await?;
    write1(dev, start + 7, s).await?;
    // The boot sector (backup 6), last.
    *s = [0; SECTOR_SIZE];
    s[..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    s[3..11].copy_from_slice(b"ORPHEUS ");
    put16(s, 11, 512);
    s[13] = spc as u8;
    put16(s, 14, reserved as u16);
    s[16] = 2;
    s[21] = 0xF8;
    put16(s, 24, 63);
    put16(s, 26, 255);
    put32(s, 28, start);
    put32(s, 32, sectors);
    put32(s, 36, fat);
    put32(s, 44, 2);
    put16(s, 48, 1);
    put16(s, 50, 6);
    s[64] = 0x80;
    s[66] = 0x29;
    put32(s, 67, serial);
    let mut l = [0u8; 11];
    label83_into(&mut l);
    s[71..82].copy_from_slice(&l);
    s[82..90].copy_from_slice(b"FAT32   ");
    s[510] = 0x55;
    s[511] = 0xAA;
    write1(dev, start + 6, s).await?;
    write1(dev, start, s).await?;
    Ok(Formatted { kind: FsKind::Fat32, start, sectors, cluster_bytes: spc * 512, clusters })
}

/// [`LABEL`] as an 8.3 name field (space padded).
fn label83_into(out: &mut [u8; 11]) {
    *out = [b' '; 11];
    for (o, b) in out.iter_mut().zip(LABEL.bytes()) {
        *o = b;
    }
}

fn label83(s: &mut Sector) {
    let mut l = [0u8; 11];
    label83_into(&mut l);
    s[..11].copy_from_slice(&l);
}

// ------------------------------------------------------------------ exFAT

/// exFAT layout of `sectors` from `start`: (sectors per cluster, FAT
/// offset, FAT length, heap offset, clusters). Clusters as mkfs.exfat
/// chooses them by size (4 KiB to 256 MiB, 32 KiB to 32 GiB, 128 KiB
/// above); the FAT and the heap start on cluster boundaries of the device.
fn exfat_layout(start: u32, sectors: u32) -> Option<(u32, u32, u32, u32, u32)> {
    let spc = match sectors {
        0..=0x7_FFFF => 8,
        0x8_0000..=EXFAT_ABOVE => 64,
        _ => 256,
    };
    let fat_off = align_up(start + 24, spc) - start;
    let rough = sectors.checked_sub(fat_off)? / spc;
    let fat_len = align_up((rough + 2).div_ceil(128), spc);
    let heap = align_up(start + fat_off + fat_len, spc) - start;
    let clusters = sectors.checked_sub(heap)? / spc;
    (16..=0xFFFF_FFF5).contains(&clusters).then_some((spc, fat_off, fat_len, heap, clusters))
}

/// An extended boot sector's byte `i` (zero, the signature at the end).
const fn ext_byte(i: usize) -> u8 {
    match i {
        510 => 0x55,
        511 => 0xAA,
        _ => 0,
    }
}

/// What the exFAT boot sector records.
struct ExGeo {
    start: u32,
    sectors: u32,
    fat_off: u32,
    fat_len: u32,
    heap: u32,
    clusters: u32,
    root_c: u32,
    serial: u32,
    spc: u32,
}

impl ExGeo {
    /// The main boot sector into `s`.
    fn boot(&self, s: &mut Sector) {
        *s = [0; SECTOR_SIZE];
        s[..3].copy_from_slice(&[0xEB, 0x76, 0x90]);
        s[3..11].copy_from_slice(b"EXFAT   ");
        s[64..72].copy_from_slice(&u64::from(self.start).to_le_bytes());
        s[72..80].copy_from_slice(&u64::from(self.sectors).to_le_bytes());
        put32(s, 80, self.fat_off);
        put32(s, 84, self.fat_len);
        put32(s, 88, self.heap);
        put32(s, 92, self.clusters);
        put32(s, 96, self.root_c);
        put32(s, 100, self.serial);
        put16(s, 104, 0x0100);
        s[108] = 9;
        s[109] = self.spc.trailing_zeros() as u8;
        s[110] = 1;
        s[111] = 0x80;
        s[510] = 0x55;
        s[511] = 0xAA;
    }
}

/// The up-case table's code units in order (see [`UPCASE_RUNS`]).
struct Upcase {
    c: u32,
    pend: Option<u16>,
}

impl Iterator for Upcase {
    type Item = u16;
    fn next(&mut self) -> Option<u16> {
        if let Some(p) = self.pend.take() {
            return Some(p);
        }
        if self.c > 0xFFFF {
            return None;
        }
        let c = self.c as u16;
        if let Some(&(_, n)) = UPCASE_RUNS.iter().find(|r| r.0 == c) {
            self.c += u32::from(n);
            self.pend = Some(n);
            return Some(0xFFFF);
        }
        self.c += 1;
        Some(crate::unicode::upcase16(c))
    }
}

fn upcase() -> Upcase {
    Upcase { c: 0, pend: None }
}

/// The exFAT checksum step (boot region and up-case table).
const fn sum32(s: u32, b: u8) -> u32 {
    s.rotate_right(1).wrapping_add(b as u32)
}

async fn exfat<D: BlockDevice>(
    dev: &mut D,
    start: u32,
    sectors: u32,
    serial: u32,
    buf: &mut [Sector],
) -> Result<Formatted, D::Error> {
    let (spc, fat_off, fat_len, heap, clusters) = exfat_layout(start, sectors).ok_or(Error::Unsupported)?;
    let cbytes = spc * 512;
    let bitmap_bytes = clusters.div_ceil(8);
    let bm_cl = bitmap_bytes.div_ceil(cbytes);
    let up_len = upcase().count() as u32 * 2;
    let up_cl = up_len.div_ceil(cbytes);
    let used = bm_cl + up_cl + 1;
    let (bm_c, up_c, root_c) = (2, 2 + bm_cl, 2 + bm_cl + up_cl);
    let cl_lba = |c: u32| start + heap + (c - 2) * spc;
    // The old boot sectors (main and backup) go first.
    zero(dev, start, 24, buf).await?;
    zero(dev, start + fat_off, fat_len, buf).await?;
    zero(dev, cl_lba(2), used * spc, buf).await?;
    // FAT: media, end, then the three chains (bitmap, up-case, root).
    let last = 2 + used;
    let mut k = 0u32;
    while k * 128 < last {
        let s = &mut buf[0];
        *s = [0; SECTOR_SIZE];
        for (j, slot) in s.chunks_exact_mut(4).enumerate() {
            let e = k * 128 + j as u32;
            let v = match e {
                0 => 0xFFFF_FFF8,
                1 => 0xFFFF_FFFF,
                _ if e >= last => break,
                _ if e + 1 == up_c || e + 1 == root_c || e + 1 == last => 0xFFFF_FFFF,
                _ => e + 1,
            };
            slot.copy_from_slice(&v.to_le_bytes());
        }
        write1(dev, start + fat_off + k, &buf[0]).await?;
        k += 1;
    }
    // Allocation bitmap: the first `used` clusters.
    let mut bit = 0u32;
    while bit < used {
        let s = &mut buf[0];
        *s = [0; SECTOR_SIZE];
        let base = bit;
        while bit < used && bit - base < 4096 {
            let i = (bit - base) as usize;
            s[i / 8] |= 1 << (i % 8);
            bit += 1;
        }
        write1(dev, cl_lba(bm_c) + base / 4096, &buf[0]).await?;
    }
    // Up-case table, and its checksum.
    let mut sum = 0u32;
    let mut it = upcase().peekable();
    let mut lba = cl_lba(up_c);
    while it.peek().is_some() {
        let s = &mut buf[0];
        *s = [0; SECTOR_SIZE];
        for pair in s.chunks_exact_mut(2) {
            let Some(u) = it.next() else { break };
            pair.copy_from_slice(&u.to_le_bytes());
            sum = sum32(sum32(sum, pair[0]), pair[1]);
        }
        write1(dev, lba, &buf[0]).await?;
        lba += 1;
    }
    // Root directory: label, bitmap, up-case.
    {
        let s = &mut buf[0];
        *s = [0; SECTOR_SIZE];
        s[0] = 0x83;
        s[1] = LABEL.len() as u8;
        for (i, b) in LABEL.bytes().enumerate() {
            put16(s, 2 + 2 * i, u16::from(b));
        }
        s[32] = 0x81;
        put32(s, 32 + 20, bm_c);
        put32(s, 32 + 24, bitmap_bytes);
        s[64] = 0x82;
        put32(s, 64 + 4, sum);
        put32(s, 64 + 20, up_c);
        put32(s, 64 + 24, up_len);
        write1(dev, cl_lba(root_c), s).await?;
    }
    // Boot region, main and backup: the extended boot sectors, the OEM
    // and reserved sectors, the checksum sector, then sector 0 (built in
    // `buf[0]`: no sector-sized locals in this future).
    let geo = ExGeo { start, sectors, fat_off, fat_len, heap, clusters, root_c, serial, spc };
    let s = &mut buf[0];
    geo.boot(s);
    let mut sum = 0u32;
    for (i, &b) in s.iter().enumerate() {
        if !matches!(i, 106 | 107 | 112) {
            sum = sum32(sum, b);
        }
    }
    // Sectors 1..=8 (zero but the signature 0x55 0xAA at their end), then
    // 9 and 10 (zero).
    for _ in 1..=8 {
        for i in 0..SECTOR_SIZE {
            sum = sum32(sum, ext_byte(i));
        }
    }
    for _ in 0..2 * SECTOR_SIZE {
        sum = sum32(sum, 0);
    }
    for (i, b) in s.iter_mut().enumerate() {
        *b = ext_byte(i);
    }
    for at in [start + 12, start] {
        for i in 1..=8 {
            write1(dev, at + i, &buf[0]).await?;
        }
    }
    for w in buf[0].chunks_exact_mut(4) {
        w.copy_from_slice(&sum.to_le_bytes());
    }
    for at in [start + 12, start] {
        write1(dev, at + 11, &buf[0]).await?;
    }
    geo.boot(&mut buf[0]);
    for at in [start + 12, start] {
        write1(dev, at, &buf[0]).await?;
    }
    Ok(Formatted { kind: FsKind::ExFat, start, sectors, cluster_bytes: cbytes, clusters })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The generated up-case table is mkfs.exfat's, byte for byte (its
    /// length and checksum: `tables::STD_UPCASE_*`, read from a
    /// mkfs.exfat image by `tools/gen_tables.py`).
    #[test]
    fn upcase_table_is_the_standard_one() {
        let mut n = 0u64;
        let mut sum = 0u32;
        for u in upcase() {
            for b in u.to_le_bytes() {
                sum = sum32(sum, b);
            }
            n += 2;
        }
        assert_eq!(n, crate::tables::STD_UPCASE_LEN);
        assert_eq!(sum, crate::tables::STD_UPCASE_CHECKSUM);
    }

    #[test]
    fn layouts() {
        // 32 GiB less a partition offset: FAT32 with 32 KiB clusters, the
        // data area on a cluster boundary.
        let (spc, res, fat, cl) = fat32_layout(8192, (1 << 26) - 8192).unwrap();
        assert_eq!(spc, 64);
        assert_eq!((8192 + res + 2 * fat) % spc, 0);
        assert!(fat * 128 >= cl + 2);
        // 64 MiB: clusters halved until FAT32 has enough.
        let (spc, _, fat, cl) = fat32_layout(0, 1 << 17).unwrap();
        assert!(spc <= 2 && cl >= FAT32_MIN_CLUSTERS && fat * 128 >= cl + 2, "{spc} {cl}");
        // Too small for FAT32.
        assert!(fat32_layout(0, 60_000).is_none());
        // 238 GB (the owner's card): exFAT, 128 KiB clusters, aligned.
        let n = 499_712_000u32;
        let (spc, fo, fl, heap, cl) = exfat_layout(8192, n).unwrap();
        assert_eq!(spc, 256);
        assert_eq!(((8192 + fo) % spc, (8192 + heap) % spc), (0, 0));
        assert!(fl * 128 >= cl + 2 && heap + cl * spc <= n);
    }
}
