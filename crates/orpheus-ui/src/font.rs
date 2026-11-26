//! Bitmap fonts: the built-in faces (format `YFN1`, see `fontgen/fontgen.py`)
//! and the 16 × 16 fallback font read on demand from a [`GlyphSource`].

/// A built-in bitmap font. The data is `'static` rodata (paged on the
/// device); nothing is copied to RAM.
///
/// A `YFN1` file larger than [`PIECE_MAX`] is linked as two statics
/// (`builtin_font!`): `data`, the header, ranges, glyph records and the
/// first bitmaps, and `tail`, the bitmaps from the last glyph boundary at
/// or below [`PIECE_MAX`] on. On the player a rodata section must fit one
/// 16 KiB group of the rodata window to be paged (02 §3.3); a larger one
/// was linked resident (2026-11-26: the 17,794 B UI font was 17.8 KB of
/// the 80 KiB resident image).
#[derive(Clone, Copy, Debug)]
pub struct Font {
    data: &'static [u8],
    tail: &'static [u8],
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
        Font::parse_split(data, &[])
    }

    /// Parse a `YFN1` blob linked in two pieces: `data` (at least the
    /// header, ranges and glyph records) and `tail`, the rest of the file.
    /// A glyph whose bitmap straddles the two is not found
    /// ([`split_point`] cuts where none does).
    #[must_use]
    pub const fn parse_split(data: &'static [u8], tail: &'static [u8]) -> Font {
        assert!(data.len() >= HEADER);
        assert!(data[0] == b'Y' && data[1] == b'F' && data[2] == b'N' && data[3] == b'1');
        let n_ranges = rd16(data, 8);
        let n_glyphs = rd16(data, 10);
        assert!(data.len() >= index_len(data));
        Font {
            data,
            tail,
            bpp: data[4],
            ascent: data[5],
            descent: data[6],
            line: data[7],
            n_ranges,
            n_glyphs,
        }
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
        let bits = if bits_start + n <= d.len() {
            d.get(bits_start..bits_start + n)?
        } else {
            let o = bits_start.checked_sub(d.len())?;
            self.tail.get(o..o + n)?
        };
        Some(Glyph { adv, w, h, x, top, bpp: self.bpp, bits })
    }

    /// Size of the font data in bytes (rodata, both pieces).
    #[must_use]
    pub const fn size_bytes(&self) -> usize {
        self.data.len() + self.tail.len()
    }

    /// The sizes of the two pieces as linked (`data`, `tail`).
    #[must_use]
    pub const fn piece_bytes(&self) -> (usize, usize) {
        (self.data.len(), self.tail.len())
    }
}

/// The largest piece a built-in font is linked in: one rodata group of the
/// player's paged rodata window (`orpheus_pager::RO_GROUP_SIZE`, 02 §3.3).
/// `xtask pagelink` refuses a larger rodata section that no rule places.
pub const PIECE_MAX: usize = 16 * 1024;

/// Header, ranges and glyph records of a `YFN1` file: what the first piece
/// must hold.
#[must_use]
pub const fn index_len(d: &[u8]) -> usize {
    HEADER + rd16(d, 8) as usize * RANGE + rd16(d, 10) as usize * GLYPH
}

/// Where a `YFN1` file is cut into its two pieces: its length when it fits
/// [`PIECE_MAX`], else the largest offset ≤ [`PIECE_MAX`] that no glyph's
/// bitmap straddles. Fails the build if that would cut the index.
#[must_use]
pub const fn split_point(d: &[u8]) -> usize {
    if d.len() <= PIECE_MAX {
        return d.len();
    }
    let base = HEADER + rd16(d, 8) as usize * RANGE;
    let n_glyphs = rd16(d, 10) as usize;
    let bits = base + n_glyphs * GLYPH;
    let mut cut = PIECE_MAX;
    // Lowering the cut to a glyph's start can put it inside another glyph
    // (shared or overlapping bitmaps): repeat until none straddles it.
    let mut moved = true;
    while moved {
        moved = false;
        let mut g = 0;
        while g < n_glyphs {
            let o = base + g * GLYPH;
            let off = d[o] as usize | (d[o + 1] as usize) << 8 | (d[o + 2] as usize) << 16;
            let n = (d[o + 4] as usize * d[o + 5] as usize * d[4] as usize).div_ceil(8);
            let start = bits + off;
            if start < cut && start + n > cut {
                cut = start;
                moved = true;
            }
            g += 1;
        }
    }
    assert!(cut >= bits, "a built-in font's index does not fit one piece");
    cut
}

