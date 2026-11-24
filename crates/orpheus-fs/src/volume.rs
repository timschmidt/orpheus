//! Volume: mount (MBR / superfloppy, FAT12/16/32, exFAT), geometry, FAT and
//! allocation-bitmap access, cluster allocation, dirty flags and flushing.

use crate::block::{BlockDevice, SECTOR_SIZE, Sector};
use crate::cache::{Cache, CacheSlot, Kind, Mirror};
use crate::{Error, Result};

/// File-system type of a mounted volume.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FsKind {
    /// FAT12.
    Fat12,
    /// FAT16.
    Fat16,
    /// FAT32.
    Fat32,
    /// exFAT.
    ExFat,
}

/// Calendar time used for timestamps (local time, no zone).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DateTime {
    /// 1980..=2107.
    pub year: u16,
    /// 1..=12.
    pub month: u8,
    /// 1..=31.
    pub day: u8,
    /// 0..=23.
    pub hour: u8,
    /// 0..=59.
    pub minute: u8,
    /// 0..=59.
    pub second: u8,
    /// 0..=99 hundredths of a second.
    pub centis: u8,
}

impl DateTime {
    /// 2000-01-01 00:00:00, the default until the owner sets the clock.
    pub const DEFAULT: DateTime =
        DateTime { year: 2000, month: 1, day: 1, hour: 0, minute: 0, second: 0, centis: 0 };

    /// FAT/exFAT packed date (high 16 bits) and time (low 16 bits).
    #[must_use]
    pub fn to_fat(&self) -> (u16, u16) {
        let y = self.year.clamp(1980, 2107) - 1980;
        let date = (y << 9) | (u16::from(self.month.clamp(1, 12)) << 5) | u16::from(self.day.clamp(1, 31));
        let time = (u16::from(self.hour.min(23)) << 11)
            | (u16::from(self.minute.min(59)) << 5)
            | u16::from(self.second.min(59) / 2);
        (date, time)
    }

    /// Decode FAT date/time (+ 10 ms increment in 0..=199).
    #[must_use]
    pub fn from_fat(date: u16, time: u16, tenms: u8) -> DateTime {
        let tenms = tenms.min(199);
        DateTime {
            year: 1980 + (date >> 9),
            month: ((date >> 5) & 0xF) as u8,
            day: (date & 0x1F) as u8,
            hour: (time >> 11) as u8,
            minute: ((time >> 5) & 0x3F) as u8,
            second: ((time & 0x1F) as u8) * 2 + tenms / 100,
            centis: tenms % 100,
        }
    }

    /// The 10 ms increment byte (0..=199) for the odd second and centis.
    #[must_use]
    pub fn tenms(&self) -> u8 {
        (self.second.min(59) % 2) * 100 + self.centis.min(99)
    }
}

/// Mount options.
#[derive(Clone, Copy, Debug, Default)]
pub struct MountOptions<'p> {
    /// Refuse every write.
    pub read_only: bool,
    /// If set, every mutating path operation must lie at or below this
    /// directory (e.g. `"/.orpheus"`); compared component-wise, ignoring case.
    pub write_prefix: Option<&'p str>,
    /// MBR partition to mount (0..=3). `None`: the first FAT/exFAT partition,
    /// or the whole device if sector 0 is a boot sector ("superfloppy").
    pub partition: Option<u8>,
}

/// Link value in a FAT.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Link {
    Free,
    Next(u32),
    End,
    Bad,
}

/// Geometry; all LBAs are absolute on the device.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Geo {
    pub kind: FsKind,
    pub base: u32,
    pub end: u32,
    pub fat_start: u32,
    pub fat_len: u32,
    pub root_start: u32,
    pub root_entries: u32,
    pub root_cluster: u32,
    pub data_start: u32,
    pub spc_shift: u8,
    pub cluster_count: u32,
    pub fsinfo: u32,
    pub bitmap_start: u32,
    pub bitmap_bytes: u32,
    pub serial: u32,
}

impl Geo {
    pub fn is_exfat(&self) -> bool {
        self.kind == FsKind::ExFat
    }
    pub fn cluster_bytes(&self) -> u32 {
        (SECTOR_SIZE as u32) << self.spc_shift
    }
    pub fn cbits(&self) -> u32 {
        9 + u32::from(self.spc_shift)
    }
    pub fn spc(&self) -> u32 {
        1 << self.spc_shift
    }
    pub fn valid_cluster(&self, c: u32) -> bool {
        c >= 2 && c - 2 < self.cluster_count
    }
    /// LBA of a cluster. Callers pass valid clusters; for anything else the
    /// result is a wrong LBA (never a panic), which the device rejects or the
    /// data checks catch.
    pub fn cluster_lba(&self, c: u32) -> u32 {
        self.data_start.wrapping_add(c.wrapping_sub(2) << self.spc_shift)
    }
    pub fn eoc(&self) -> u32 {
        match self.kind {
            FsKind::Fat12 => 0xFFF,
            FsKind::Fat16 => 0xFFFF,
            FsKind::Fat32 => 0x0FFF_FFFF,
            FsKind::ExFat => 0xFFFF_FFFF,
        }
    }
    fn classify(&self, v: u32) -> Link {
        let (bad, eoc_min) = match self.kind {
            FsKind::Fat12 => (0xFF7, 0xFF8),
            FsKind::Fat16 => (0xFFF7, 0xFFF8),
            FsKind::Fat32 => (0x0FFF_FFF7, 0x0FFF_FFF8),
            FsKind::ExFat => (0xFFFF_FFF7, 0xFFFF_FFF8),
        };
        if v == 0 {
            Link::Free
        } else if v >= eoc_min {
            Link::End
        } else if v == bad || !self.valid_cluster(v) {
            Link::Bad
        } else {
            Link::Next(v)
        }
    }
}

