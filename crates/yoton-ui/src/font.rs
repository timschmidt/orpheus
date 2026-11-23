//! Bitmap fonts: the built-in faces (format `YFN1`, see `fontgen/fontgen.py`)
//! and the 16 × 16 fallback font read on demand from a [`GlyphSource`].

/// A built-in bitmap font. The data is `'static` rodata (paged on the
/// device); nothing is copied to RAM.
#[derive(Clone, Copy, Debug)]
pub struct Font {
    data: &'static [u8],
    bpp: u8,
    /// Cap height: baseline to the top of `H` (or `0` for numeral fonts).
    pub ascent: u8,
    /// Baseline to the bottom of `g`.
    pub descent: u8,
    /// Suggested line pitch.
    pub line: u8,
    n_ranges: u16,
    n_glyphs: u16,
}

const HEADER: usize = 12;
const RANGE: usize = 8;
const GLYPH: usize = 8;

/// One glyph ready to blit: `w × h` pixels of `bpp` bits, row-major, MSB
/// first; the top-left pixel is at (`pen_x + x`, `baseline - top`).
#[derive(Clone, Copy, Debug)]
pub struct Glyph<'a> {
    pub adv: u8,
    pub w: u8,
    pub h: u8,
    pub x: i8,
    pub top: i8,
    pub bpp: u8,
    pub bits: &'a [u8],
}

const fn rd16(d: &[u8], o: usize) -> u16 {
    d[o] as u16 | ((d[o + 1] as u16) << 8)
}

impl Font {
    /// Parse a `YFN1` blob. Evaluated at compile time for the built-in fonts,
    /// so a malformed file fails the build.
    #[must_use]
    pub const fn parse(data: &'static [u8]) -> Font {
        assert!(data.len() >= HEADER);
        assert!(data[0] == b'Y' && data[1] == b'F' && data[2] == b'N' && data[3] == b'1');
        let n_ranges = rd16(data, 8);
        let n_glyphs = rd16(data, 10);
        assert!(data.len() >= HEADER + n_ranges as usize * RANGE + n_glyphs as usize * GLYPH);
        Font { data, bpp: data[4], ascent: data[5], descent: data[6], line: data[7], n_ranges, n_glyphs }
    }

    fn range(&self, i: usize) -> (u32, u16, u16) {
        let o = HEADER + i * RANGE;
        let d = self.data;
        let start = u32::from(rd16(d, o)) | (u32::from(rd16(d, o + 2)) << 16);
        (start, rd16(d, o + 4), rd16(d, o + 6))
    }

    /// Look up a code point. `None` if the font does not have it.
    #[must_use]
    pub fn glyph(&self, c: char) -> Option<Glyph<'static>> {
        let cp = u32::from(c);
        let (mut lo, mut hi) = (0usize, usize::from(self.n_ranges));
        while lo < hi {
            let mid = (lo + hi) / 2;
            let (start, count, first) = self.range(mid);
            if cp < start {
                hi = mid;
            } else if cp >= start + u32::from(count) {
                lo = mid + 1;
            } else {
                let gi = usize::from(first) + usize::try_from(cp - start).ok()?;
                return self.glyph_at(gi);
            }
        }
        None
    }

    fn glyph_at(&self, gi: usize) -> Option<Glyph<'static>> {
        let d = self.data;
        let base = HEADER + usize::from(self.n_ranges) * RANGE;
        let o = base + gi * GLYPH;
        let rec = d.get(o..o + GLYPH)?;
        let off = usize::from(rec[0]) | (usize::from(rec[1]) << 8) | (usize::from(rec[2]) << 16);
        let (adv, w, h) = (rec[3], rec[4], rec[5]);
        let x = i8::from_le_bytes([rec[6]]);
        let top = i8::from_le_bytes([rec[7]]);
        let bits_start = base + usize::from(self.n_glyphs) * GLYPH + off;
        let n = (usize::from(w) * usize::from(h) * usize::from(self.bpp)).div_ceil(8);
        let bits = d.get(bits_start..bits_start + n)?;
        Some(Glyph { adv, w, h, x, top, bpp: self.bpp, bits })
    }

    /// Size of the font data in bytes (rodata).
    #[must_use]
    pub const fn size_bytes(&self) -> usize {
        self.data.len()
    }
}

/// UI font: DejaVu Sans Condensed 12 px, 2 bits per pixel.
pub static UI: Font = Font::parse(include_bytes!("../fonts/ui12.yfnt"));
/// UI bold: DejaVu Sans Condensed Bold 12 px (titles, clock).
pub static UI_BOLD: Font = Font::parse(include_bytes!("../fonts/ui12b.yfnt"));
/// Large numerals, 28 px digits (Fira Sans Condensed), 4 bits per pixel.
pub static NUM28: Font = Font::parse(include_bytes!("../fonts/num28.yfnt"));
/// Secondary numerals, 16 px digits (Fira Sans Condensed Medium).
pub static NUM16: Font = Font::parse(include_bytes!("../fonts/num16.yfnt"));

/// Which face to draw with.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FontId {
    #[default]
    Ui,
    UiBold,
    Num28,
    Num16,
}

impl FontId {
    #[must_use]
    pub fn font(self) -> &'static Font {
        match self {
            FontId::Ui => &UI,
            FontId::UiBold => &UI_BOLD,
            FontId::Num28 => &NUM28,
            FontId::Num16 => &NUM16,
        }
    }
}

