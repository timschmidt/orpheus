//! Thumbnail cache files: small fixed-size RGB565 pictures kept on the card
//! so lists of pictures and album art draw without decoding (04 §4.4, §7;
//! 05 §7.6).
//!
//! Thumbnails are grouped into bucket files so the card does not fill with
//! tiny files (each FAT file costs a whole cluster). A key picks its bucket
//! by hash; a bucket holds a fixed number of slots and replaces the oldest
//! slot (round robin) when full. Everything is disposable: a bucket that
//! does not parse is rewritten from scratch.
//!
//! ## Bucket file format (`YTC1`, little-endian)
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 4 | magic `YTC1` |
//! | 4 | 1, 1 | thumbnail width, height |
//! | 6 | 2 | slots `N` |
//! | 8 | 2 | next slot to replace |
//! | 10 | 18 | reserved (0) |
//! | 28 | 4 | CRC-32 of bytes 0–27 |
//! | 32 | `48 × N` | index entries |
//! | `32 + 48N` | `2wh × N` | pixel slots, RGB565 little-endian, rows top first |
//!
//! Index entry (48 bytes): the 36-byte [`ThumbKey`], original width and
//! height (u16 each, saturated), flags (bit 7 used, bit 0 approximate
//! decode), 3 reserved bytes, CRC-32 of the slot's pixel bytes. A slot is
//! written before its entry, so a write cut short leaves a CRC mismatch,
//! which reads as a miss.
//!
//! The key is the file's locator (04 §6.3, 16 bytes), its size and
//! modification time (a changed file misses), the part of the file (byte
//! offset of an embedded picture, 0 for a picture file), the thumbnail
//! size and the user rotation.

/// One cache kind: where its buckets live and the thumbnail geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheKind {
    /// Directory on the card (inside `/.orpheus/`).
    pub dir: &'static str,
    /// File name prefix letter.
    pub prefix: u8,
    pub w: u8,
    pub h: u8,
    pub buckets: u16,
    pub slots: u16,
}

/// Picture browser cells (05 §7.6: 3 × 3 grid of 40 × 30 cells).
pub const THUMBS: CacheKind =
    CacheKind { dir: "/.orpheus/cache/thumbs", prefix: b'T', w: 40, h: 30, buckets: 64, slots: 64 };
/// Album art in Now Playing's Standard view (05 §7.1; the UI draws it at
/// 52 × 52, `orpheus_apps::services::ArtSize::Small`).
pub const ART: CacheKind =
    CacheKind { dir: "/.orpheus/cache/art", prefix: b'A', w: 52, h: 52, buckets: 64, slots: 32 };
/// Album art for Now Playing's Big art view (full screen, 128 × 128):
/// at most 256 KiB per bucket file.
pub const ART_BIG: CacheKind =
    CacheKind { dir: "/.orpheus/cache/art", prefix: b'B', w: 128, h: 128, buckets: 32, slots: 8 };
/// The wallpaper (05 §7.6: one 128 × 128 picture).
pub const WALLPAPER: CacheKind =
    CacheKind { dir: "/.orpheus/cache", prefix: b'W', w: 128, h: 128, buckets: 1, slots: 1 };

const MAGIC: &[u8; 4] = b"YTC1";
const HEADER: u32 = 32;
const ENTRY: u32 = 48;
const KEY_LEN: usize = 36;
const USED: u8 = 0x80;

/// A key's slot and index entry if present, and the first free slot.
type Found = (Option<(u16, [u8; ENTRY as usize])>, Option<u16>);

/// What identifies a thumbnail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ThumbKey {
    /// File locator (volume serial, parent cluster, entry position, name
    /// hash).
    pub locator: [u8; 16],
    pub file_size: u64,
    /// FAT/exFAT modification timestamp.
    pub mtime: u32,
    /// Offset of an embedded picture in the file, 0 for a picture file.
    pub part: u32,
    pub w: u8,
    pub h: u8,
    /// User rotation as an EXIF orientation value (1 = none).
    pub rotation: u8,
    pub reserved: u8,
}

impl ThumbKey {
    #[must_use]
    pub fn to_bytes(&self) -> [u8; KEY_LEN] {
        let mut b = [0u8; KEY_LEN];
        b[..16].copy_from_slice(&self.locator);
        b[16..24].copy_from_slice(&self.file_size.to_le_bytes());
        b[24..28].copy_from_slice(&self.mtime.to_le_bytes());
        b[28..32].copy_from_slice(&self.part.to_le_bytes());
        b[32] = self.w;
        b[33] = self.h;
        b[34] = self.rotation;
        b[35] = self.reserved;
        b
    }
}