/// A mounted volume. Owns the block device and borrows the cache slots.
pub struct Volume<'c, D: BlockDevice> {
    pub(crate) dev: D,
    pub(crate) cache: Cache<'c>,
    pub(crate) g: Geo,
    pub(crate) read_only: bool,
    pub(crate) prefix: Option<&'c str>,
    pub(crate) now: DateTime,
    /// Free clusters, `u32::MAX` while unknown.
    pub(crate) free: u32,
    /// Next-free hint.
    pub(crate) next_free: u32,
    pub(crate) fsinfo_dirty: bool,
    /// The on-disk dirty flag has been set by us.
    pub(crate) marked_dirty: bool,
    /// exFAT: the volume's up-case table is the recommended one.
    pub(crate) upcase_std: bool,
    pub(crate) upcase_cluster: u32,
    pub(crate) upcase_len: u32,
}

pub(crate) fn le16(b: &[u8], o: usize) -> u16 {
    match b.get(o..o + 2) {
        Some(s) => u16::from_le_bytes([s[0], s[1]]),
        None => 0,
    }
}
pub(crate) fn le32(b: &[u8], o: usize) -> u32 {
    match b.get(o..o + 4) {
        Some(s) => u32::from_le_bytes([s[0], s[1], s[2], s[3]]),
        None => 0,
    }
}
pub(crate) fn le64(b: &[u8], o: usize) -> u64 {
    u64::from(le32(b, o)) | (u64::from(le32(b, o + 4)) << 32)
}
pub(crate) use {le16 as rd16, le32 as rd32, le64 as rd64};

fn looks_like_fat(s: &Sector) -> bool {
    let jump = s[0] == 0xEB || s[0] == 0xE9;
    let bps = le16(s, 11);
    let spc = s[13];
    jump && bps.is_power_of_two()
        && (512..=4096).contains(&bps)
        && spc != 0
        && spc.is_power_of_two()
        && le16(s, 14) != 0
        && s[16] != 0
}

fn looks_like_exfat(s: &Sector) -> bool {
    &s[3..11] == b"EXFAT   "
}

/// Zero sector used for zero-filling.
pub(crate) static ZERO: Sector = [0; SECTOR_SIZE];

impl<'c, D: BlockDevice> Volume<'c, D> {
    /// Mount the volume on `dev`, using `cache` (at least 2 slots) for
    /// metadata.
    pub async fn mount(
        mut dev: D,
        cache: &'c mut [CacheSlot],
        opts: MountOptions<'c>,
    ) -> Result<Self, D::Error> {
        if cache.len() < 2 {
            return Err(Error::BufferTooSmall);
        }
        let mut cache = Cache::new(cache);
        let dev_sectors = dev.sector_count();
        if dev_sectors == 0 {
            return Err(Error::NoFilesystem);
        }
        // Find the volume start.
        let i = cache.load(&mut dev, 0, Kind::Meta, false).await?;
        let s0 = *cache.data(i);
        let (base, len) = if opts.partition.is_none() && (looks_like_exfat(&s0) || looks_like_fat(&s0)) {
            (0u32, dev_sectors)
        } else {
            if s0[510] != 0x55 || s0[511] != 0xAA {
                return Err(Error::NoFilesystem);
            }
            let mut found = None;
            for p in 0..4usize {
                let e = &s0[446 + 16 * p..446 + 16 * p + 16];
                if let Some(want) = opts.partition
                    && usize::from(want) != p
                {
                    continue;
                }
                let ty = e[4];
                let start = le32(e, 8);
                let count = le32(e, 12);
                if ty == 0xEE {
                    return Err(Error::Unsupported); // GPT
                }
                let known = matches!(ty, 0x01 | 0x04 | 0x06 | 0x07 | 0x0B | 0x0C | 0x0E);
                if (known || opts.partition.is_some()) && ty != 0 && start != 0 && count != 0 {
                    found = Some((start, count));
                    break;
                }
            }
            let Some((start, count)) = found else {
                return Err(Error::NoFilesystem);
            };
            if start >= dev_sectors {
                return Err(Error::Corrupt);
            }
            (start, count.min(dev_sectors - start))
        };
        let i = cache.load(&mut dev, base, Kind::Meta, false).await?;
        let bs = *cache.data(i);
        let mut vol = Volume {
            dev,
            cache,
            g: Geo {
                kind: FsKind::Fat32,
                base,
                end: base + len,
                fat_start: 0,
                fat_len: 0,
                root_start: 0,
                root_entries: 0,
                root_cluster: 0,
                data_start: 0,
                spc_shift: 0,
                cluster_count: 0,
                fsinfo: 0,
                bitmap_start: 0,
                bitmap_bytes: 0,
                serial: 0,
            },
            read_only: opts.read_only,
            prefix: opts.write_prefix,
            now: DateTime::DEFAULT,
            free: u32::MAX,
            next_free: 2,
            fsinfo_dirty: false,
            marked_dirty: false,
            upcase_std: false,
            upcase_cluster: 0,
            upcase_len: 0,
        };
        if looks_like_exfat(&bs) {
            vol.mount_exfat(&bs, len).await?;
        } else if looks_like_fat(&bs) {
            vol.mount_fat(&bs, len).await?;
        } else {
            return Err(Error::NoFilesystem);
        }
        if let Some(p) = vol.prefix
            && crate::dir::components(p).any(|c| c == "..")
        {
            return Err(Error::InvalidName);
        }
        Ok(vol)
    }

