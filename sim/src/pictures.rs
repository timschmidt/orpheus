//! Pictures for the simulated card (made here, deterministic) and the
//! simulator's picture decoding: the real `yoton-img` decoders and scaler,
//! run at once into the screen (the firmware runs them in its card task).

use yoton_apps::services::{PicError, PicFormat, PicInfo};
use yoton_img::{Decoder, Error, Fit, Format, Options, Orient, Placement, Scaler, SliceSource, Target};

type Rgb = [u8; 3];

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

/// Night sky with an aurora over mountains (landscape).
#[must_use]
pub fn aurora(w: u32, h: u32) -> Vec<Rgb> {
    let mut v = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let mut c = mix([4, 8, 40], [10, 30, 70], fy);
            let band = 0.35 + 0.12 * (fx * 9.0).sin() + 0.05 * (fx * 23.0).cos();
            let d = (fy - band).abs();
            if d < 0.12 {
                c = mix(c, [60, 230, 140], 1.0 - d / 0.12);
            }
            let hash = (x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B)).wrapping_mul(0xC2B2_AE35);
            if hash >> 24 < 2 && fy < 0.6 {
                c = [240, 240, 255];
            }
            let ridge = 0.72 + 0.10 * (fx * 5.0 + 1.0).sin() * (fx * 2.3).cos();
            if fy > ridge {
                c = mix([12, 14, 20], [2, 2, 4], (fy - ridge) * 3.0);
            }
            v.push(c);
        }
    }
    v
}

/// A lake under the sun with green hills (landscape).
#[must_use]
pub fn lake(w: u32, h: u32) -> Vec<Rgb> {
    let mut v = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let horizon = 0.55;
            let hill = 0.45 + 0.08 * (fx * 7.0).sin();
            let mut c = if fy < hill {
                mix([120, 180, 250], [230, 240, 255], fy / hill)
            } else if fy < horizon {
                mix([40, 140, 50], [20, 90, 30], (fy - hill) / (horizon - hill))
            } else {
                mix([60, 120, 200], [10, 40, 90], (fy - horizon) / (1.0 - horizon))
            };
            let (sx, sy) = (fx - 0.72, (fy - 0.2) * h as f32 / w as f32);
            if sx * sx + sy * sy < 0.006 {
                c = [255, 220, 80];
            }
            if fy > horizon && (fx - 0.72).abs() < 0.03 * (1.0 + (fy * 60.0).sin()) {
                c = mix(c, [255, 220, 80], 0.6);
            }
            v.push(c);
        }
    }
    v
}

/// A sunset with a tree (portrait: letterboxed on the square screen).
#[must_use]
pub fn tree(w: u32, h: u32) -> Vec<Rgb> {
    let mut v = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let mut c = mix([250, 120, 40], [60, 20, 90], 1.0 - fy);
            let trunk = (fx - 0.5).abs() < 0.06 && fy > 0.45;
            let (cx, cy) = (fx - 0.5, (fy - 0.35) * h as f32 / w as f32);
            if trunk || cx * cx + cy * cy < 0.09 || fy > 0.9 {
                c = [20, 12, 16];
            }
            v.push(c);
        }
    }
    v
}

/// RGB565 bytes (little-endian) of a picture, for the PNG writer.
#[must_use]
pub fn to_rgb565_le(px: &[Rgb]) -> Vec<u8> {
    px.iter().flat_map(|&c| yoton_img::scale::rgb565(c).to_le_bytes()).collect()
}

/// A 24-bit bottom-up BMP.
#[must_use]
pub fn bmp(w: u32, h: u32, px: &[Rgb]) -> Vec<u8> {
    let row = (w * 3).div_ceil(4) * 4;
    let mut v = Vec::new();
    v.extend_from_slice(b"BM");
    v.extend_from_slice(&(54 + row * h).to_le_bytes());
    v.extend_from_slice(&[0; 4]);
    v.extend_from_slice(&54u32.to_le_bytes());
    v.extend_from_slice(&40u32.to_le_bytes());
    v.extend_from_slice(&w.to_le_bytes());
    v.extend_from_slice(&h.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&24u16.to_le_bytes());
    v.extend_from_slice(&[0; 24]);
    for y in (0..h).rev() {
        for x in 0..w {
            let c = px[(y * w + x) as usize];
            v.extend_from_slice(&[c[2], c[1], c[0]]);
        }
        v.resize(v.len() + (row - w * 3) as usize, 0);
    }
    v
}

/// A QOI file (every pixel as `QOI_OP_RGB`: valid, if not small).
#[must_use]
pub fn qoi(w: u32, h: u32, px: &[Rgb]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"qoif");
    v.extend_from_slice(&w.to_be_bytes());
    v.extend_from_slice(&h.to_be_bytes());
    v.extend_from_slice(&[3, 0]);
    for c in px {
        v.extend_from_slice(&[0xFE, c[0], c[1], c[2]]);
    }
    v.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
    v
}

fn format(f: Format) -> PicFormat {
    match f {
        Format::Jpeg => PicFormat::Jpeg,
        Format::Png => PicFormat::Png,
        Format::Gif => PicFormat::Gif,
        Format::Bmp => PicFormat::Bmp,
        Format::Qoi => PicFormat::Qoi,
    }
}

/// Decode `data` into a `w` × `h` box (RGB565 rows), fitted or filled,
/// rotated; 64 KiB of decoder memory as on the player with no music.
pub fn decode(data: &[u8], w: u16, h: u16, fill: bool, quarter: u8) -> Result<(Vec<u16>, PicInfo), PicError> {
    let mut out = vec![0u16; usize::from(w) * usize::from(h)];
    let mut arena = vec![0u8; 64 * 1024];
    let mut work = vec![0u32; Scaler::need_words(usize::from(w.max(h)))];
    let p = Placement {
        fit: if fill { Fit::Cover } else { Fit::Contain },
        rotate: Orient::quarter_turns(quarter),
        ..Placement::fit(w, h)
    };
    let mut sink = Scaler::new(&mut work, p, Target { buf: &mut out, stride: usize::from(w), x: 0, y: 0 })
        .map_err(|_| PicError::TooLarge)?;
    let mut dec = Decoder::new(&mut arena, Options::default());
    let r = dec.run(&mut SliceSource::new(data), &mut sink);
    let info = dec.info();
    match r {
        Ok(()) => {}
        Err(Error::UnknownFormat | Error::Unsupported(_)) => return Err(PicError::Unsupported),
        Err(Error::Arena { .. }) => return Err(PicError::TooLarge),
        Err(Error::Corrupt | Error::Truncated) => return Err(PicError::Corrupt),
    }
    let info = info.ok_or(PicError::Corrupt)?;
    let pi = PicInfo {
        width: info.width,
        height: info.height,
        format: Some(format(info.format)),
        reduced: info.interlaced,
    };
    Ok((out, pi))
}
