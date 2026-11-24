//! CELT bit allocation, ported from libopus 1.5.2 `celt/rate.[ch]`
//! (BSD-3-Clause, see LICENSE-XIPH). Decoder side only.

use super::modes::{BAND_ALLOCATION, CACHE_CAPS, LOGN, NB_ALLOC_VECTORS, NB_EBANDS, cache_row, ebands};
use crate::entdec::{BITRES, EcDec};

pub const MAX_FINE_BITS: i32 = 8;
const FINE_OFFSET: i32 = 21;
pub const QTHETA_OFFSET: i32 = 4;
pub const QTHETA_OFFSET_TWOPHASE: i32 = 16;
const ALLOC_STEPS: i32 = 6;
const LOG_MAX_PSEUDO: usize = 6;

static LOG2_FRAC_TABLE: [u8; 24] =
    [0, 8, 13, 16, 19, 21, 23, 24, 26, 27, 28, 29, 30, 31, 32, 32, 33, 34, 34, 35, 36, 36, 37, 37];

#[inline]
#[must_use]
pub fn get_pulses(i: i32) -> i32 {
    if i < 8 { i } else { (8 + (i & 7)) << ((i >> 3) - 1) }
}

#[inline]
fn cache_at(c: &[u8], i: i32) -> i32 {
    i32::from(c.get(usize::try_from(i).unwrap_or(0)).copied().unwrap_or(0))
}