    async fn mount_fat(&mut self, bs: &Sector, len: u32) -> Result<(), D::Error> {
        if bs[510] != 0x55 || bs[511] != 0xAA {
            return Err(Error::NoFilesystem);
        }
        if le16(bs, 11) != 512 {
            return Err(Error::Unsupported);
        }
        let spc = u32::from(bs[13]);
        let rsvd = u32::from(le16(bs, 14));
        let nfats = u32::from(bs[16]);
        let root_entries = u32::from(le16(bs, 17));
        let tot = if le16(bs, 19) != 0 { u32::from(le16(bs, 19)) } else { le32(bs, 32) };
        let fat16 = u32::from(le16(bs, 22));
        let fat_len = if fat16 != 0 { fat16 } else { le32(bs, 36) };
        if fat_len == 0 || tot == 0 || nfats == 0 || nfats > 4 {
            return Err(Error::Corrupt);
        }
        let tot = tot.min(len);
        let root_sectors = (root_entries * 32).div_ceil(512);
        let meta = u64::from(rsvd) + u64::from(nfats) * u64::from(fat_len) + u64::from(root_sectors);
        if meta >= u64::from(tot) {
            return Err(Error::Corrupt);
        }
        let meta = meta as u32;
        let clusters = (tot - meta) / spc;
        let kind = if clusters < 4085 {
            FsKind::Fat12
        } else if clusters < 65525 {
            FsKind::Fat16
        } else {
            FsKind::Fat32
        };
        if kind == FsKind::Fat32 && (root_entries != 0 || fat16 != 0) {
            return Err(Error::Corrupt);
        }
        if kind != FsKind::Fat32 && root_entries == 0 {
            return Err(Error::Corrupt);
        }
        // Clamp the cluster count to what the FAT can describe.
        let fat_bytes = u64::from(fat_len) * 512;
        let entries = match kind {
            FsKind::Fat12 => fat_bytes * 2 / 3,
            FsKind::Fat16 => fat_bytes / 2,
            _ => fat_bytes / 4,
        };
        let clusters = (u64::from(clusters)).min(entries.saturating_sub(2)) as u32;
        let base = self.g.base;
        let mut fat_start = base + rsvd;
        let mut copies = nfats as u8;
        if kind == FsKind::Fat32 {
            let ext = le16(bs, 40);
            if ext & 0x80 != 0 {
                // mirroring disabled: only the active FAT is used
                let active = u32::from(ext & 0xF);
                if active >= nfats {
                    return Err(Error::Corrupt);
                }
                fat_start += active * fat_len;
                copies = 1;
            }
        }
        self.g.kind = kind;
        self.g.fat_start = fat_start;
        self.g.fat_len = fat_len;
        self.g.root_start = base + rsvd + nfats * fat_len;
        self.g.root_entries = root_entries;
        self.g.data_start = base + meta;
        self.g.spc_shift = spc.trailing_zeros() as u8;
        self.g.cluster_count = clusters;
        self.g.serial = if kind == FsKind::Fat32 { le32(bs, 67) } else { le32(bs, 39) };
        if kind == FsKind::Fat32 {
            let rc = le32(bs, 44);
            if !self.g.valid_cluster(rc) {
                return Err(Error::Corrupt);
            }
            self.g.root_cluster = rc;
            let fsi = u32::from(le16(bs, 48));
            if fsi != 0 && fsi != 0xFFFF && fsi < rsvd {
                let lba = base + fsi;
                let i = self.cache.load(&mut self.dev, lba, Kind::Meta, false).await?;
                let s = self.cache.data(i);
                if le32(s, 0) == 0x4161_5252 && le32(s, 484) == 0x6141_7272 {
                    self.g.fsinfo = lba;
                    let free = le32(s, 488);
                    if free <= clusters {
                        self.free = free;
                    }
                    let nf = le32(s, 492);
                    if self.g.valid_cluster(nf) {
                        self.next_free = nf;
                    }
                }
            }
        }
        if u64::from(self.g.data_start) + (u64::from(clusters) << self.g.spc_shift) > u64::from(self.g.end) {
            return Err(Error::Corrupt);
        }
        self.cache.mirror = Mirror { start: fat_start, len: fat_len, copies };
        Ok(())
    }

