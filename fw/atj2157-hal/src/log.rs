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

/// Start of an image: keep the ring a previous start left (its last lines
/// say what happened before a reset) and mark the new start, or set up an
/// empty one ([`logring::keep`]). Runs before the pager: never inlined, so
/// `check-resident` proves everything it reaches resident.
#[inline(never)]
pub fn init() {
    let kept = logring::keep(
        rd(BASE + logring::OFF_MAGIC as u32),
        rd(BASE + logring::OFF_CAP as u32),
        rd(BASE + logring::OFF_WRITE as u32),
    );
    if kept {
        line(logring::BOOT_MARK);
        return;
    }
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

    // Nothing here can panic (no indexing the compiler cannot prove): the
    // panic handler formats with it, and a panic inside it would recurse
    // (VER-10, `xtask stack`).
    fn push_byte(&mut self, b: u8) {
        if let Some(d) = self.buf.get_mut(self.n) {
            *d = b;
            self.n += 1;
        }
    }

    pub fn push_str(&mut self, s: &str) -> &mut Self {
        for &b in s.as_bytes() {
            self.push_byte(b);
        }
        self
    }

    pub fn push_u32(&mut self, v: u32) -> &mut Self {
        // Most significant digit first; `checked_*` keep the division-by-
        // zero panic out.
        let mut started = false;
        for p in [1_000_000_000, 100_000_000, 10_000_000, 1_000_000, 100_000, 10_000, 1_000, 100, 10, 1u32] {
            let d = v.checked_div(p).unwrap_or(0).checked_rem(10).unwrap_or(0);
            if d != 0 || started || p == 1 {
                started = true;
                self.push_byte(b'0' + d as u8);
            }
        }
        self
    }

    pub fn push_hex(&mut self, v: u32) -> &mut Self {
        self.push_str("0x");
        for s in (0..8).rev() {
            let nib = ((v >> (s * 4)) & 0xF) as u8;
            self.push_byte(if nib < 10 { b'0' + nib } else { b'a' + nib - 10 });
        }
        self
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.buf.get(..self.n).unwrap_or(&[])
    }

    pub fn emit(&self) {
        line(self.as_bytes());
    }
}