/// What is stored with a thumbnail besides its pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ThumbMeta {
    /// Size of the original picture (saturated to 65535).
    pub orig_w: u16,
    pub orig_h: u16,
    /// Decoded at reduced detail (progressive DC only and the like).
    pub approximate: bool,
}

/// A bucket file, opened (and created if missing) by the caller.
pub trait CacheFile {
    type Error;
    /// Read at `off`; returns the bytes read (fewer at the end of the file).
    fn read_at(&mut self, off: u32, buf: &mut [u8]) -> Result<usize, Self::Error>;
    /// Write at `off`, extending the file as needed.
    fn write_at(&mut self, off: u32, data: &[u8]) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheError<E> {
    Io(E),
    /// The pixel buffer does not have `w × h` pixels.
    Size,
}

/// CRC-32 (IEEE, reflected), nibble table.
#[must_use]
pub fn crc32(mut crc: u32, data: &[u8]) -> u32 {
    const T: [u32; 16] = [
        0x0000_0000,
        0x1DB7_1064,
        0x3B6E_20C8,
        0x26D9_30AC,
        0x76DC_4190,
        0x6B6B_51F4,
        0x4DB2_6158,
        0x5005_713C,
        0xEDB8_8320,
        0xF00F_9344,
        0xD6D6_A3E8,
        0xCB61_B38C,
        0x9B64_C2B0,
        0x86D3_D2D4,
        0xA00A_E278,
        0xBDBD_F21C,
    ];
    crc = !crc;
    for &b in data {
        crc = (crc >> 4) ^ T[((crc ^ u32::from(b)) & 15) as usize];
        crc = (crc >> 4) ^ T[((crc ^ (u32::from(b) >> 4)) & 15) as usize];
    }
    !crc
}

fn meta_of(e: &[u8; ENTRY as usize]) -> ThumbMeta {
    ThumbMeta {
        orig_w: u16::from_le_bytes([e[36], e[37]]),
        orig_h: u16::from_le_bytes([e[38], e[39]]),
        approximate: e[40] & 1 != 0,
    }
}

impl CacheKind {
    fn slot_bytes(&self) -> u32 {
        2 * u32::from(self.w) * u32::from(self.h)
    }

    fn slot_off(&self, i: u16) -> u32 {
        HEADER + ENTRY * u32::from(self.slots) + u32::from(i) * self.slot_bytes()
    }

    /// The bucket a key lives in.
    #[must_use]
    pub fn bucket(&self, key: &ThumbKey) -> u16 {
        // FNV-1a.
        let mut h: u32 = 0x811C_9DC5;
        for b in key.to_bytes() {
            h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
        }
        (h % u32::from(self.buckets.max(1))) as u16
    }

    /// The bucket's file name (8.3: `T03F.YTC`), inside [`CacheKind::dir`].
    #[must_use]
    pub fn file_name(&self, bucket: u16) -> [u8; 8] {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let mut n = *b"X000.YTC";
        n[0] = self.prefix;
        n[1] = HEX[usize::from((bucket >> 8) & 15)];
        n[2] = HEX[usize::from((bucket >> 4) & 15)];
        n[3] = HEX[usize::from(bucket & 15)];
        n
    }

    fn header(&self, next: u16) -> [u8; HEADER as usize] {
        let mut h = [0u8; HEADER as usize];
        h[..4].copy_from_slice(MAGIC);
        h[4] = self.w;
        h[5] = self.h;
        h[6..8].copy_from_slice(&self.slots.to_le_bytes());
        h[8..10].copy_from_slice(&next.to_le_bytes());
        let c = crc32(0, &h[..28]);
        h[28..].copy_from_slice(&c.to_le_bytes());
        h
    }

    /// The header's replacement cursor if the file is a valid bucket of
    /// this kind.
    fn read_header<F: CacheFile>(&self, f: &mut F) -> Result<Option<u16>, CacheError<F::Error>> {
        let mut h = [0u8; HEADER as usize];
        let n = f.read_at(0, &mut h).map_err(CacheError::Io)?;
        if n < h.len() || &h[..4] != MAGIC || h[4] != self.w || h[5] != self.h {
            return Ok(None);
        }
        if u16::from_le_bytes([h[6], h[7]]) != self.slots
            || crc32(0, &h[..28]) != u32::from_le_bytes([h[28], h[29], h[30], h[31]])
        {
            return Ok(None);
        }
        Ok(Some(u16::from_le_bytes([h[8], h[9]]) % self.slots.max(1)))
    }