/// Bytes `from..from + N` of `d` as an array (a built-in font's piece).
#[must_use]
pub const fn piece<const N: usize>(d: &[u8], from: usize) -> [u8; N] {
    let mut out = [0u8; N];
    let mut i = 0;
    while i < N {
        out[i] = d[from + i];
        i += 1;
    }
    out
}

/// A built-in [`Font`] from a `YFN1` file, linked as two statics of at
/// most [`PIECE_MAX`] bytes each (`HEAD`, `TAIL`; the tail is empty when
/// the file fits one piece), so that each can be paged on the player.
macro_rules! builtin_font {
    ($file:literal) => {{
        const RAW: &[u8] = include_bytes!($file);
        const SPLIT: usize = $crate::font::split_point(RAW);
        static HEAD: [u8; SPLIT] = $crate::font::piece(RAW, 0);
        static TAIL: [u8; RAW.len() - SPLIT] = $crate::font::piece(RAW, SPLIT);
        $crate::font::Font::parse_split(&HEAD, &TAIL)
    }};
}

/// UI font: DejaVu Sans Condensed 12 px, 2 bits per pixel. 17,794 B, so
/// two pieces (`builtin_font!`).
pub static UI: Font = builtin_font!("../fonts/ui12.yfnt");
/// UI bold: DejaVu Sans Condensed Bold 12 px (titles, clock).
pub static UI_BOLD: Font = builtin_font!("../fonts/ui12b.yfnt");
/// Large numerals, 28 px digits (Fira Sans Condensed), 4 bits per pixel.
pub static NUM28: Font = builtin_font!("../fonts/num28.yfnt");
/// Secondary numerals, 16 px digits (Fira Sans Condensed Medium).
pub static NUM16: Font = builtin_font!("../fonts/num16.yfnt");

/// Reading fonts (Books, 05 §7.15): DejaVu Sans Condensed 16 px and 20
/// px, 2 bits per pixel, Latin, Latin-1, Extended-A, Cyrillic and
/// punctuation (450 glyphs each; anything else comes from the UI font,
/// then the fallback font). The 16 px face is also the **Large** UI text
/// (A11Y-2; 14,468 B of rodata, paged on the player: under the 16 KiB
/// group size); the 20 px face only with feature `book-fonts` (20,244 B:
/// two pieces, paged like the UI font).
pub static BOOK16: Font = builtin_font!("../fonts/book16.yfnt");
#[cfg(feature = "book-fonts")]
pub static BOOK20: Font = builtin_font!("../fonts/book20.yfnt");

/// How a character is drawn ([`Gfx::char_styled`](crate::Gfx::char_styled),
/// and the large UI faces): pixel-scaled (1 or 2), emboldened (drawn
/// again a pixel to the right, one pixel wider) and slanted (italic
/// emphasis without an italic face).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Style {
    pub scale: u8,
    pub bold: bool,
    pub slant: bool,
}

impl Default for Style {
    fn default() -> Self {
        Style { scale: 1, bold: false, slant: false }
    }
}

/// UI text size (Settings › Accessibility › Text size, A11Y-2; 05 §7.14
/// "large text"): lists, their headers, menus, dialogs, the strip and Now
/// Playing draw their text in [`FontId::sized`] faces.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TextSize {
    /// The 12 px UI font.
    #[default]
    Normal,
    /// The 16 px face ([`BOOK16`]; bold drawn emboldened).
    Large,
    /// The UI font at twice the size (24 px, pixel-doubled): the spec's
    /// "UI font at twice the size". Where a line has a fixed height that
    /// cannot hold it (the strip, dialogs) [`TextSize::Large`] is used.
    ExtraLarge,
}

