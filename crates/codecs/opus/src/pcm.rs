//! Float to `i16` conversion and the soft clipper of libopus 1.5.2
//! `src/opus.c` (`opus_pcm_soft_clip`) and `celt/float_cast.h`
//! (`FLOAT2INT16`), BSD-3-Clause, see LICENSE-XIPH.

/// Round to nearest even and saturate, as `FLOAT2INT16`.
#[inline]
#[must_use]
pub fn float2int16(x: f32) -> i16 {
    let x = (x * 32768.0).clamp(-32768.0, 32767.0);
    // 1.5 * 2^23: adding and subtracting it rounds to an integer (ties to
    // even) for |x| < 2^22.
    const MAGIC: f32 = 12_582_912.0;
    ((x + MAGIC) - MAGIC) as i16
}

/// `opus_pcm_soft_clip` on interleaved `x` (`n` frames of `c` channels).
pub fn soft_clip(x: &mut [f32], n: usize, c: usize, mem: &mut [f32; 2]) {
    if c < 1 || n < 1 {
        return;
    }
    for v in x[..n * c].iter_mut() {
        *v = v.clamp(-2.0, 2.0);
    }
    for ch in 0..c {
        let at = |i: usize| i * c + ch;
        let mut a = mem[ch];
        for i in 0..n {
            let v = x[at(i)];
            if v * a >= 0.0 {
                break;
            }
            x[at(i)] = v + a * v * v;
        }
        let mut curr = 0usize;
        let x0 = x[at(0)];
        loop {
            let mut i = curr;
            while i < n {
                let v = x[at(i)];
                if v > 1.0 || v < -1.0 {
                    break;
                }
                i += 1;
            }
            if i == n {
                a = 0.0;
                break;
            }
            let mut peak_pos = i;
            let mut start = i;
            let mut end = i;
            let xi = x[at(i)];
            let mut maxval = xi.abs();
            while start > 0 && xi * x[at(start - 1)] >= 0.0 {
                start -= 1;
            }
            while end < n && xi * x[at(end)] >= 0.0 {
                if x[at(end)].abs() > maxval {
                    maxval = x[at(end)].abs();
                    peak_pos = end;
                }
                end += 1;
            }
            let special = start == 0 && xi * x[at(0)] >= 0.0;
            a = (maxval - 1.0) / (maxval * maxval);
            a += a * 2.4e-7;
            if xi > 0.0 {
                a = -a;
            }
            for k in start..end {
                let v = x[at(k)];
                x[at(k)] = v + a * v * v;
            }
            if special && peak_pos >= 2 {
                let mut offset = x0 - x[at(0)];
                let delta = offset / peak_pos as f32;
                for k in curr..peak_pos {
                    offset -= delta;
                    let v = x[at(k)] + offset;
                    x[at(k)] = v.clamp(-1.0, 1.0);
                }
            }
            curr = end;
            if curr == n {
                break;
            }
        }
        mem[ch] = a;
    }
}