    async fn mount_exfat(&mut self, bs: &Sector, len: u32) -> Result<(), D::Error> {
        if bs[510] != 0x55 || bs[511] != 0xAA || bs[..3] != [0xEB, 0x76, 0x90] {
            return Err(Error::NoFilesystem);
        }
        if bs[11..64].iter().any(|&b| b != 0) {
            return Err(Error::Corrupt);
        }
        if bs[108] != 9 {
            return Err(Error::Unsupported);
        }
        let spc_shift = bs[109];
        if spc_shift > 16 {
            return Err(Error::Corrupt);
        }
        let vol_len = le64(bs, 72);
        let fat_off = le32(bs, 80);
        let fat_len = le32(bs, 84);
        let heap = le32(bs, 88);
        let count = le32(bs, 92);
        let root = le32(bs, 96);
        let nfats = bs[110];
        if nfats != 1 && nfats != 2 {
            return Err(Error::Corrupt);
        }
        let flags = le16(bs, 106);
        if vol_len > u64::from(len) || fat_off < 24 || heap < fat_off || count == 0 {
            return Err(Error::Corrupt);
        }
        if u64::from(fat_len) * 128 < u64::from(count) + 2 {
            return Err(Error::Corrupt);
        }
        if u64::from(heap) + (u64::from(count) << spc_shift) > vol_len {
            return Err(Error::Corrupt);
        }
        // Boot checksum over sectors 0..=10, compared with sector 11.
        let base = self.g.base;
        let mut sum: u32 = 0;
        for k in 0..11u32 {
            let i = self.cache.load(&mut self.dev, base + k, Kind::Meta, false).await?;
            let s = self.cache.data(i);
            for (j, &b) in s.iter().enumerate() {
                if k == 0 && (j == 106 || j == 107 || j == 112) {
                    continue;
                }
                sum = sum.rotate_right(1).wrapping_add(u32::from(b));
            }
        }
        let i = self.cache.load(&mut self.dev, base + 11, Kind::Meta, false).await?;
        if le32(self.cache.data(i), 0) != sum {
            return Err(Error::Corrupt);
        }
        let active = if nfats == 2 { u32::from(flags & 1) } else { 0 };
        if nfats == 2 {
            // TexFAT layout: mount, but never write
            self.read_only = true;
        }
        self.g.kind = FsKind::ExFat;
        self.g.fat_start = base + fat_off + active * fat_len;
        self.g.fat_len = fat_len;
        self.g.data_start = base + heap;
        self.g.spc_shift = spc_shift;
        self.g.cluster_count = count;
        self.g.serial = le32(bs, 100);
        if !self.g.valid_cluster(root) {
            return Err(Error::Corrupt);
        }
        self.g.root_cluster = root;
        self.cache.mirror = Mirror::default();
        // Scan the root directory for the bitmap and up-case table.
        let mut cur = self.root();
        let mut bitmap = None;
        let mut upcase = None;
        let mut steps = 0u32;
        while let Some(e) = self.dir_raw_next(&mut cur.spec, &mut cur.pos).await? {
            steps += 1;
            if steps > 4096 {
                break;
            }
            match e[0] {
                0x00 => break,
                0x81 if bitmap.is_none() && e[1] & 1 == 0 => {
                    bitmap = Some((le32(&e, 20), le64(&e, 24)));
                }
                0x82 if upcase.is_none() => {
                    upcase = Some((le32(&e, 4), le32(&e, 20), le64(&e, 24)));
                }
                _ => {}
            }
        }
        let Some((bc, blen)) = bitmap else {
            return Err(Error::Corrupt);
        };
        if !self.g.valid_cluster(bc) || blen < u64::from(count).div_ceil(8) || blen > u64::from(u32::MAX) {
            return Err(Error::Corrupt);
        }
        // The bitmap must be contiguous (it always is when formatted).
        let nclus = blen.div_ceil(u64::from(self.g.cluster_bytes())) as u32;
        if !self.g.valid_cluster(bc + nclus - 1) {
            return Err(Error::Corrupt);
        }
        for k in 0..nclus.saturating_sub(1) {
            // A FAT chain, if present, must be the contiguous one.
            if let Link::Next(n) = self.fat_get(bc + k).await?
                && n != bc + k + 1
            {
                return Err(Error::Unsupported);
            }
        }
        self.g.bitmap_start = self.g.cluster_lba(bc);
        self.g.bitmap_bytes = u64::from(count).div_ceil(8) as u32;
        if let Some((sum, uc, ulen)) = upcase {
            self.upcase_std =
                sum == crate::tables::STD_UPCASE_CHECKSUM && ulen == crate::tables::STD_UPCASE_LEN;
            if self.g.valid_cluster(uc) {
                self.upcase_cluster = uc;
                self.upcase_len = ulen.min(0x2_0000) as u32;
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------- info

    /// File-system type.
    pub fn kind(&self) -> FsKind {
        self.g.kind
    }
    /// Bytes per cluster.
    pub fn cluster_size(&self) -> u32 {
        self.g.cluster_bytes()
    }
    /// Number of data clusters.
    pub fn cluster_count(&self) -> u32 {
        self.g.cluster_count
    }
    /// Volume serial number.
    pub fn serial(&self) -> u32 {
        self.g.serial
    }
    /// First sector of the volume on the device.
    pub fn base_lba(&self) -> u32 {
        self.g.base
    }
    /// Change the write guard of a mounted volume (`None`: no guard). For
    /// one-off maintenance such as moving the firmware's own folder; the
    /// caller restores it right after.
    pub fn set_write_prefix(&mut self, prefix: Option<&'c str>) {
        self.prefix = prefix;
    }
    /// True if the volume refuses writes.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }
    /// Set the time used for new timestamps.
    pub fn set_time(&mut self, t: DateTime) {
        self.now = t;
    }
    /// Metadata cache statistics: (sector reads, sector writes).
    pub fn cache_stats(&self) -> (u32, u32) {
        (self.cache.reads, self.cache.writes)
    }
    /// Cached sectors not yet written back.
    pub fn dirty_sectors(&self) -> usize {
        self.cache.dirty_count()
    }
    /// Access the device (for raw-LBA work through [`Volume::extents`]).
    /// Call [`Volume::flush`] before and [`Volume::invalidate_cached`] after
    /// writing sectors the cache may hold.
    pub fn device(&mut self) -> &mut D {
        &mut self.dev
    }
    /// Drop cached copies of `[lba, lba + sectors)` (dirty ones too: flush
    /// first). Call after writing file data through raw LBAs.
    pub fn invalidate_cached(&mut self, lba: u32, sectors: u32) {
        self.cache.invalidate_range(lba, sectors);
    }
    /// Give the device back without flushing.
    pub fn into_device(self) -> D {
        self.dev
    }

    /// Flush everything and give the device back.
    pub async fn unmount(mut self) -> (D, Result<(), D::Error>) {
        let r = self.flush().await;
        (self.dev, r)
    }

    /// Number of free clusters (counted on first call unless FSINFO knows).
    pub async fn free_clusters(&mut self) -> Result<u32, D::Error> {
        if self.free == u32::MAX {
            self.free = self.count_free().await?;
        }
        Ok(self.free)
    }

    /// Free bytes.
    pub async fn free_bytes(&mut self) -> Result<u64, D::Error> {
        Ok(u64::from(self.free_clusters().await?) << self.g.cbits())
    }

    // ---------------------------------------------------------------- FAT

    pub(crate) async fn fat_get(&mut self, c: u32) -> Result<Link, D::Error> {
        if !self.g.valid_cluster(c) {
            return Err(Error::Corrupt);
        }
        let v = match self.g.kind {
            FsKind::Fat12 => {
                let off = c + c / 2;
                let lba = self.g.fat_start + off / 512;
                let o = (off % 512) as usize;
                let i = self.cache.load(&mut self.dev, lba, Kind::Alloc, false).await?;
                let lo = self.cache.data(i)[o];
                let hi = if o == 511 {
                    let j = self.cache.load(&mut self.dev, lba + 1, Kind::Alloc, false).await?;
                    self.cache.data(j)[0]
                } else {
                    self.cache.data(i)[o + 1]
                };
                let v = u32::from(u16::from_le_bytes([lo, hi]));
                if c & 1 == 1 { v >> 4 } else { v & 0xFFF }
            }
            FsKind::Fat16 => {
                let off = c * 2;
                let i =
                    self.cache.load(&mut self.dev, self.g.fat_start + off / 512, Kind::Alloc, false).await?;
                u32::from(le16(self.cache.data(i), (off % 512) as usize))
            }
            FsKind::Fat32 | FsKind::ExFat => {
                let off = u64::from(c) * 4;
                let lba = self.g.fat_start + (off / 512) as u32;
                let i = self.cache.load(&mut self.dev, lba, Kind::Alloc, false).await?;
                let v = le32(self.cache.data(i), (off % 512) as usize);
                if self.g.kind == FsKind::Fat32 { v & 0x0FFF_FFFF } else { v }
            }
        };
        Ok(self.g.classify(v))
    }

    pub(crate) async fn fat_set(&mut self, c: u32, v: u32) -> Result<(), D::Error> {
        if !self.g.valid_cluster(c) {
            return Err(Error::Corrupt);
        }
        match self.g.kind {
            FsKind::Fat12 => {
                let off = c + c / 2;
                let lba = self.g.fat_start + off / 512;
                let o = (off % 512) as usize;
                let v = (v & 0xFFF) as u16;
                let i = self.cache.load(&mut self.dev, lba, Kind::Alloc, false).await?;
                let lo = self.cache.data(i)[o];
                let (nlo, hi_mask, hi_val) = if c & 1 == 1 {
                    ((lo & 0x0F) | ((v << 4) as u8), 0x00u8, (v >> 4) as u8)
                } else {
                    (v as u8, 0xF0u8, ((v >> 8) as u8) & 0x0F)
                };
                self.cache.data_mut(i)[o] = nlo;
                if o == 511 {
                    let j = self.cache.load(&mut self.dev, lba + 1, Kind::Alloc, false).await?;
                    let h = self.cache.data(j)[0];
                    self.cache.data_mut(j)[0] = (h & hi_mask) | hi_val;
                } else {
                    let h = self.cache.data(i)[o + 1];
                    self.cache.data_mut(i)[o + 1] = (h & hi_mask) | hi_val;
                }
            }
            FsKind::Fat16 => {
                let off = c * 2;
                let i =
                    self.cache.load(&mut self.dev, self.g.fat_start + off / 512, Kind::Alloc, false).await?;
                let o = (off % 512) as usize;
                self.cache.data_mut(i)[o..o + 2].copy_from_slice(&(v as u16).to_le_bytes());
            }
            FsKind::Fat32 | FsKind::ExFat => {
                let off = u64::from(c) * 4;
                let lba = self.g.fat_start + (off / 512) as u32;
                let o = (off % 512) as usize;
                let i = self.cache.load(&mut self.dev, lba, Kind::Alloc, false).await?;
                let v = if self.g.kind == FsKind::Fat32 {
                    (le32(self.cache.data(i), o) & 0xF000_0000) | (v & 0x0FFF_FFFF)
                } else {
                    v
                };
                self.cache.data_mut(i)[o..o + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        Ok(())
    }

    /// Next cluster of a chain (`contig`: exFAT NoFatChain, the next is c+1
    /// while `remaining` > 1).
    pub(crate) async fn next_cluster(&mut self, c: u32, contig: bool) -> Result<Option<u32>, D::Error> {
        if contig {
            let n = c + 1;
            return Ok(if self.g.valid_cluster(n) { Some(n) } else { None });
        }
        match self.fat_get(c).await? {
            Link::Next(n) => Ok(Some(n)),
            Link::End => Ok(None),
            Link::Free | Link::Bad => Err(Error::Corrupt),
        }
    }

    // ------------------------------------------------------- bitmap (exFAT)

    async fn bitmap_get(&mut self, c: u32) -> Result<bool, D::Error> {
        let bit = c - 2;
        let lba = self.g.bitmap_start + bit / 4096;
        let i = self.cache.load(&mut self.dev, lba, Kind::Alloc, false).await?;
        Ok(self.cache.data(i)[((bit / 8) % 512) as usize] & (1 << (bit % 8)) != 0)
    }

    async fn bitmap_set(&mut self, c: u32, used: bool) -> Result<(), D::Error> {
        let bit = c - 2;
        let lba = self.g.bitmap_start + bit / 4096;
        let i = self.cache.load(&mut self.dev, lba, Kind::Alloc, false).await?;
        let b = &mut self.cache.data_mut(i)[((bit / 8) % 512) as usize];
        if used {
            *b |= 1 << (bit % 8);
        } else {
            *b &= !(1 << (bit % 8));
        }
        Ok(())
    }

    // ----------------------------------------------------------- allocation

    pub(crate) async fn is_free(&mut self, c: u32) -> Result<bool, D::Error> {
        if !self.g.valid_cluster(c) {
            return Ok(false);
        }
        if self.g.is_exfat() {
            Ok(!self.bitmap_get(c).await?)
        } else {
            Ok(self.fat_get(c).await? == Link::Free)
        }
    }

    /// First free cluster in `[from, to)`, scanning whole sectors at a time.
    async fn scan_free(&mut self, from: u32, to: u32) -> Result<Option<u32>, D::Error> {
        let mut c = from;
        while c < to {
            match self.g.kind {
                FsKind::ExFat => {
                    let bit = c - 2;
                    let lba = self.g.bitmap_start + bit / 4096;
                    let i = self.cache.load(&mut self.dev, lba, Kind::Alloc, false).await?;
                    let s = self.cache.data(i);
                    let sector_end = (c - (bit % 4096)) + 4096;
                    while c < to && c < sector_end {
                        let b = c - 2;
                        let byte = s[((b / 8) % 512) as usize];
                        if byte == 0xFF && b.is_multiple_of(8) {
                            c += 8;
                            continue;
                        }
                        if byte & (1 << (b % 8)) == 0 {
                            return Ok(Some(c));
                        }
                        c += 1;
                    }
                }
                FsKind::Fat16 | FsKind::Fat32 => {
                    let w = if self.g.kind == FsKind::Fat16 { 2u32 } else { 4 };
                    let per = 512 / w;
                    let lba = self.g.fat_start + c / per;
                    let i = self.cache.load(&mut self.dev, lba, Kind::Alloc, false).await?;
                    let s = self.cache.data(i);
                    let sector_end = c - (c % per) + per;
                    while c < to && c < sector_end {
                        let o = ((c % per) * w) as usize;
                        let v = if w == 2 { u32::from(le16(s, o)) } else { le32(s, o) & 0x0FFF_FFFF };
                        if v == 0 {
                            return Ok(Some(c));
                        }
                        c += 1;
                    }
                }
                FsKind::Fat12 => {
                    if self.fat_get(c).await? == Link::Free {
                        return Ok(Some(c));
                    }
                    c += 1;
                }
            }
        }
        Ok(None)
    }

    async fn count_free(&mut self) -> Result<u32, D::Error> {
        let end = self.g.cluster_count + 2;
        let mut n = 0u32;
        if self.g.is_exfat() {
            let bytes = self.g.bitmap_bytes;
            let mut k = 0u32;
            while k < bytes {
                let i =
                    self.cache.load(&mut self.dev, self.g.bitmap_start + k / 512, Kind::Alloc, false).await?;
                let s = self.cache.data(i);
                let stop = (k - k % 512 + 512).min(bytes);
                while k < stop {
                    let byte = s[(k % 512) as usize];
                    for b in 0..8 {
                        let c = 2 + k * 8 + b;
                        if c < end && byte & (1 << b) == 0 {
                            n += 1;
                        }
                    }
                    k += 1;
                }
            }
            return Ok(n);
        }
        let mut c = 2;
        while c < end {
            match self.scan_free(c, end).await? {
                Some(f) => {
                    n += 1;
                    c = f + 1;
                }
                None => break,
            }
        }
        Ok(n)
    }

    /// Allocate one cluster, preferring `near` (then the next-free hint).
    /// FAT: the entry becomes end-of-chain. exFAT: the bitmap bit is set and,
    /// with `chain`, the FAT entry becomes end-of-chain.
    pub(crate) async fn alloc_cluster(&mut self, near: u32, chain: bool) -> Result<u32, D::Error> {
        let end = self.g.cluster_count + 2;
        let mut found = None;
        if self.g.valid_cluster(near) && self.is_free(near).await? {
            found = Some(near);
        }
        if found.is_none() {
            let hint = if self.g.valid_cluster(self.next_free) { self.next_free } else { 2 };
            found = self.scan_free(hint, end).await?;
            if found.is_none() {
                found = self.scan_free(2, hint).await?;
            }
        }
        let Some(c) = found else {
            self.free = 0;
            return Err(Error::NoSpace);
        };
        if self.g.is_exfat() {
            self.bitmap_set(c, true).await?;
            if chain {
                self.fat_set(c, self.g.eoc()).await?;
            }
        } else {
            self.fat_set(c, self.g.eoc()).await?;
        }
        if self.free != u32::MAX {
            self.free = self.free.saturating_sub(1);
        }
        self.next_free = if c + 1 < end { c + 1 } else { 2 };
        self.fsinfo_dirty = true;
        Ok(c)
    }

    /// Release one cluster.
    pub(crate) async fn release_cluster(&mut self, c: u32) -> Result<(), D::Error> {
        if self.g.is_exfat() {
            self.bitmap_set(c, false).await?;
        } else {
            self.fat_set(c, 0).await?;
        }
        if self.free != u32::MAX {
            self.free = (self.free + 1).min(self.g.cluster_count);
        }
        self.fsinfo_dirty = true;
        Ok(())
    }

    /// Free a chain starting at `first`. `contig_len`: exFAT NoFatChain run
    /// length; otherwise the FAT chain is followed.
    pub(crate) async fn free_chain(&mut self, first: u32, contig_len: Option<u32>) -> Result<(), D::Error> {
        if !self.g.valid_cluster(first) {
            return Ok(());
        }
        if let Some(n) = contig_len {
            for k in 0..n {
                let c = first.wrapping_add(k);
                if !self.g.valid_cluster(c) {
                    return Err(Error::Corrupt);
                }
                self.release_cluster(c).await?;
            }
            return Ok(());
        }
        let mut c = first;
        let mut steps = 0u32;
        loop {
            let next = self.fat_get(c).await?;
            self.release_cluster(c).await?;
            steps += 1;
            if steps > self.g.cluster_count {
                return Err(Error::Corrupt);
            }
            match next {
                Link::Next(n) => c = n,
                Link::End => return Ok(()),
                Link::Free | Link::Bad => return Err(Error::Corrupt),
            }
        }
    }

    /// Zero a cluster on the device (direct writes).
    pub(crate) async fn zero_cluster(&mut self, c: u32, skip_first: bool) -> Result<(), D::Error> {
        let lba = self.g.cluster_lba(c);
        let n = self.g.spc();
        self.cache.invalidate_range(lba + u32::from(skip_first), n - u32::from(skip_first));
        for k in u32::from(skip_first)..n {
            self.dev.write(lba + k, core::slice::from_ref(&ZERO)).await.map_err(Error::Io)?;
        }
        Ok(())
    }

    // ------------------------------------------------------ dirty / flush

    /// Called before the first change: checks permissions and sets the
    /// on-disk dirty flag (written through at once).
    pub(crate) async fn begin_write(&mut self) -> Result<(), D::Error> {
        if self.read_only {
            return Err(Error::ReadOnly);
        }
        if self.marked_dirty {
            return Ok(());
        }
        self.set_dirty_flag(true).await?;
        self.marked_dirty = true;
        Ok(())
    }

    async fn set_dirty_flag(&mut self, dirty: bool) -> Result<(), D::Error> {
        match self.g.kind {
            FsKind::ExFat => {
                let lba = self.g.base;
                let i = self.cache.load(&mut self.dev, lba, Kind::Meta, false).await?;
                let s = self.cache.data_mut(i);
                if dirty {
                    s[106] |= 2;
                } else {
                    s[106] &= !2;
                }
                self.cache.flush_range(&mut self.dev, lba, 1).await?;
            }
            FsKind::Fat16 | FsKind::Fat32 => {
                // FAT[1] clean-shutdown bit
                let (lba, o, bit) = if self.g.kind == FsKind::Fat16 {
                    (self.g.fat_start, 3usize, 0x80u8)
                } else {
                    (self.g.fat_start, 7usize, 0x08u8)
                };
                let i = self.cache.load(&mut self.dev, lba, Kind::Alloc, false).await?;
                let s = self.cache.data_mut(i);
                if dirty {
                    s[o] &= !bit;
                } else {
                    s[o] |= bit;
                }
                self.cache.flush_range(&mut self.dev, lba, 1).await?;
            }
            FsKind::Fat12 => {}
        }
        self.dev.sync().await.map_err(Error::Io)
    }

    /// Write back every dirty sector, FSINFO, clear the dirty flag and sync.
    /// Open files must be flushed first ([`Volume::flush_file`]) for their
    /// directory entries to be current.
    pub async fn flush(&mut self) -> Result<(), D::Error> {
        if self.read_only {
            return Ok(());
        }
        if self.fsinfo_dirty && self.g.fsinfo != 0 {
            let lba = self.g.fsinfo;
            let free = self.free;
            let nf = self.next_free;
            let i = self.cache.load(&mut self.dev, lba, Kind::Meta, false).await?;
            let s = self.cache.data_mut(i);
            s[488..492].copy_from_slice(&free.to_le_bytes());
            s[492..496].copy_from_slice(&nf.to_le_bytes());
        }
        self.fsinfo_dirty = false;
        self.cache.flush(&mut self.dev).await?;
        self.dev.sync().await.map_err(Error::Io)?;
        if self.marked_dirty {
            self.set_dirty_flag(false).await?;
            self.marked_dirty = false;
        }
        Ok(())
    }

    /// Check the write prefix for a path.
    pub(crate) fn check_prefix(&self, path: &str) -> Result<(), D::Error> {
        if crate::dir::components(path).any(|c| c == "..") {
            return Err(Error::InvalidName);
        }
        let Some(p) = self.prefix else { return Ok(()) };
        let mut a = crate::dir::components(path);
        for want in crate::dir::components(p) {
            match a.next() {
                Some(have) if crate::unicode::eq_ignore_case(have, want) => {}
                _ => return Err(Error::OutsidePrefix),
            }
        }
        Ok(())
    }

    /// Up-case one unit using the volume's table (exFAT name hash).
    pub(crate) async fn vol_upcase(&mut self, u: u16) -> Result<u16, D::Error> {
        if self.upcase_std || u < 0x80 || self.upcase_cluster == 0 {
            return Ok(crate::unicode::upcase16(u));
        }
        // Slow path for a non-standard table: decompress until `u`.
        let mut cp: u32 = 0;
        let mut pos: u32 = 0;
        let mut skip_next = false;
        let n = self.upcase_len / 2;
        let mut c = self.upcase_cluster;
        let cb = self.g.cluster_bytes();
        while pos < n {
            let byte = pos * 2;
            if byte > 0 && byte.is_multiple_of(cb) {
                c = match self.next_cluster(c, false).await {
                    Ok(Some(x)) => x,
                    _ if self.g.valid_cluster(c.wrapping_add(1)) => c + 1,
                    _ => return Ok(u),
                };
            }
            let lba = self.g.cluster_lba(c) + (byte % cb) / 512;
            let i = self.cache.load(&mut self.dev, lba, Kind::Meta, false).await?;
            let v = le16(self.cache.data(i), (byte % 512) as usize);
            pos += 1;
            if skip_next {
                cp += u32::from(v);
                skip_next = false;
            } else if v == 0xFFFF {
                skip_next = true;
            } else {
                if cp == u32::from(u) {
                    return Ok(v);
                }
                cp += 1;
            }
            if cp > u32::from(u) {
                break;
            }
        }
        Ok(u)
    }
}
