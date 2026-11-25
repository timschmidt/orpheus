//! The update file `orpheus-<version>.ofw` (02 §2.5), little-endian:
//!
//! | Offset | Field |
//! |---|---|
//! | `+0x00` | magic `"OFW1"` |
//! | `+0x04` | header version (1) |
//! | `+0x08` | header size ([`HEADER_SIZE`]) |
//! | `+0x0C` | target layout (`slot::LAYOUT_FULL`) |
//! | `+0x10` | image length (bytes of the `"YFW1"` slot image after the header) |
//! | `+0x14` | change-list length (bytes at `+0x80`) |
//! | `+0x18` | version text (16 bytes, NUL padded) |
//! | `+0x28` | build id (16 bytes, = the slot header's) |
//! | `+0x38` | SHA-256 of the image (32 bytes) |
//! | `+0x58..0x7C` | reserved (0) |
//! | `+0x7C` | CRC-32 of the header with this word read as 0 |
//! | `+0x80..0x180` | change list (UTF-8, lines separated by `\n`) |
//! | `+0x180` | the slot image |
//!
//! [`check`] validates the header *and* the slot header inside the image
//! against everything boot stage 2 and the pager will rely on, before a
//! single byte is written.

use orpheus_proto::hash::crc32_update;
use orpheus_proto::layout::full;
use orpheus_proto::slot::{self, LAYOUT_FULL, SlotHeader};
use orpheus_proto::{magic, rd32, wr32};

use crate::Error;

pub const MAGIC: u32 = magic(b"OFW1");
pub const VERSION: u32 = 1;
/// Header bytes before the image.
pub const HEADER_SIZE: usize = 0x180;
/// Start of the change list.
pub const CHANGES: usize = 0x80;
pub const CHANGES_MAX: usize = HEADER_SIZE - CHANGES;
pub const VERSION_LEN: usize = 16;
/// Header sector of a slot: the 128-byte `"YFW1"` header, then erased
/// bytes up to the resident image.
pub const SLOT_HEADER_SECTOR: u32 = 0x1000;
/// Largest image: the smaller of the two full-layout slots.
pub const IMAGE_MAX: u32 =
    if full::SLOT_A_SIZE < full::SLOT_B_SIZE { full::SLOT_A_SIZE } else { full::SLOT_B_SIZE };
/// Stage 2's limits (`fw/boot/stage2`): resident load address, largest
/// resident image, resident offset bound.
pub const LOAD_ADDR: u32 = 0x0010_0000;
pub const RESIDENT_MAX: u32 = 0x0003_7C00;
pub const RESIDENT_OFF_MAX: u32 = 0x0007_8000;

/// The fields of an `.ofw` header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub layout: u32,
    pub image_len: u32,
    pub changes_len: u16,
    pub version: [u8; VERSION_LEN],
    pub build_id: [u8; 16],
    pub sha256: [u8; 32],
}

fn crc(b: &[u8; HEADER_SIZE]) -> u32 {
    let mut c = crc32_update(0xFFFF_FFFF, &b[..0x7C]);
    c = crc32_update(c, &[0; 4]);
    crc32_update(c, &b[0x80..]) ^ 0xFFFF_FFFF
}

impl Header {
    /// The header bytes with `changes` (cut at [`CHANGES_MAX`] on a
    /// character boundary).
    #[must_use]
    pub fn to_bytes(&self, changes: &str) -> [u8; HEADER_SIZE] {
        let mut b = [0u8; HEADER_SIZE];
        let mut n = changes.len().min(CHANGES_MAX);
        while !changes.is_char_boundary(n) {
            n -= 1;
        }
        wr32(&mut b, 0x00, MAGIC);
        wr32(&mut b, 0x04, VERSION);
        wr32(&mut b, 0x08, HEADER_SIZE as u32);
        wr32(&mut b, 0x0C, self.layout);
        wr32(&mut b, 0x10, self.image_len);
        wr32(&mut b, 0x14, n as u32);
        b[0x18..0x28].copy_from_slice(&self.version);
        b[0x28..0x38].copy_from_slice(&self.build_id);
        b[0x38..0x58].copy_from_slice(&self.sha256);
        b[CHANGES..CHANGES + n].copy_from_slice(&changes.as_bytes()[..n]);
        let c = crc(&b);
        wr32(&mut b, 0x7C, c);
        b
    }