#[must_use]
pub fn bits2pulses(band: usize, lm: i32, mut bits: i32) -> i32 {
    let cache = cache_row((lm + 1).max(0) as usize, band);
    let mut lo = 0i32;
    let mut hi = cache_at(cache, 0);
    bits -= 1;
    for _ in 0..LOG_MAX_PSEUDO {
        let mid = (lo + hi + 1) >> 1;
        if cache_at(cache, mid) >= bits {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let lov = if lo == 0 { -1 } else { cache_at(cache, lo) };
    if bits - lov <= cache_at(cache, hi) - bits { lo } else { hi }
}

#[must_use]
pub fn pulses2bits(band: usize, lm: i32, pulses: i32) -> i32 {
    let cache = cache_row((lm + 1).max(0) as usize, band);
    if pulses == 0 { 0 } else { cache_at(cache, pulses) + 1 }
}

/// `init_caps`.
pub fn init_caps(cap: &mut [i32; NB_EBANDS], lm: usize, c: usize) {
    for (i, cp) in cap.iter_mut().enumerate() {
        let n = (ebands(i + 1) - ebands(i)) << lm;
        let v = i32::from(CACHE_CAPS.get(NB_EBANDS * (2 * lm + c - 1) + i).copied().unwrap_or(0));
        *cp = ((v + 64) * c as i32 * n) >> 2;
    }
}

/// Output of [`compute_allocation`].
#[derive(Debug, Clone, Copy)]
pub struct Alloc {
    pub coded_bands: i32,
    pub intensity: i32,
    pub dual_stereo: bool,
    pub balance: i32,
    pub pulses: [i32; NB_EBANDS],
    pub ebits: [i32; NB_EBANDS],
    pub fine_priority: [i32; NB_EBANDS],
}

/// `clt_compute_allocation` (decoder).
pub fn compute_allocation(
    start: usize,
    end: usize,
    offsets: &[i32; NB_EBANDS],
    cap: &[i32; NB_EBANDS],
    alloc_trim: i32,
    total: i32,
    c: i32,
    lm: i32,
    ec: &mut EcDec<'_>,
) -> Alloc {
    let mut total = total.max(0);
    let len = NB_EBANDS;
    let mut skip_start = start;
    let skip_rsv = if total >= 1 << BITRES { 1 << BITRES } else { 0 };
    total -= skip_rsv;
    let mut intensity_rsv = 0;
    let mut dual_stereo_rsv = 0;
    if c == 2 {
        intensity_rsv = i32::from(LOG2_FRAC_TABLE[(end - start).min(23)]);
        if intensity_rsv > total {
            intensity_rsv = 0;
        } else {
            total -= intensity_rsv;
            dual_stereo_rsv = if total >= 1 << BITRES { 1 << BITRES } else { 0 };
            total -= dual_stereo_rsv;
        }
    }
    let mut bits1 = [0i32; NB_EBANDS];
    let mut bits2 = [0i32; NB_EBANDS];
    let mut thresh = [0i32; NB_EBANDS];
    let mut trim_offset = [0i32; NB_EBANDS];
    for j in start..end {
        let w = ebands(j + 1) - ebands(j);
        thresh[j] = (c << BITRES).max(((3 * w) << lm << BITRES) >> 4);
        trim_offset[j] =
            (c * w * (alloc_trim - 5 - lm) * (end as i32 - j as i32 - 1) * (1 << (lm + BITRES))) >> 6;
        if w << lm == 1 {
            trim_offset[j] -= c << BITRES;
        }
    }
    let av = |row: i32, j: usize| -> i32 {
        i32::from(BAND_ALLOCATION.get(row as usize * len + j).copied().unwrap_or(0))
    };
    let mut lo = 1i32;
    let mut hi = NB_ALLOC_VECTORS as i32 - 1;
    loop {
        let mut done = false;
        let mut psum = 0i32;
        let mid = (lo + hi) >> 1;
        for j in (start..end).rev() {
            let n = ebands(j + 1) - ebands(j);
            let mut bitsj = (c * n * av(mid, j)) << lm >> 2;
            if bitsj > 0 {
                bitsj = (bitsj + trim_offset[j]).max(0);
            }
            bitsj += offsets[j];
            if bitsj >= thresh[j] || done {
                done = true;
                psum += bitsj.min(cap[j]);
            } else if bitsj >= c << BITRES {
                psum += c << BITRES;
            }
        }
        if psum > total {
            hi = mid - 1;
        } else {
            lo = mid + 1;
        }
        if lo > hi {
            break;
        }
    }
    hi = lo;
    lo -= 1;
    for j in start..end {
        let n = ebands(j + 1) - ebands(j);
        let mut bits1j = (c * n * av(lo, j)) << lm >> 2;
        let mut bits2j = if hi >= NB_ALLOC_VECTORS as i32 { cap[j] } else { (c * n * av(hi, j)) << lm >> 2 };
        if bits1j > 0 {
            bits1j = (bits1j + trim_offset[j]).max(0);
        }
        if bits2j > 0 {
            bits2j = (bits2j + trim_offset[j]).max(0);
        }
        if lo > 0 {
            bits1j += offsets[j];
        }
        bits2j += offsets[j];
        if offsets[j] > 0 {
            skip_start = j;
        }
        bits2j = (bits2j - bits1j).max(0);
        bits1[j] = bits1j;
        bits2[j] = bits2j;
    }
    interp_bits2pulses(
        start,
        end,
        skip_start,
        &bits1,
        &bits2,
        &thresh,
        cap,
        total,
        skip_rsv,
        intensity_rsv,
        dual_stereo_rsv,
        c,
        lm,
        ec,
    )
}

fn interp_bits2pulses(
    start: usize,
    end: usize,
    skip_start: usize,
    bits1: &[i32; NB_EBANDS],
    bits2: &[i32; NB_EBANDS],
    thresh: &[i32; NB_EBANDS],
    cap: &[i32; NB_EBANDS],
    mut total: i32,
    skip_rsv: i32,
    mut intensity_rsv: i32,
    mut dual_stereo_rsv: i32,
    c: i32,
    lm: i32,
    ec: &mut EcDec<'_>,
) -> Alloc {
    let mut a = Alloc {
        coded_bands: 0,
        intensity: 0,
        dual_stereo: false,
        balance: 0,
        pulses: [0; NB_EBANDS],
        ebits: [0; NB_EBANDS],
        fine_priority: [0; NB_EBANDS],
    };
    let bits = &mut a.pulses;
    let ebits = &mut a.ebits;
    let fine_priority = &mut a.fine_priority;
    let alloc_floor = c << BITRES;
    let stereo = i32::from(c > 1);
    let log_m = lm << BITRES;
    let mut lo = 0i32;
    let mut hi = 1i32 << ALLOC_STEPS;
    for _ in 0..ALLOC_STEPS {
        let mid = (lo + hi) >> 1;
        let mut psum = 0i32;
        let mut done = false;
        for j in (start..end).rev() {
            let tmp = bits1[j] + ((mid * bits2[j]) >> ALLOC_STEPS);
            if tmp >= thresh[j] || done {
                done = true;
                psum += tmp.min(cap[j]);
            } else if tmp >= alloc_floor {
                psum += alloc_floor;
            }
        }
        if psum > total {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let mut psum = 0i32;
    let mut done = false;
    for j in (start..end).rev() {
        let mut tmp = bits1[j] + ((lo * bits2[j]) >> ALLOC_STEPS);
        if tmp < thresh[j] && !done {
            tmp = if tmp >= alloc_floor { alloc_floor } else { 0 };
        } else {
            done = true;
        }
        tmp = tmp.min(cap[j]);
        bits[j] = tmp;
        psum += tmp;
    }
    let es = ebands(start);
    let mut coded_bands = end as i32;
    loop {
        let j = (coded_bands - 1) as usize;
        if coded_bands - 1 <= skip_start as i32 {
            total += skip_rsv;
            break;
        }
        let mut left = total - psum;
        let span = (ebands(coded_bands as usize) - es).max(1);
        // celt_udiv: the C code divides the (possibly negative) value as
        // unsigned; keep that, with wrapping arithmetic.
        let percoeff = ((left as u32) / span as u32) as i32;
        left = left.wrapping_sub(span.wrapping_mul(percoeff));
        let rem = (left - (ebands(j) - es)).max(0);
        let band_width = ebands(coded_bands as usize) - ebands(j);
        let mut band_bits = bits[j].wrapping_add(percoeff.wrapping_mul(band_width)).wrapping_add(rem);
        if band_bits >= thresh[j].max(alloc_floor + (1 << BITRES)) {
            if ec.bit_logp(1) {
                break;
            }
            psum += 1 << BITRES;
            band_bits -= 1 << BITRES;
        }
        psum -= bits[j] + intensity_rsv;
        if intensity_rsv > 0 {
            intensity_rsv = i32::from(LOG2_FRAC_TABLE[(j - start).min(23)]);
        }
        psum += intensity_rsv;
        if band_bits >= alloc_floor {
            psum += alloc_floor;
            bits[j] = alloc_floor;
        } else {
            bits[j] = 0;
        }
        coded_bands -= 1;
    }
    let coded = coded_bands as usize;
    if intensity_rsv > 0 {
        a.intensity = start as i32 + ec.uint((coded_bands + 1 - start as i32) as u32) as i32;
    } else {
        a.intensity = 0;
    }
    if a.intensity <= start as i32 {
        total += dual_stereo_rsv;
        dual_stereo_rsv = 0;
    }
    a.dual_stereo = if dual_stereo_rsv > 0 { ec.bit_logp(1) } else { false };
    let mut left = total - psum;
    let span = (ebands(coded) - es).max(1);
    let percoeff = ((left as u32) / span as u32) as i32;
    left = left.wrapping_sub(span.wrapping_mul(percoeff));
    for j in start..coded {
        bits[j] = bits[j].wrapping_add(percoeff.wrapping_mul(ebands(j + 1) - ebands(j)));
    }
    for j in start..coded {
        let tmp = left.min(ebands(j + 1) - ebands(j));
        bits[j] += tmp;
        left -= tmp;
    }
    let mut balance = 0i32;
    let mut j = start;
    while j < coded {
        let n0 = ebands(j + 1) - ebands(j);
        let n = n0 << lm;
        let bit = bits[j] + balance;
        let mut excess;
        if n > 1 {
            excess = (bit - cap[j]).max(0);
            bits[j] = bit - excess;
            let den = c * n + i32::from(c == 2 && n > 2 && !a.dual_stereo && (j as i32) < a.intensity);
            let nclogn = den * (i32::from(LOGN[j]) + log_m);
            let mut offset = (nclogn >> 1) - den * FINE_OFFSET;
            if n == 2 {
                offset += (den << BITRES) >> 2;
            }
            if bits[j] + offset < (den * 2) << BITRES {
                offset += nclogn >> 2;
            } else if bits[j] + offset < (den * 3) << BITRES {
                offset += nclogn >> 3;
            }
            ebits[j] = (bits[j] + offset + (den << (BITRES - 1))).max(0);
            ebits[j] = ((ebits[j] as u32 / den.max(1) as u32) as i32) >> BITRES;
            if c * ebits[j] > (bits[j] >> BITRES) {
                ebits[j] = bits[j] >> stereo >> BITRES;
            }
            ebits[j] = ebits[j].min(MAX_FINE_BITS);
            fine_priority[j] = i32::from(ebits[j] * (den << BITRES) >= bits[j] + offset);
            bits[j] -= (c * ebits[j]) << BITRES;
        } else {
            excess = (bit - (c << BITRES)).max(0);
            bits[j] = bit - excess;
            ebits[j] = 0;
            fine_priority[j] = 1;
        }
        if excess > 0 {
            let extra_fine = (excess >> (stereo + BITRES)).min(MAX_FINE_BITS - ebits[j]);
            ebits[j] += extra_fine;
            let extra_bits = (extra_fine * c) << BITRES;
            fine_priority[j] = i32::from(extra_bits >= excess - balance);
            excess -= extra_bits;
        }
        balance = excess;
        j += 1;
    }
    a.balance = balance;
    while j < end {
        ebits[j] = bits[j] >> stereo >> BITRES;
        bits[j] = 0;
        fine_priority[j] = i32::from(ebits[j] < 1);
        j += 1;
    }
    a.coded_bands = coded_bands;
    a
}
