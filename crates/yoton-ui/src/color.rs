//! RGB565 colour values and blending.
//!
//! Colours are raw `u16` RGB565 values as stored in the frame buffer (native
//! endianness). Drawing code never uses literal colours; it asks the
//! [`Palette`](crate::theme::Palette) for a role.

/// A raw RGB565 pixel value.
pub type Color = u16;

/// RGB888 to RGB565 (truncating).
#[must_use]
pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
    ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3)
}

/// Split into 5/6/5-bit channels.
#[must_use]
pub const fn channels(c: Color) -> (u16, u16, u16) {
    ((c >> 11) & 31, (c >> 5) & 63, c & 31)
}

/// Expand to RGB888 with bit replication (what the host PNG export does).
#[must_use]
pub const fn to_rgb888(c: Color) -> [u8; 3] {
    let (r, g, b) = channels(c);
    let (r, g, b) = (r as u8, g as u8, b as u8);
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
}

/// `fg` over `bg` with coverage `a` (0 = all `bg`, 255 = all `fg`).
#[must_use]
pub fn blend(bg: Color, fg: Color, a: u8) -> Color {
    match a {
        0 => bg,
        255 => fg,
        _ => {
            let a = u32::from(a) + u32::from(a >> 7); // 0..=256
            let (br, bgc, bb) = channels(bg);
            let (fr, fgc, fb) = channels(fg);
            let mix = |b: u16, f: u16| -> u16 {
                let v = (u32::from(f) * a + u32::from(b) * (256 - a) + 128) >> 8;
                // Channels are at most 6 bits wide, so this never truncates.
                u16::try_from(v).unwrap_or(0)
            };
            (mix(br, fr) << 11) | (mix(bgc, fgc) << 5) | mix(bb, fb)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_and_blend() {
        assert_eq!(rgb(255, 255, 255), 0xFFFF);
        assert_eq!(rgb(255, 0, 0), 0xF800);
        assert_eq!(blend(0, 0xFFFF, 0), 0);
        assert_eq!(blend(0, 0xFFFF, 255), 0xFFFF);
        let mid = blend(0, 0xFFFF, 128);
        let (r, g, b) = channels(mid);
        assert!((15..=16).contains(&r) && (31..=32).contains(&g) && (15..=16).contains(&b));
        assert_eq!(to_rgb888(0xF800), [255, 0, 0]);
    }
}
