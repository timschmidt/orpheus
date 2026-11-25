//! The binary format of save files and the score table.
//!
//! ```text
//! "OGS1" | kind u8 | version u8 | length u16 LE | payload | CRC-32 LE
//! ```
//!
//! The CRC (IEEE 802.3, reflected, as zlib) covers everything before it.
//! Payload fields are little-endian; each game's `encode`/`decode` writes
//! and reads them in a fixed order for its version and validates every
//! value on the way back, so a damaged or foreign file is refused, never
//! half-loaded.

pub const MAGIC: [u8; 4] = *b"OGS1";
/// Bytes around the payload: magic, kind, version, length, CRC.
pub const OVERHEAD: usize = 12;

/// CRC-32 (IEEE, reflected, initial and final XOR `0xFFFF_FFFF`).
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

/// Writes fields into a buffer; remembers an overflow instead of
/// panicking.
pub struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
    ok: bool,
}

impl<'a> Writer<'a> {
    pub fn new(buf: &'a mut [u8]) -> Writer<'a> {
        Writer { buf, pos: 0, ok: true }
    }

    pub fn bytes(&mut self, b: &[u8]) {
        match self.buf.get_mut(self.pos..self.pos + b.len()) {
            Some(dst) if self.ok => {
                dst.copy_from_slice(b);
                self.pos += b.len();
            }
            _ => self.ok = false,
        }
    }

    pub fn u8(&mut self, v: u8) {
        self.bytes(&[v]);
    }

    pub fn i8(&mut self, v: i8) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn bool(&mut self, v: bool) {
        self.u8(u8::from(v));
    }

    pub fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }

    /// Bytes written so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pos
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pos == 0
    }

    /// Everything fitted.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.ok
    }
}

/// Reads fields; every read is `None` past the end.
pub struct Reader<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    #[must_use]
    pub fn new(d: &'a [u8]) -> Reader<'a> {
        Reader { d, pos: 0 }
    }

    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let b = self.d.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(b)
    }

    pub fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.bytes(N)?.try_into().ok()
    }

    pub fn u8(&mut self) -> Option<u8> {
        Some(self.array::<1>()?[0])
    }

    pub fn i8(&mut self) -> Option<i8> {
        Some(i8::from_le_bytes(self.array()?))
    }

    /// Exactly 0 or 1.
    pub fn bool(&mut self) -> Option<bool> {
        match self.u8()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.array()?))
    }

    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.array()?))
    }

    /// Everything was read.
    #[must_use]
    pub fn done(&self) -> bool {
        self.pos == self.d.len()
    }
}

/// Write one file into `buf`: the header, what `body` writes, the CRC.
/// Returns its length, or `None` if it did not fit.
pub fn seal(buf: &mut [u8], kind: u8, version: u8, body: impl FnOnce(&mut Writer<'_>)) -> Option<usize> {
    let (head, rest) = buf.split_at_mut_checked(8)?;
    let cap = rest.len().checked_sub(4)?;
    let n = {
        let mut w = Writer::new(rest.get_mut(..cap)?);
        body(&mut w);
        if !w.ok() {
            return None;
        }
        w.len()
    };
    let len = u16::try_from(n).ok()?;
    head[..4].copy_from_slice(&MAGIC);
    head[4] = kind;
    head[5] = version;
    head[6..8].copy_from_slice(&len.to_le_bytes());
    let total = 8 + n;
    let crc = crc32(buf.get(..total)?);
    buf.get_mut(total..total + 4)?.copy_from_slice(&crc.to_le_bytes());
    Some(total + 4)
}

/// Check a file and return its kind, version and payload.
#[must_use]
pub fn open(data: &[u8]) -> Option<(u8, u8, &[u8])> {
    let head = data.get(..8)?;
    if head[..4] != MAGIC {
        return None;
    }
    let len = usize::from(u16::from_le_bytes([head[6], head[7]]));
    if data.len() != 8 + len + 4 {
        return None;
    }
    let crc = u32::from_le_bytes(data.get(8 + len..)?.try_into().ok()?);
    if crc32(data.get(..8 + len)?) != crc {
        return None;
    }
    Some((head[4], head[5], data.get(8..8 + len)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_reference() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn seal_and_open() {
        let mut buf = [0u8; 32];
        let n = seal(&mut buf, 7, 3, |w| {
            w.u8(1);
            w.u16(0x0203);
            w.u32(0x0405_0607);
            w.bool(true);
            w.i8(-2);
        })
        .unwrap();
        assert_eq!(n, OVERHEAD + 9);
        let (k, v, p) = open(&buf[..n]).unwrap();
        assert_eq!((k, v, p.len()), (7, 3, 9));
        let mut r = Reader::new(p);
        assert_eq!(
            (r.u8(), r.u16(), r.u32(), r.bool(), r.i8()),
            (Some(1), Some(0x0203), Some(0x0405_0607), Some(true), Some(-2))
        );
        assert!(r.done());
        assert_eq!(r.u8(), None);
        // Too small a buffer, a damaged byte, a wrong length, no magic.
        assert_eq!(seal(&mut buf[..15], 7, 3, |w| w.u64(1)), None);
        for i in 0..n {
            let mut b = buf;
            b[i] ^= 0x10;
            assert!(open(&b[..n]).is_none(), "byte {i}");
        }
        assert!(open(&buf[..n + 1]).is_none());
        assert!(open(b"OGS").is_none());
        let mut r = Reader::new(&[2]);
        assert_eq!(r.bool(), None);
    }
}