/// Size of one fallback glyph record: 16 rows of 2 bytes (MSB = leftmost
/// pixel), then one advance-width byte. Same as the vendor `UNICODE.FON`.
pub const FALLBACK_RECORD: usize = 33;

/// Where the 16 px fallback font comes from: on the device a resource on NOR
/// (`record = base + code_point * 33`), in the simulator a file or a subset.
pub trait GlyphSource {
    /// Fill `out` with the record for `cp` (BMP only). Return `false` if the
    /// record cannot be read; an all-zero record means "no glyph".
    fn read(&mut self, cp: u16, out: &mut [u8; FALLBACK_RECORD]) -> bool;

    /// How many of the cell's 16 rows are drawn above the text baseline.
    /// 13 for both GNU Unifont (whose own baseline is row 14; one pixel
    /// lower centres ideographs on Latin capitals and keeps them inside a
    /// 19 px row's focus pill) and the vendor's `UNICODE.FON`.
    fn baseline(&self) -> u8 {
        13
    }
}

/// Number of cached fallback glyphs (05 §4, 02 §7: 64 entries ≈ 2.2 KiB).
pub const CACHE_ENTRIES: usize = 64;

/// RAM cache of fallback glyph records. Lives wherever the firmware likes
/// (a `static` in the UI task); [`Gfx`](crate::Gfx) borrows it while drawing.
pub struct GlyphCache {
    cps: [u16; CACHE_ENTRIES],
    recs: [[u8; FALLBACK_RECORD]; CACHE_ENTRIES],
    hand: u8,
    /// Lookups and misses since creation (Diagnostics).
    pub lookups: u32,
    pub misses: u32,
}

impl Default for GlyphCache {
    fn default() -> Self {
        Self::new()
    }
}

impl GlyphCache {
    #[must_use]
    pub const fn new() -> GlyphCache {
        GlyphCache {
            cps: [0; CACHE_ENTRIES],
            recs: [[0; FALLBACK_RECORD]; CACHE_ENTRIES],
            hand: 0,
            lookups: 0,
            misses: 0,
        }
    }

    /// Fetch the record for `cp`, reading through `src` on a miss. Code
    /// point 0 is never cached (it marks free slots).
    pub fn get(&mut self, src: &mut dyn GlyphSource, cp: u16) -> Option<&[u8; FALLBACK_RECORD]> {
        if cp == 0 {
            return None;
        }
        self.lookups = self.lookups.wrapping_add(1);
        if let Some(i) = self.cps.iter().position(|&c| c == cp) {
            return self.recs.get(i);
        }
        self.misses = self.misses.wrapping_add(1);
        let i = usize::from(self.hand) % CACHE_ENTRIES;
        self.hand = self.hand.wrapping_add(1);
        let rec = self.recs.get_mut(i)?;
        if !src.read(cp, rec) {
            *rec = [0; FALLBACK_RECORD];
        }
        if let Some(slot) = self.cps.get_mut(i) {
            *slot = cp;
        }
        self.recs.get(i)
    }
}

/// Interpret a fallback record as a glyph; `None` if it is empty.
#[must_use]
pub fn fallback_glyph(rec: &[u8; FALLBACK_RECORD], baseline: u8) -> Option<Glyph<'_>> {
    let adv = rec[32];
    if adv == 0 && rec[..32].iter().all(|&b| b == 0) {
        return None;
    }
    let adv = if adv == 0 || adv > 16 { 16 } else { adv };
    let top = i8::try_from(baseline).unwrap_or(14);
    Some(Glyph { adv, w: 16, h: 16, x: 0, top, bpp: 1, bits: &rec[..32] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_fonts_parse() {
        let a = UI.glyph('A').unwrap();
        assert!(a.adv > 4 && a.w > 4 && a.top == 9);
        assert!(UI.glyph('Ж').is_some());
        assert!(UI.glyph('Ω').is_some());
        assert!(UI.glyph('中').is_none());
        assert!(UI_BOLD.glyph('я').is_some());
        let z = NUM28.glyph('0').unwrap();
        let one = NUM28.glyph('1').unwrap();
        assert_eq!(z.adv, one.adv, "numerals are tabular");
        assert!(NUM16.glyph(':').is_some());
        assert_eq!(UI.ascent, 9);
    }

    struct Src(u32);
    impl GlyphSource for Src {
        fn read(&mut self, cp: u16, out: &mut [u8; FALLBACK_RECORD]) -> bool {
            self.0 += 1;
            *out = [0; FALLBACK_RECORD];
            out[0] = 0x80;
            out[32] = if cp > 0x2000 { 16 } else { 8 };
            true
        }
    }

    #[test]
    fn cache_hits() {
        let mut c = GlyphCache::new();
        let mut s = Src(0);
        for _ in 0..3 {
            for cp in 0x4E00..0x4E10u16 {
                assert!(c.get(&mut s, cp).is_some());
            }
        }
        assert_eq!(s.0, 16);
        for cp in 0x5000..0x5100u16 {
            c.get(&mut s, cp);
        }
        assert_eq!(s.0, 16 + 256);
        let rec = *c.get(&mut s, 0x4E2D).unwrap();
        let g = fallback_glyph(&rec, 14).unwrap();
        assert_eq!((g.adv, g.w, g.h, g.top), (16, 16, 16, 14));
    }
}