    /// Find `key`: its slot and entry, and the first free slot.
    fn find<F: CacheFile>(&self, f: &mut F, key: &[u8; KEY_LEN]) -> Result<Found, CacheError<F::Error>> {
        let mut buf = [0u8; ENTRY as usize * 8];
        let mut free = None;
        let mut i = 0u16;
        while i < self.slots {
            let k = (self.slots - i).min(8);
            let len = usize::from(k) * ENTRY as usize;
            let off = HEADER + ENTRY * u32::from(i);
            let n = f.read_at(off, &mut buf[..len]).map_err(CacheError::Io)?;
            for j in 0..k {
                let e = &buf[usize::from(j) * ENTRY as usize..(usize::from(j) + 1) * ENTRY as usize];
                let whole = n >= (usize::from(j) + 1) * ENTRY as usize;
                if whole && e[40] & USED != 0 && e[..KEY_LEN] == key[..] {
                    let mut out = [0u8; ENTRY as usize];
                    out.copy_from_slice(e);
                    return Ok((Some((i + j, out)), free));
                }
                if free.is_none() && (!whole || e[40] & USED == 0) {
                    free = Some(i + j);
                }
            }
            i += k;
        }
        Ok((None, free))
    }

    /// Look `key` up in a bucket file; on a hit `out` (`w × h` pixels)
    /// holds the thumbnail.
    pub fn lookup<F: CacheFile>(
        &self,
        f: &mut F,
        key: &ThumbKey,
        out: &mut [u16],
    ) -> Result<Option<ThumbMeta>, CacheError<F::Error>> {
        if out.len() != usize::from(self.w) * usize::from(self.h) {
            return Err(CacheError::Size);
        }
        self.lookup_into(f, key, out, usize::from(self.w))
    }

    /// [`CacheKind::lookup`] into a box of a larger picture: row `y` of the
    /// thumbnail goes to `out[y * stride..][..w]` (a frame-buffer region).
    /// On a miss or a CRC mismatch the box may hold some of the bytes read.
    pub fn lookup_into<F: CacheFile>(
        &self,
        f: &mut F,
        key: &ThumbKey,
        out: &mut [u16],
        stride: usize,
    ) -> Result<Option<ThumbMeta>, CacheError<F::Error>> {
        let (w, h) = (usize::from(self.w), usize::from(self.h));
        if stride < w || h == 0 || out.len() < (h - 1) * stride + w {
            return Err(CacheError::Size);
        }
        if self.read_header(f)?.is_none() {
            return Ok(None);
        }
        let (hit, _) = self.find(f, &key.to_bytes())?;
        let Some((slot, e)) = hit else { return Ok(None) };
        let mut crc = 0;
        let mut buf = [0u8; 256];
        let mut off = self.slot_off(slot);
        let total = w * h;
        let mut px = 0usize;
        while px < total {
            let k = (total - px).min(buf.len() / 2);
            let n = f.read_at(off, &mut buf[..2 * k]).map_err(CacheError::Io)?;
            if n < 2 * k {
                return Ok(None);
            }
            crc = crc32(crc, &buf[..2 * k]);
            for i in 0..k {
                let p = px + i;
                out[p / w * stride + p % w] = u16::from_le_bytes([buf[2 * i], buf[2 * i + 1]]);
            }
            px += k;
            off += 2 * k as u32;
        }
        if crc != u32::from_le_bytes([e[44], e[45], e[46], e[47]]) {
            return Ok(None);
        }
        Ok(Some(meta_of(&e)))
    }

    /// Whether `key` is in a bucket file and what was stored with it,
    /// without reading the pixels (`lookup` checks their CRC). Album art
    /// uses it at track open; an entry with a 0 × 0 original records that
    /// there is no picture to find.
    pub fn meta<F: CacheFile>(
        &self,
        f: &mut F,
        key: &ThumbKey,
    ) -> Result<Option<ThumbMeta>, CacheError<F::Error>> {
        if self.read_header(f)?.is_none() {
            return Ok(None);
        }
        let (hit, _) = self.find(f, &key.to_bytes())?;
        Ok(hit.map(|(_, e)| meta_of(&e)))
    }

