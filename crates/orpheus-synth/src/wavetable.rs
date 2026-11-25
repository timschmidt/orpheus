//! Small built-in wavetables, band-limited by mip level.
//!
//! [`TABLES`] single-cycle waves of [`LEN`] `i16` samples, each in [`LEVELS`]
//! versions: level `k` holds harmonics 1 to 64 >> k (64, 32, …, 1). The
//! oscillator picks the lowest level whose highest harmonic stays below
//! Nyquist ([`level_for`]) and interpolates linearly. The tables are built
//! by additive synthesis at compile time (a `const fn`), so they cost
//! `TABLES × LEVELS × LEN × 2` = 14 336 bytes of read-only data and no RAM.
//!
//! The waves (harmonic amplitudes, sine phase; our own choices):
//!
//! | # | Name | Harmonics |
//! |---|---|---|
//! | 0 | organ | 1, 2, 3, 4, 6, 8 at 1, 0.8, 0.6, 0.5, 0.3, 0.25 (drawbar-like) |
//! | 1 | bright | 1/h, harmonics 5–9 raised ×3 (a resonant saw) |
//! | 2 | hollow | odd harmonics only, at 2/(h(h+1)) |
//! | 3 | vocal | two formant groups around harmonics 3–4 and 9–11 over a 1/h² floor |

/// Samples per cycle.
pub const LEN: usize = 256;
/// Mip levels per table.
pub const LEVELS: usize = 7;
/// Number of built-in tables.
pub const TABLES: usize = 4;
/// Highest harmonic of level 0.
pub const TOP_HARMONIC: usize = 64;

const fn amp(t: usize, h: usize) -> f32 {
    let hf = h as f32;
    match t {
        0 => match h {
            1 => 1.0,
            2 => 0.8,
            3 => 0.6,
            4 => 0.5,
            6 => 0.3,
            8 => 0.25,
            _ => 0.0,
        },
        1 => {
            let a = 1.0 / hf;
            if h >= 5 && h <= 9 { 3.0 * a } else { a }
        }
        2 => {
            if h % 2 == 1 {
                2.0 / (hf * (1.0 + hf))
            } else {
                0.0
            }
        }
        _ => {
            let floor = 1.0 / (hf * hf);
            let f = match h {
                3 | 4 => 0.6,
                9..=11 => 0.25,
                _ => 0.0,
            };
            floor + f
        }
    }
}

/// sin(2π i/LEN) by the same Taylor polynomial as [`crate::math::sin_turns`].
const fn sine() -> [f32; LEN] {
    let mut s = [0.0f32; LEN];
    let mut i = 0;
    while i < LEN {
        let f = i as f32 / LEN as f32;
        let x = if f < 0.25 {
            f
        } else if f < 0.75 {
            0.5 - f
        } else {
            f - 1.0
        };
        let t = x * core::f32::consts::TAU;
        let t2 = t * t;
        let p = 1.0 / 39_916_800.0;
        let p = p * -t2 + 1.0 / 362_880.0;
        let p = p * t2 - 1.0 / 5040.0;
        let p = p * t2 + 1.0 / 120.0;
        let p = p * t2 - 1.0 / 6.0;
        let p = p * t2 + 1.0;
        s[i] = p * t;
        i += 1;
    }
    s
}

type Table = [[i16; LEN]; LEVELS];

const fn build() -> [Table; TABLES] {
    let sine = sine();
    let mut out = [[[0i16; LEN]; LEVELS]; TABLES];
    let mut t = 0;
    while t < TABLES {
        let mut f = [[0.0f32; LEN]; LEVELS];
        let mut peak = 0.0f32;
        let mut k = 0;
        while k < LEVELS {
            let top = TOP_HARMONIC >> k;
            let mut i = 0;
            while i < LEN {
                let mut s = 0.0;
                let mut h = 1;
                while h <= top {
                    let a = amp(t, h);
                    if a != 0.0 {
                        s += a * sine[(h * i) % LEN];
                    }
                    h += 1;
                }
                f[k][i] = s;
                let a = if s < 0.0 { -s } else { s };
                if a > peak {
                    peak = a;
                }
                i += 1;
            }
            k += 1;
        }
        // One scale per table (across its levels), so changing level does not
        // change the loudness.
        let scale = 32767.0 / peak;
        let mut k = 0;
        while k < LEVELS {
            let mut i = 0;
            while i < LEN {
                out[t][k][i] = (f[k][i] * scale) as i16;
                i += 1;
            }
            k += 1;
        }
        t += 1;
    }
    out
}

static WAVES: [Table; TABLES] = build();

/// The mip level for a phase increment of `dt` cycles per sample: the lowest
/// level whose top harmonic is below half the sample rate, or `None` if even
/// the fundamental is not.
#[must_use]
pub fn level_for(dt: f32) -> Option<u8> {
    let mut k = 0;
    while k < LEVELS {
        if ((TOP_HARMONIC >> k) as f32) * dt < 0.5 {
            return Some(k as u8);
        }
        k += 1;
    }
    None
}

/// The value of table `t` (wrapped to the number of tables) at level `k` and
/// phase `p` in [0, 1), linearly interpolated, in [−1, 1].
#[inline]
#[must_use]
pub fn sample(t: u8, k: u8, p: f32) -> f32 {
    let tab = &WAVES[usize::from(t) % TABLES][usize::from(k).min(LEVELS - 1)];
    let x = p * LEN as f32;
    let i = x as usize;
    let fr = x - i as f32;
    let a = f32::from(tab[i % LEN]);
    let b = f32::from(tab[(i + 1) % LEN]);
    (a + (b - a) * fr) * (1.0 / 32767.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_normalised_and_levels_chosen() {
        for (t, w) in WAVES.iter().enumerate() {
            let peak = w.iter().flatten().map(|v| v.unsigned_abs()).max().unwrap();
            assert!(peak >= 32_700, "table {t} peak {peak}");
        }
        assert_eq!(level_for(0.001), Some(0));
        assert_eq!(level_for(0.01), Some(1));
        assert_eq!(level_for(0.4), Some(6));
        assert_eq!(level_for(0.6), None);
        assert!(sample(0, 0, 0.0).abs() < 1e-3);
    }
}
