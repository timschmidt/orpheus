//! Range decoder (RFC 6716 §4.1), ported from libopus `celt/entdec.c`,
//! `celt/entcode.[ch]` and `celt/laplace.c` (BSD-3-Clause, Xiph.Org
//! Foundation, Timothy B. Terriberry; see the crate README).

const SYM_BITS: u32 = 8;
const CODE_BITS: i32 = 32;
const SYM_MAX: u32 = (1 << SYM_BITS) - 1;
const CODE_TOP: u32 = 1 << (CODE_BITS - 1);
const CODE_BOT: u32 = CODE_TOP >> SYM_BITS;
const CODE_EXTRA: u32 = ((CODE_BITS - 2) % SYM_BITS as i32 + 1) as u32;
const UINT_BITS: i32 = 8;
const WINDOW_SIZE: i32 = 32;
pub const BITRES: i32 = 3;

/// Bits needed to represent `x` (0 for 0).
#[inline]
#[must_use]
pub fn ilog(x: u32) -> i32 {
    32 - x.leading_zeros() as i32
}

/// Range decoder state over one packet (or part of one).
#[derive(Debug, Clone)]
pub struct EcDec<'a> {
    buf: &'a [u8],
    /// Bytes of `buf` in use (the redundancy frame at the end is cut off by
    /// shrinking this).
    pub storage: u32,
    end_offs: u32,
    end_window: u32,
    nend_bits: i32,
    nbits_total: i32,
    offs: u32,
    pub rng: u32,
    val: u32,
    ext: u32,
    rem: i32,
    pub error: bool,
}

impl<'a> EcDec<'a> {
    #[must_use]
    pub fn new(buf: &'a [u8]) -> Self {
        let storage = buf.len().min(u32::MAX as usize) as u32;
        let mut d = Self {
            buf,
            storage,
            end_offs: 0,
            end_window: 0,
            nend_bits: 0,
            nbits_total: CODE_BITS + 1
                - ((CODE_BITS - CODE_EXTRA as i32) / SYM_BITS as i32) * SYM_BITS as i32,
            offs: 0,
            rng: 1 << CODE_EXTRA,
            val: 0,
            ext: 0,
            rem: 0,
            error: false,
        };
        d.rem = d.read_byte();
        d.val = d.rng - 1 - ((d.rem as u32) >> (SYM_BITS - CODE_EXTRA));
        d.normalize();
        d
    }

    #[inline]
    fn read_byte(&mut self) -> i32 {
        if self.offs < self.storage {
            let b = self.buf.get(self.offs as usize).copied().unwrap_or(0);
            self.offs += 1;
            i32::from(b)
        } else {
            0
        }
    }

    #[inline]
    fn read_byte_from_end(&mut self) -> u32 {
        if self.end_offs < self.storage {
            self.end_offs += 1;
            u32::from(self.buf.get((self.storage - self.end_offs) as usize).copied().unwrap_or(0))
        } else {
            0
        }
    }

    #[inline(never)]
    fn normalize(&mut self) {
        while self.rng <= CODE_BOT {
            self.nbits_total += SYM_BITS as i32;
            self.rng <<= SYM_BITS;
            let mut sym = self.rem;
            self.rem = self.read_byte();
            sym = ((sym << SYM_BITS) | self.rem) >> (SYM_BITS - CODE_EXTRA);
            self.val = ((self.val << SYM_BITS).wrapping_add(SYM_MAX & !(sym as u32))) & (CODE_TOP - 1);
        }
    }

    #[inline(never)]
    pub fn decode(&mut self, ft: u32) -> u32 {
        self.ext = self.rng / ft.max(1);
        let s = self.val / self.ext.max(1);
        ft - (s + 1).min(ft)
    }

    #[inline(never)]
    pub fn decode_bin(&mut self, bits: u32) -> u32 {
        self.ext = self.rng >> bits;
        let s = self.val / self.ext.max(1);
        (1u32 << bits) - (s + 1).min(1u32 << bits)
    }

    #[inline(never)]
    pub fn update(&mut self, fl: u32, fh: u32, ft: u32) {
        let s = self.ext.wrapping_mul(ft.wrapping_sub(fh));
        self.val = self.val.wrapping_sub(s);
        self.rng = if fl > 0 { self.ext.wrapping_mul(fh.wrapping_sub(fl)) } else { self.rng.wrapping_sub(s) };
        self.normalize();
    }

    /// A bit that is 1 with probability `1/(1 << logp)`.
    #[inline(never)]
    pub fn bit_logp(&mut self, logp: u32) -> bool {
        let r = self.rng;
        let d = self.val;
        let s = r >> logp;
        let ret = d < s;
        if !ret {
            self.val = d - s;
        }
        self.rng = if ret { s } else { r - s };
        self.normalize();
        ret
    }

