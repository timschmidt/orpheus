//! A tiny text log ring for bring-up images, in the persistent log-ring
//! format of `orpheus_proto::logring` (NUL-separated lines). The firmware will
//! put defmt frames into the same ring; the host prints printable bytes.

use orpheus_proto::logring;

/// Where this image's ring lives (`ALT_BASE` in SRAM0 for RAM images).
const BASE: u32 = logring::ALT_BASE;

fn wr(addr: u32, v: u32) {
    // SAFETY: the log ring area is reserved by the linker script.
    unsafe { core::ptr::write_volatile(addr as *mut u32, v) }
}
fn rd(addr: u32) -> u32 {
    // SAFETY: as in `wr`.
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

/// Reset the ring (start of an image).
pub fn init() {
    wr(BASE + logring::OFF_WRITE as u32, 0);
    wr(BASE + logring::OFF_CAP as u32, logring::CAP as u32);
    wr(BASE + logring::OFF_BUILD as u32, 0);
    wr(BASE + logring::OFF_MAGIC as u32, logring::MAGIC);
}

fn put_byte(b: u8) {
    let w = rd(BASE + logring::OFF_WRITE as u32);
    let pos = BASE + logring::HEADER as u32 + w % logring::CAP as u32;
    // SAFETY: inside the reserved ring.
    unsafe { core::ptr::write_volatile(pos as *mut u8, b) };
    wr(BASE + logring::OFF_WRITE as u32, w.wrapping_add(1));
}

/// Append one line.
pub fn line(s: &[u8]) {
    critical_section::with(|_| {
        for &b in s {
            put_byte(b);
        }
        put_byte(0);
    });
}

/// Fixed-size line builder without `core::fmt`.
pub struct LineBuf {
    buf: [u8; 96],
    n: usize,
}

impl Default for LineBuf {
    fn default() -> Self {
        Self::new()
    }
}

impl LineBuf {
    #[must_use]
    pub const fn new() -> Self {
        Self { buf: [0; 96], n: 0 }
    }

    pub fn push_str(&mut self, s: &str) -> &mut Self {
        for &b in s.as_bytes() {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    pub fn push_u32(&mut self, mut v: u32) -> &mut Self {
        let mut d = [0u8; 10];
        let mut i = d.len();
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        for &c in &d[i..] {
            if self.n < self.buf.len() {
                self.buf[self.n] = c;
                self.n += 1;
            }
        }
        self
    }

    pub fn push_hex(&mut self, v: u32) -> &mut Self {
        self.push_str("0x");
        for s in (0..8).rev() {
            let nib = ((v >> (s * 4)) & 0xF) as u8;
            let c = if nib < 10 { b'0' + nib } else { b'a' + nib - 10 };
            if self.n < self.buf.len() {
                self.buf[self.n] = c;
                self.n += 1;
            }
        }
        self
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.n]
    }

    pub fn emit(&self) {
        line(self.as_bytes());
    }
}