impl TextSize {
    /// From the setting's value (0 Normal, 1 Large, 2 Extra large).
    #[must_use]
    pub const fn from_setting(v: i16) -> TextSize {
        match v {
            1 => TextSize::Large,
            2 => TextSize::ExtraLarge,
            _ => TextSize::Normal,
        }
    }

    /// At most `Large` (lines of a fixed height: the strip, dialogs).
    #[must_use]
    pub const fn at_most_large(self) -> TextSize {
        match self {
            TextSize::ExtraLarge => TextSize::Large,
            s => s,
        }
    }
}

/// Which face to draw with.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FontId {
    #[default]
    Ui,
    UiBold,
    Num28,
    Num16,
    /// Large text (A11Y-2): the 16 px face.
    Large,
    /// Large text, bold: the 16 px face emboldened.
    LargeBold,
    /// Extra large text: the UI font at twice the size.
    XLarge,
    /// Extra large text, bold: the bold UI font at twice the size.
    XLargeBold,
}

impl FontId {
    /// The face's glyph data (a scaled face: its 1× data).
    #[must_use]
    pub fn font(self) -> &'static Font {
        match self {
            FontId::Ui | FontId::XLarge => &UI,
            FontId::UiBold | FontId::XLargeBold => &UI_BOLD,
            FontId::Num28 => &NUM28,
            FontId::Num16 => &NUM16,
            FontId::Large | FontId::LargeBold => &BOOK16,
        }
    }

    /// How the face's glyphs are drawn: scale and emboldening.
    #[must_use]
    pub const fn style(self) -> Style {
        match self {
            FontId::LargeBold => Style { scale: 1, bold: true, slant: false },
            FontId::XLarge | FontId::XLargeBold => Style { scale: 2, bold: false, slant: false },
            _ => Style { scale: 1, bold: false, slant: false },
        }
    }

    /// The UI face (`Ui` or `UiBold`) at text size `size`; other faces
    /// unchanged.
    #[must_use]
    pub const fn sized(self, size: TextSize) -> FontId {
        match (self, size) {
            (FontId::Ui, TextSize::Large) => FontId::Large,
            (FontId::UiBold, TextSize::Large) => FontId::LargeBold,
            (FontId::Ui, TextSize::ExtraLarge) => FontId::XLarge,
            (FontId::UiBold, TextSize::ExtraLarge) => FontId::XLargeBold,
            (f, _) => f,
        }
    }

    /// Cap height as drawn (baseline to the top of `H`).
    #[must_use]
    pub fn ascent(self) -> i16 {
        i16::from(self.font().ascent) * i16::from(self.style().scale)
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

    #[cfg(feature = "book-fonts")]
    #[test]
    fn book_fonts_parse() {
        for (f, asc) in [(&BOOK16, 12), (&BOOK20, 15)] {
            assert_eq!(f.ascent, asc);
            assert!(f.glyph('Ж').is_some() && f.glyph('é').is_some() && f.glyph('—').is_some());
            assert!(f.glyph('Ω').is_none(), "Greek from the UI font");
            assert!(f.glyph('m').unwrap().adv > UI.glyph('m').unwrap().adv);
        }
    }

    /// A11Y-2: the large faces and their sizes.
    #[test]
    fn sized_faces() {
        assert_eq!(FontId::Ui.sized(TextSize::Normal), FontId::Ui);
        assert_eq!(FontId::UiBold.sized(TextSize::Large), FontId::LargeBold);
        assert_eq!(FontId::Ui.sized(TextSize::ExtraLarge), FontId::XLarge);
        assert_eq!(FontId::Num28.sized(TextSize::ExtraLarge), FontId::Num28);
        assert_eq!(FontId::Ui.ascent(), 9);
        assert_eq!(FontId::Large.ascent(), 12);
        assert_eq!(FontId::XLarge.ascent(), 18, "twice the UI font");
        assert_eq!(TextSize::from_setting(2).at_most_large(), TextSize::Large);
        assert!(BOOK16.glyph('Ж').is_some() && BOOK16.size_bytes() < 16 * 1024);
    }

    extern crate std;
    use std::{vec, vec::Vec};

    /// Every face as built in, with the whole file it came from.
    fn builtin() -> Vec<(&'static str, &'static Font, &'static [u8])> {
        #[allow(unused_mut)]
        let mut v: Vec<(&'static str, &'static Font, &'static [u8])> = vec![
            ("ui12", &UI, include_bytes!("../fonts/ui12.yfnt")),
            ("ui12b", &UI_BOLD, include_bytes!("../fonts/ui12b.yfnt")),
            ("num28", &NUM28, include_bytes!("../fonts/num28.yfnt")),
            ("num16", &NUM16, include_bytes!("../fonts/num16.yfnt")),
            ("book16", &BOOK16, include_bytes!("../fonts/book16.yfnt")),
        ];
        #[cfg(feature = "book-fonts")]
        v.push(("book20", &BOOK20, include_bytes!("../fonts/book20.yfnt")));
        v
    }

    /// Bug 2026-11-26 (PG-1, resident image): the 17,794 B UI font was one
    /// rodata section larger than the player's 16 KiB rodata group, so
    /// `xtask pagelink` linked it resident ("larger than a rodata group"):
    /// 17.8 KB of the 80 KiB resident image, 45 B left. Every built-in
    /// face is now linked in pieces of at most [`PIECE_MAX`] bytes, and
    /// every glyph of every face is the same as the whole file's.
    #[test]
    fn regress_ui_font_resident_as_one_oversized_section() {
        for (name, f, raw) in builtin() {
            let (head, tail) = f.piece_bytes();
            assert!(head <= PIECE_MAX && tail <= PIECE_MAX, "{name}: pieces {head} + {tail} B");
            assert_eq!(head + tail, raw.len(), "{name}");
            assert_eq!(f.size_bytes(), raw.len(), "{name}");
            assert_eq!(tail == 0, raw.len() <= PIECE_MAX, "{name}: split only when needed");
            let whole = Font::parse(raw);
            let mut n = 0;
            for cp in 0..0x1_0000u32 {
                let Some(c) = char::from_u32(cp) else { continue };
                let (a, b) = (f.glyph(c), whole.glyph(c));
                assert_eq!(a.is_some(), b.is_some(), "{name}: U+{cp:04X}");
                if let (Some(a), Some(b)) = (a, b) {
                    assert_eq!((a.adv, a.w, a.h, a.x, a.top, a.bpp), (b.adv, b.w, b.h, b.x, b.top, b.bpp));
                    assert_eq!(a.bits, b.bits, "{name}: U+{cp:04X}");
                    n += 1;
                }
            }
            assert_eq!(n, usize::from(whole.n_glyphs), "{name}: every glyph found");
        }
        let (head, tail) = UI.piece_bytes();
        assert!(head > index_len(include_bytes!("../fonts/ui12.yfnt")) && tail > 0, "{head} + {tail}");
    }

    /// The cut never falls inside a glyph's bitmap, also when bitmaps are
    /// shared or overlap (the cut moves down until none straddles it).
    #[test]
    fn split_point_respects_glyphs() {
        // Header, one range of 3 glyphs (1 bpp, 8 × 8 = 8 B each), records,
        // bitmaps at file offsets 44 (the first), PIECE_MAX - 10 (inside
        // the first piece) and PIECE_MAX - 4 (across PIECE_MAX). Cutting
        // below the third puts the cut inside the second: a second pass.
        let mut d = vec![0u8; 12 + 8 + 3 * 8];
        d[..4].copy_from_slice(b"YFN1");
        d[4] = 1;
        d[8] = 1; // ranges
        d[10] = 3; // glyphs
        d[12..20].copy_from_slice(&[0x41, 0, 0, 0, 3, 0, 0, 0]);
        let bits = d.len();
        let at = |o: usize| [(o & 0xFF) as u8, (o >> 8 & 0xFF) as u8, (o >> 16) as u8, 8, 8, 8, 0, 8];
        d[20..28].copy_from_slice(&at(0));
        d[28..36].copy_from_slice(&at(PIECE_MAX - bits - 10));
        d[36..44].copy_from_slice(&at(PIECE_MAX - bits - 4));
        d.resize(PIECE_MAX + 200, 0);
        assert_eq!(split_point(&d), PIECE_MAX - 10, "below both glyphs");
        assert_eq!(split_point(&d[..PIECE_MAX]), PIECE_MAX, "fits: not cut");
    }

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