    /// Parse and check the header itself (not the image).
    pub fn parse(b: &[u8]) -> Result<Header, Error> {
        let b: &[u8; HEADER_SIZE] =
            b.get(..HEADER_SIZE).and_then(|s| s.try_into().ok()).ok_or(Error::Short)?;
        let r = |o| rd32(b, o).unwrap_or(0);
        if r(0) != MAGIC {
            return Err(Error::Magic);
        }
        if r(4) != VERSION || r(8) != HEADER_SIZE as u32 {
            return Err(Error::Version);
        }
        if r(0x7C) != crc(b) {
            return Err(Error::Crc);
        }
        let changes_len = r(0x14);
        if changes_len as usize > CHANGES_MAX {
            return Err(Error::Crc);
        }
        let mut h = Header {
            layout: r(0x0C),
            image_len: r(0x10),
            changes_len: changes_len as u16,
            version: [0; VERSION_LEN],
            build_id: [0; 16],
            sha256: [0; 32],
        };
        h.version.copy_from_slice(&b[0x18..0x28]);
        h.build_id.copy_from_slice(&b[0x28..0x38]);
        h.sha256.copy_from_slice(&b[0x38..0x58]);
        if h.version_str().is_none() {
            return Err(Error::Text);
        }
        Ok(h)
    }

    /// The version text (`None` if empty, not UTF-8 or not printable).
    #[must_use]
    pub fn version_str(&self) -> Option<&str> {
        let n = self.version.iter().position(|&c| c == 0).unwrap_or(VERSION_LEN);
        let s = core::str::from_utf8(&self.version[..n]).ok()?;
        (!s.is_empty() && s.bytes().all(|c| (0x21..0x7F).contains(&c))).then_some(s)
    }
}

/// The change list of a header's bytes (empty if invalid UTF-8).
#[must_use]
pub fn changes(b: &[u8]) -> &str {
    let n = rd32(b, 0x14).unwrap_or(0) as usize;
    b.get(CHANGES..CHANGES + n.min(CHANGES_MAX)).and_then(|s| core::str::from_utf8(s).ok()).unwrap_or("")
}

/// A version text as the header stores it (cut at 16 bytes).
#[must_use]
pub fn version_bytes(v: &str) -> [u8; VERSION_LEN] {
    let mut o = [0u8; VERSION_LEN];
    let n = v.len().min(VERSION_LEN);
    o[..n].copy_from_slice(&v.as_bytes()[..n]);
    o
}