    /// A symbol from an inverse CDF table with `1 << ftb` total.
    #[inline(never)]
    pub fn icdf(&mut self, icdf: &[u8], ftb: u32) -> i32 {
        let mut s = self.rng;
        let d = self.val;
        let r = s >> ftb;
        let mut ret = 0usize;
        let mut t;
        loop {
            t = s;
            s = r.wrapping_mul(u32::from(icdf.get(ret).copied().unwrap_or(0)));
            if d >= s {
                break;
            }
            ret += 1;
        }
        self.val = d - s;
        self.rng = t - s;
        self.normalize();
        ret as i32
    }

    /// As [`Self::icdf`] with 16-bit table entries.
    #[inline(never)]
    pub fn icdf16(&mut self, icdf: &[u16], ftb: u32) -> i32 {
        let mut s = self.rng;
        let d = self.val;
        let r = s >> ftb;
        let mut ret = 0usize;
        let mut t;
        loop {
            t = s;
            s = r.wrapping_mul(u32::from(icdf.get(ret).copied().unwrap_or(0)));
            if d >= s {
                break;
            }
            ret += 1;
        }
        self.val = d - s;
        self.rng = t - s;
        self.normalize();
        ret as i32
    }

    /// A uniform integer in `[0, ft)` (`ft > 1`).
    pub fn uint(&mut self, ft: u32) -> u32 {
        if ft <= 1 {
            return 0;
        }
        let ft1 = ft - 1;
        let mut ftb = ilog(ft1);
        if ftb > UINT_BITS {
            ftb -= UINT_BITS;
            let f = (ft1 >> ftb) + 1;
            let s = self.decode(f);
            self.update(s, s + 1, f);
            let t = (s << ftb) | self.bits(ftb as u32);
            if t <= ft1 {
                return t;
            }
            self.error = true;
            ft1
        } else {
            let s = self.decode(ft);
            self.update(s, s + 1, ft);
            s
        }
    }

    /// Raw bits from the end of the buffer.
    pub fn bits(&mut self, bits: u32) -> u32 {
        let mut window = self.end_window;
        let mut available = self.nend_bits;
        if (available as u32) < bits {
            loop {
                window |= self.read_byte_from_end() << available;
                available += SYM_BITS as i32;
                if available > WINDOW_SIZE - SYM_BITS as i32 {
                    break;
                }
            }
        }
        let ret = if bits >= 32 { window } else { window & ((1u32 << bits) - 1) };
        window = if bits >= 32 { 0 } else { window >> bits };
        available -= bits as i32;
        self.end_window = window;
        self.nend_bits = available;
        self.nbits_total += bits as i32;
        ret
    }

    /// Bits used so far (rounded up).
    #[inline]
    #[must_use]
    pub fn tell(&self) -> i32 {
        self.nbits_total - ilog(self.rng)
    }

    /// Bits used so far in 1/8 bit.
    #[must_use]
    pub fn tell_frac(&self) -> u32 {
        static CORRECTION: [u32; 8] = [35733, 38967, 42495, 46340, 50535, 55109, 60097, 65535];
        let nbits = (self.nbits_total as u32) << BITRES;
        let mut l = ilog(self.rng);
        let r = self.rng >> (l - 16);
        let mut b = (r >> 12).wrapping_sub(8) as usize & 7;
        b += usize::from(r > CORRECTION[b]);
        l = (l << 3) + b as i32;
        nbits.wrapping_sub(l as u32)
    }

    /// Laplace-distributed value (CELT coarse energy).
    pub fn laplace(&mut self, mut fs: u32, decay: i32) -> i32 {
        const MINP: u32 = 1;
        const NMIN: u32 = 16;
        let mut val = 0i32;
        let fm = self.decode_bin(15);
        let mut fl = 0u32;
        if fm >= fs {
            val += 1;
            fl = fs;
            let ft = 32768 - MINP * (2 * NMIN) - fs;
            fs = ((ft as i32 * (16384 - decay)) >> 15) as u32 + MINP;
            while fs > MINP && fm >= fl + 2 * fs {
                fs *= 2;
                fl += fs;
                fs = (((fs - 2 * MINP) as i32 * decay) >> 15) as u32;
                fs += MINP;
                val += 1;
            }
            if fs <= MINP {
                let di = (fm - fl) >> 1;
                val += di as i32;
                fl += 2 * di * MINP;
            }
            if fm < fl + fs {
                val = -val;
            } else {
                fl += fs;
            }
        }
        // Corrupt input can push the interval out of range: clamp rather
        // than assert (the decoded value is then garbage, never a panic).
        let fl = fl.min(32767);
        let fh = (fl + fs.max(1)).min(32768);
        self.update(fl, fh, 32768);
        val
    }

    /// Pretend everything up to bit `tell` was read (CELT silence frames).
    pub fn skip_to(&mut self, tell: i32) {
        self.nbits_total += tell - self.tell();
    }

    /// Bytes of the buffer consumed from the front.
    #[must_use]
    pub fn range_bytes(&self) -> u32 {
        self.offs
    }
}
