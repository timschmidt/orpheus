//! Fallback-font sources for the simulator.
//!
//! * [`HexFont`]: GNU Unifont glyphs in `.hex` format (the committed subset
//!   in `assets/unifont-subset.hex`, or a full `unifont-*.hex` file). Used for
//!   golden images.
//! * [`FonFile`]: a 33-byte-record font file read in place, like the device
//!   reads its NOR resource. Point it at the vendor's `UNICODE.FON` inside the
//!   NOR dump for development (`YOTON_FALLBACK_FON=<file>@<offset>`); never for
//!   golden images, and the data is never committed.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result, bail};
use yoton_ui::font::{FALLBACK_RECORD, GlyphSource};

/// The committed Unifont subset (SIL OFL 1.1, see `assets/OFL-unifont.txt`).
pub const UNIFONT_SUBSET: &str = include_str!("../assets/unifont-subset.hex");

/// Unifont `.hex` glyphs converted to 33-byte records.
pub struct HexFont {
    glyphs: HashMap<u16, [u8; FALLBACK_RECORD]>,
    /// Records read through the `GlyphSource` interface (cache misses).
    pub reads: u32,
}

impl HexFont {
    /// Parse `.hex` text (`XXXX:bits`; `#` lines are comments).
    pub fn parse(text: &str) -> Result<HexFont> {
        let mut glyphs = HashMap::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (cp, bits) = line.split_once(':').with_context(|| format!("line {}", n + 1))?;
            let Ok(cp) = u16::from_str_radix(cp, 16) else { continue };
            glyphs.insert(cp, hex_record(bits).with_context(|| format!("line {}", n + 1))?);
        }
        Ok(HexFont { glyphs, reads: 0 })
    }

    /// The committed subset.
    pub fn subset() -> HexFont {
        // The asset is part of the crate; a parse failure is a build bug.
        HexFont::parse(UNIFONT_SUBSET).unwrap_or(HexFont { glyphs: HashMap::new(), reads: 0 })
    }

    #[must_use]
    pub fn has(&self, cp: u16) -> bool {
        self.glyphs.contains_key(&cp)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }
}

/// One `.hex` bitmap to a 33-byte record: 16 rows of 2 bytes, MSB left, then
/// the advance width.
fn hex_record(bits: &str) -> Result<[u8; FALLBACK_RECORD]> {
    let raw: Vec<u8> = (0..bits.len() / 2)
        .map(|i| u8::from_str_radix(&bits[2 * i..2 * i + 2], 16))
        .collect::<Result<_, _>>()?;
    let mut rec = [0u8; FALLBACK_RECORD];
    match raw.len() {
        16 => {
            for (row, b) in raw.iter().enumerate() {
                rec[2 * row] = *b;
            }
            rec[32] = 8;
        }
        32 => {
            rec[..32].copy_from_slice(&raw);
            rec[32] = 16;
        }
        n => bail!("unsupported glyph size ({n} bytes)"),
    }
    Ok(rec)
}

impl GlyphSource for HexFont {
    fn read(&mut self, cp: u16, out: &mut [u8; FALLBACK_RECORD]) -> bool {
        self.reads += 1;
        *out = self.glyphs.get(&cp).copied().unwrap_or([0; FALLBACK_RECORD]);
        true
    }

    /// Unifont's baseline is row 14 of 16; reporting 13 drops the glyphs
    /// by one pixel so ideographs (which fill the cell and have no
    /// descenders) sit centred on Latin caps and clear of the focus pill's
    /// top edge.
    fn baseline(&self) -> u8 {
        13
    }
}

/// A direct-indexed 33-byte-record font in a file, read on demand
/// (`offset + cp * 33`), exactly as the device reads the NOR resource.
pub struct FonFile {
    f: File,
    offset: u64,
    baseline: u8,
}

impl FonFile {
    pub fn open(path: &Path, offset: u64, baseline: u8) -> Result<FonFile> {
        let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
        Ok(FonFile { f, offset, baseline })
    }

    /// From `YOTON_FALLBACK_FON=<path>@<offset>[@<baseline>]` (offset in
    /// hex with `0x` or decimal; baseline defaults to 13, the vendor font's).
    pub fn from_env() -> Option<FonFile> {
        let v = std::env::var("YOTON_FALLBACK_FON").ok()?;
        let mut parts = v.split('@');
        let path = parts.next()?;
        let off = parts.next().map_or(Some(0), parse_num)?;
        let base = parts.next().map_or(Some(13), parse_num)?;
        FonFile::open(Path::new(path), off, u8::try_from(base).ok()?).ok()
    }
}

fn parse_num(s: &str) -> Option<u64> {
    match s.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

impl GlyphSource for FonFile {
    fn read(&mut self, cp: u16, out: &mut [u8; FALLBACK_RECORD]) -> bool {
        let pos = self.offset + u64::from(cp) * FALLBACK_RECORD as u64;
        self.f.seek(SeekFrom::Start(pos)).is_ok() && self.f.read_exact(out).is_ok()
    }

    fn baseline(&self) -> u8 {
        self.baseline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing() {
        let f = HexFont::parse("0041:0000000018242442427E424242420000\n4E2D:0100010001003FF8210821082108210821083FF8210801000100010001000100\n").unwrap();
        assert!(f.has(0x41) && f.has(0x4E2D));
        let mut f = f;
        let mut rec = [0u8; FALLBACK_RECORD];
        assert!(f.read(0x4E2D, &mut rec));
        assert_eq!(rec[32], 16);
        assert_eq!(&rec[..2], &[0x01, 0x00]);
        assert!(f.read(0x41, &mut rec));
        assert_eq!(rec[32], 8);
        assert_eq!(rec[8], 0x18);
        assert_eq!(rec[10], 0x24);
        assert_eq!(rec[9], 0);
    }
}