/// Check an `.ofw` header and the first 128 bytes of its image (the slot
/// header) against everything the installer, stage 2 and the pager rely
/// on. Returns both parsed.
pub fn check(ofw: &[u8], slot_hdr: &[u8]) -> Result<(Header, SlotHeader), Error> {
    let h = Header::parse(ofw)?;
    if h.layout != LAYOUT_FULL {
        return Err(Error::Layout);
    }
    if h.image_len > IMAGE_MAX || h.image_len < SLOT_HEADER_SECTOR + 4 {
        return Err(Error::Size);
    }
    let s = SlotHeader::from_bytes(slot_hdr).map_err(|_| Error::SlotHeader)?;
    if s.layout != LAYOUT_FULL {
        return Err(Error::Layout);
    }
    if s.build_id != h.build_id {
        return Err(Error::BuildId);
    }
    let end = |off: u32, len: u32| off.checked_add(len).filter(|&e| e <= h.image_len);
    let pages = s
        .code_pages
        .checked_add(s.rodata_pages)
        .and_then(|n| n.checked_mul(slot::PAGE_SIZE))
        .ok_or(Error::Bounds)?;
    let ok = s.load_addr == LOAD_ADDR
        && s.resident_len > 0
        && s.resident_len <= RESIDENT_MAX
        && s.resident_off >= SLOT_HEADER_SECTOR
        && s.resident_off < RESIDENT_OFF_MAX
        && s.entry & 1 == 1
        && end(s.resident_off, s.resident_len).is_some()
        && s.pages_off >= SLOT_HEADER_SECTOR
        && s.pages_off.is_multiple_of(SLOT_HEADER_SECTOR)
        && end(s.pages_off, pages).is_some();
    if !ok {
        return Err(Error::Bounds);
    }
    Ok((h, s))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub use crate::sim::{image, ofw_header};

    #[test]
    fn roundtrip_and_checks() {
        let img = image(1, 0x2345, 2);
        let h = ofw_header(&img, "0.2.0+abc", "Faster start\nUpdate screen");
        let (o, s) = check(&h, &img[..128]).unwrap();
        assert_eq!(o.version_str(), Some("0.2.0+abc"));
        assert_eq!(o.image_len as usize, img.len());
        assert_eq!(s.resident_len, 0x2345);
        assert_eq!(changes(&h), "Faster start\nUpdate screen");
    }

    #[test]
    fn every_damaged_header_byte_is_caught() {
        let img = image(2, 0x1800, 1);
        let h = ofw_header(&img, "1.0", "x");
        for i in 0..HEADER_SIZE {
            let mut b = h;
            b[i] ^= 0x10;
            assert!(check(&b, &img[..128]).is_err(), "byte {i}");
        }
        assert_eq!(check(&h[..100], &img[..128]), Err(Error::Short));
    }

    #[test]
    fn refuses_wrong_layout_size_and_bounds() {
        let img = image(3, 0x1800, 1);
        let mut o = Header::parse(&ofw_header(&img, "1.0", "")).unwrap();
        let rebuild = |o: &Header| o.to_bytes("");
        o.layout = slot::LAYOUT_DUAL;
        assert_eq!(check(&rebuild(&o), &img[..128]), Err(Error::Layout));
        o.layout = LAYOUT_FULL;
        o.image_len = IMAGE_MAX + 0x1000;
        assert_eq!(check(&rebuild(&o), &img[..128]), Err(Error::Size));
        o.image_len = 0x800;
        assert_eq!(check(&rebuild(&o), &img[..128]), Err(Error::Size));
        o.image_len = img.len() as u32;
        o.build_id[3] ^= 1;
        assert_eq!(check(&rebuild(&o), &img[..128]), Err(Error::BuildId));
        o.build_id[3] ^= 1;
        // A slot header whose resident image lies in the header sector or
        // beyond the image, or is not Thumb, or loads elsewhere.
        let base = SlotHeader::from_bytes(&img).unwrap();
        let bad = [
            SlotHeader { resident_off: 0x800, ..base },
            SlotHeader { resident_len: img.len() as u32, ..base },
            SlotHeader { resident_len: RESIDENT_MAX + 4, ..base },
            SlotHeader { entry: LOAD_ADDR, ..base },
            SlotHeader { load_addr: 0x0010_2000, ..base },
            SlotHeader { code_pages: 9, ..base },
            SlotHeader { code_pages: u32::MAX, ..base },
            SlotHeader { layout: slot::LAYOUT_DUAL, ..base },
        ];
        for (i, s) in bad.iter().enumerate() {
            let r = check(&rebuild(&o), &s.to_bytes());
            assert!(matches!(r, Err(Error::Bounds | Error::Layout)), "case {i}: {r:?}");
        }
        assert_eq!(check(&rebuild(&o), &[0u8; 128]), Err(Error::SlotHeader));
    }

    #[test]
    fn version_text() {
        let mut o = Header::parse(&ofw_header(&image(4, 0x1000, 1), "1.0", "")).unwrap();
        o.version = [0; 16];
        assert_eq!(Header::parse(&o.to_bytes("")), Err(Error::Text));
        o.version = version_bytes("bad version");
        assert_eq!(Header::parse(&o.to_bytes("")), Err(Error::Text));
        o.version = version_bytes("0123456789abcdefXYZ");
        assert_eq!(Header::parse(&o.to_bytes("")).unwrap().version_str(), Some("0123456789abcdef"));
        // A change list cut on a character boundary.
        let long: std::string::String = "é".repeat(200);
        let b = o.to_bytes(&long);
        assert_eq!(changes(&b).len(), CHANGES_MAX);
        assert!(changes(&b).chars().all(|c| c == 'é'));
    }
}