    /// Store a thumbnail (`w × h` pixels) in a bucket file, replacing an
    /// entry with the same key, else using a free slot, else the oldest.
    pub fn store<F: CacheFile>(
        &self,
        f: &mut F,
        key: &ThumbKey,
        meta: &ThumbMeta,
        px: &[u16],
    ) -> Result<(), CacheError<F::Error>> {
        if px.len() != usize::from(self.w) * usize::from(self.h) {
            return Err(CacheError::Size);
        }
        let mut next = match self.read_header(f)? {
            Some(n) => n,
            None => {
                // New or unusable: header and an empty index.
                f.write_at(0, &self.header(0)).map_err(CacheError::Io)?;
                let zero = [0u8; ENTRY as usize];
                for i in 0..self.slots {
                    f.write_at(HEADER + ENTRY * u32::from(i), &zero).map_err(CacheError::Io)?;
                }
                0
            }
        };
        let kb = key.to_bytes();
        let (hit, free) = self.find(f, &kb)?;
        let slot = match (hit, free) {
            (Some((s, _)), _) => s,
            (None, Some(s)) => s,
            (None, None) => {
                let s = next;
                next = (next + 1) % self.slots.max(1);
                s
            }
        };
        // Invalidate the entry, write the pixels, then the entry.
        let eoff = HEADER + ENTRY * u32::from(slot);
        f.write_at(eoff + 40, &[0]).map_err(CacheError::Io)?;
        let mut crc = 0;
        let mut buf = [0u8; 256];
        let mut off = self.slot_off(slot);
        for chunk in px.chunks(buf.len() / 2) {
            for (i, p) in chunk.iter().enumerate() {
                buf[2 * i..2 * i + 2].copy_from_slice(&p.to_le_bytes());
            }
            let b = &buf[..2 * chunk.len()];
            crc = crc32(crc, b);
            f.write_at(off, b).map_err(CacheError::Io)?;
            off += b.len() as u32;
        }
        let mut e = [0u8; ENTRY as usize];
        e[..KEY_LEN].copy_from_slice(&kb);
        e[36..38].copy_from_slice(&meta.orig_w.to_le_bytes());
        e[38..40].copy_from_slice(&meta.orig_h.to_le_bytes());
        e[40] = USED | u8::from(meta.approximate);
        e[44..48].copy_from_slice(&crc.to_le_bytes());
        f.write_at(eoff, &e).map_err(CacheError::Io)?;
        f.write_at(0, &self.header(next)).map_err(CacheError::Io)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    #[derive(Default)]
    struct MemFile {
        d: Vec<u8>,
        writes: usize,
        fail_after: Option<usize>,
    }

    impl CacheFile for MemFile {
        type Error = ();
        fn read_at(&mut self, off: u32, buf: &mut [u8]) -> Result<usize, ()> {
            let off = off as usize;
            if off >= self.d.len() {
                return Ok(0);
            }
            let n = buf.len().min(self.d.len() - off);
            buf[..n].copy_from_slice(&self.d[off..off + n]);
            Ok(n)
        }
        fn write_at(&mut self, off: u32, data: &[u8]) -> Result<(), ()> {
            if self.fail_after.is_some_and(|n| self.writes >= n) {
                return Err(());
            }
            self.writes += 1;
            let end = off as usize + data.len();
            if self.d.len() < end {
                self.d.resize(end, 0);
            }
            self.d[off as usize..end].copy_from_slice(data);
            Ok(())
        }
    }

    fn key(i: u32) -> ThumbKey {
        let mut k = ThumbKey {
            file_size: 1000 + u64::from(i),
            mtime: 7,
            w: 40,
            h: 30,
            rotation: 1,
            ..Default::default()
        };
        k.locator[..4].copy_from_slice(&i.to_le_bytes());
        k
    }

    fn pixels(seed: u32) -> Vec<u16> {
        (0..1200u32).map(|i| (i.wrapping_mul(2_654_435_761) ^ seed) as u16).collect()
    }

    #[test]
    fn crc_known() {
        assert_eq!(crc32(0, b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn store_lookup_replace() {
        let kind = CacheKind { slots: 4, ..THUMBS };
        let mut f = MemFile::default();
        let mut out = std::vec![0u16; 1200];
        assert_eq!(kind.lookup(&mut f, &key(1), &mut out), Ok(None));
        let meta = ThumbMeta { orig_w: 4000, orig_h: 3000, approximate: true };
        for i in 0..6 {
            kind.store(&mut f, &key(i), &meta, &pixels(i)).unwrap();
        }
        // 4 slots: keys 0 and 1 were replaced by 4 and 5.
        assert_eq!(kind.lookup(&mut f, &key(0), &mut out), Ok(None));
        assert_eq!(kind.lookup(&mut f, &key(1), &mut out), Ok(None));
        for i in 2..6 {
            assert_eq!(kind.lookup(&mut f, &key(i), &mut out), Ok(Some(meta)));
            assert_eq!(out, pixels(i));
        }
        // Same key again: replaced in place.
        kind.store(&mut f, &key(3), &meta, &pixels(99)).unwrap();
        kind.lookup(&mut f, &key(3), &mut out).unwrap().unwrap();
        assert_eq!(out, pixels(99));
        // A changed mtime misses.
        let mut k = key(3);
        k.mtime = 8;
        assert_eq!(kind.lookup(&mut f, &k, &mut out), Ok(None));
        assert_eq!(f.d.len(), (32 + 48 * 4 + 2400 * 4) as usize);
    }

    #[test]
    fn torn_write_is_a_miss() {
        let kind = CacheKind { slots: 4, ..THUMBS };
        let meta = ThumbMeta::default();
        let mut f = MemFile::default();
        kind.store(&mut f, &key(1), &meta, &pixels(1)).unwrap();
        // Rewriting key 1 with new pixels fails part-way through.
        f.fail_after = Some(f.writes + 3);
        assert!(kind.store(&mut f, &key(1), &meta, &pixels(2)).is_err());
        let mut out = std::vec![0u16; 1200];
        assert_eq!(kind.lookup(&mut f, &key(1), &mut out), Ok(None));
        // A garbage file is rebuilt.
        let mut g = MemFile { d: std::vec![0xAB; 100], ..Default::default() };
        kind.store(&mut g, &key(5), &meta, &pixels(5)).unwrap();
        assert_eq!(kind.lookup(&mut g, &key(5), &mut out), Ok(Some(meta)));
    }

    #[test]
    fn lookup_into_a_box() {
        let kind = CacheKind { slots: 2, ..THUMBS };
        let mut f = MemFile::default();
        let meta = ThumbMeta { orig_w: 640, orig_h: 480, approximate: false };
        kind.store(&mut f, &key(3), &meta, &pixels(3)).unwrap();
        // A 40 × 30 thumbnail at (5, 2) of a 128-wide frame buffer.
        let mut fb = std::vec![0xEEEEu16; 128 * 40];
        let at = 2 * 128 + 5;
        assert_eq!(kind.lookup_into(&mut f, &key(3), &mut fb[at..], 128), Ok(Some(meta)));
        let px = pixels(3);
        for y in 0..30 {
            assert_eq!(&fb[at + y * 128..at + y * 128 + 40], &px[y * 40..y * 40 + 40]);
            assert_eq!(fb[at + y * 128 + 40], 0xEEEE);
        }
        assert_eq!(fb[at - 1], 0xEEEE);
        assert_eq!(kind.lookup_into(&mut f, &key(4), &mut fb[at..], 128), Ok(None));
        assert_eq!(kind.lookup_into(&mut f, &key(3), &mut fb[..100], 128), Err(CacheError::Size));
        assert_eq!(ART.w, 52);
        assert_eq!(&ART_BIG.file_name(31), b"B01F.YTC");
    }

    #[test]
    fn meta_without_pixels() {
        let kind = CacheKind { slots: 4, ..THUMBS };
        let mut f = MemFile::default();
        assert_eq!(kind.meta(&mut f, &key(1)), Ok(None));
        let none = ThumbMeta { orig_w: 0, orig_h: 0, approximate: false };
        kind.store(&mut f, &key(1), &none, &[0; 40 * 30]).unwrap();
        let meta = ThumbMeta { orig_w: 500, orig_h: 500, approximate: true };
        kind.store(&mut f, &key(2), &meta, &pixels(2)).unwrap();
        assert_eq!(kind.meta(&mut f, &key(1)), Ok(Some(none)));
        assert_eq!(kind.meta(&mut f, &key(2)), Ok(Some(meta)));
        assert_eq!(kind.meta(&mut f, &key(3)), Ok(None));
        // Only the header and the index were read.
        let mut g = MemFile { d: f.d[..(HEADER + ENTRY * 4) as usize].to_vec(), ..MemFile::default() };
        assert_eq!(kind.meta(&mut g, &key(2)), Ok(Some(meta)));
    }

    #[test]
    fn names() {
        let k = key(9);
        let b = THUMBS.bucket(&k);
        assert!(b < 64);
        assert_eq!(&THUMBS.file_name(0x3F), b"T03F.YTC");
        assert_eq!(&ART.file_name(1), b"A001.YTC");
    }
}
