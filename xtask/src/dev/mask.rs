//! Screenshot comparison with masks for live content (VER-7, 07 §5.7).
//!
//! `tests/suite/masks.toml` names rectangles per screen (the `ui top`
//! name; `"*"` applies to every screen): the clock, battery and indicators
//! in the status bar, playback time, signal strength, card-dependent lists.
//! Pixels inside a mask are counted but not compared; outside, a pixel
//! differs when a colour channel (RGB888) differs by more than `channel`,
//! and a shot passes with at most `pixels` differing pixels.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

pub const W: usize = 128;
pub const H: usize = 128;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub x: u8,
    pub y: u8,
    pub w: u8,
    pub h: u8,
    /// What lives there (shown in the report).
    pub why: String,
}

impl Rect {
    fn contains(&self, x: usize, y: usize) -> bool {
        let (rx, ry) = (usize::from(self.x), usize::from(self.y));
        x >= rx && y >= ry && x < rx + usize::from(self.w) && y < ry + usize::from(self.h)
    }
}

/// Masks and tolerance of one screen, or of one shot.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Masks {
    #[serde(default)]
    pub rects: Vec<Rect>,
    /// Differing pixels allowed outside the masks.
    pub pixels: Option<u32>,
    /// Per-channel difference (RGB888) still counted as equal.
    pub channel: Option<u8>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaskFile {
    #[serde(default)]
    pub screen: BTreeMap<String, Masks>,
}

impl MaskFile {
    pub fn load(path: &Path) -> Result<Self> {
        let t = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let f: MaskFile = toml::from_str(&t).with_context(|| format!("parse {}", path.display()))?;
        for (name, m) in &f.screen {
            for r in &m.rects {
                if r.w == 0
                    || r.h == 0
                    || usize::from(r.x) + usize::from(r.w) > W
                    || usize::from(r.y) + usize::from(r.h) > H
                {
                    bail!("{}: screen {name}: rectangle {r:?} outside the 128 × 128 panel", path.display());
                }
            }
        }
        Ok(f)
    }

    /// What applies to a shot on `screen`: `"*"`, the screen's own masks,
    /// then the shot's (`extra`; its tolerance wins). `defaults = false`
    /// drops the file's masks (a shot that checks the status bar).
    pub fn for_shot(&self, screen: &str, extra: Option<&Masks>, defaults: bool) -> Masks {
        let mut m = Masks::default();
        let mut add = |s: &Masks| {
            m.rects.extend(s.rects.iter().cloned());
            m.pixels = s.pixels.or(m.pixels);
            m.channel = s.channel.or(m.channel);
        };
        if defaults {
            if let Some(s) = self.screen.get("*") {
                add(s);
            }
            if let Some(s) = self.screen.get(screen) {
                add(s);
            }
        }
        if let Some(s) = extra {
            add(s);
        }
        m
    }
}

/// Result of comparing one shot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Compared {
    /// Pixels outside the masks that differ.
    pub differing: u32,
    /// Pixels inside masks (not compared).
    pub masked: u32,
    /// Bounding box of the differing pixels (x0, y0, x1, y1).
    pub bbox: Option<(usize, usize, usize, usize)>,
    pub allowed: u32,
}

impl Compared {
    pub fn pass(&self) -> bool {
        self.differing <= self.allowed
    }
}

pub fn rgb(p: u16) -> [u8; 3] {
    orpheus_host::image::rgb565_to_rgb888(p)
}

/// Compare `got` with `want` (RGB565 frames) under `m`; also returns the
/// diff image (RGB888): equal pixels faded, masked ones blue-grey,
/// differing ones magenta.
pub fn compare(got: &[u16], want: &[u16], m: &Masks) -> (Compared, Vec<u8>) {
    let ch = m.channel.unwrap_or(0);
    let mut c = Compared { allowed: m.pixels.unwrap_or(0), ..Compared::default() };
    let mut img = Vec::with_capacity(W * H * 3);
    for i in 0..W * H {
        let (x, y) = (i % W, i / W);
        let (a, b) = (rgb(got.get(i).copied().unwrap_or(0)), rgb(want.get(i).copied().unwrap_or(0)));
        if m.rects.iter().any(|r| r.contains(x, y)) {
            c.masked += 1;
            img.extend(b.iter().map(|v| v / 6 + 30).zip([0u8, 10, 50]).map(|(v, t)| v.saturating_add(t)));
            continue;
        }
        if a.iter().zip(b).any(|(p, q)| p.abs_diff(q) > ch) {
            c.differing += 1;
            c.bbox = Some(match c.bbox {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
            img.extend([255, 0, 255]);
        } else {
            img.extend(b.iter().map(|v| v / 4 + 40));
        }
    }
    (c, img)
}

/// RGB888 to PNG bytes.
pub fn png_rgb(rgb: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, W as u32, H as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(rgb)?;
    }
    Ok(out)
}

/// RGB565 frame to PNG bytes.
pub fn png_frame(fb: &[u16]) -> Result<Vec<u8>> {
    let bytes: Vec<u8> = fb.iter().flat_map(|p| p.to_le_bytes()).collect();
    orpheus_host::image::rgb565_png_bytes(&bytes, W as u32, H as u32)
}

/// A PNG (as written by [`png_frame`]) back to RGB565.
pub fn read_frame(path: &Path) -> Result<Vec<u16>> {
    let (w, h, rgb) = orpheus_host::image::read_png_rgb(path)?;
    if (w as usize, h as usize) != (W, H) {
        bail!("{}: {w} × {h}, expected 128 × 128", path.display());
    }
    Ok(rgb.as_chunks::<3>().0.iter().map(|p| orpheus_ui::color::rgb(p[0], p[1], p[2])).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: u8, y: u8, w: u8, h: u8) -> Rect {
        Rect { x, y, w, h, why: "t".into() }
    }

    #[test]
    fn masks_hide_live_content_only() {
        let want = vec![0x1234u16; W * H];
        let mut got = want.clone();
        // A changed clock digit inside the status-bar mask …
        got[3 * W + 10] = 0xFFFF;
        let m = Masks { rects: vec![rect(0, 0, 128, 14)], ..Masks::default() };
        let (c, img) = compare(&got, &want, &m);
        assert_eq!((c.differing, c.masked), (0, 128 * 14));
        assert!(c.pass());
        assert_eq!(img.len(), W * H * 3);
        // … and one outside it fails, located by the bounding box.
        got[40 * W + 91] = 0xFFFF;
        let (c, img) = compare(&got, &want, &m);
        assert_eq!((c.differing, c.bbox), (1, Some((91, 40, 91, 40))));
        assert!(!c.pass());
        assert_eq!(&img[(40 * W + 91) * 3..][..3], &[255, 0, 255]);
        // A pixel tolerance lets it through; so does a channel tolerance
        // for a small colour change.
        assert!(compare(&got, &want, &Masks { pixels: Some(1), ..m.clone() }).0.pass());
        got[40 * W + 91] = 0x1234 + 1;
        assert!(!compare(&got, &want, &m).0.pass());
        assert!(compare(&got, &want, &Masks { channel: Some(8), ..m }).0.pass());
    }

    #[test]
    fn shot_masks_combine() {
        let f: MaskFile = toml::from_str(
            r#"
            [screen."*"]
            rects = [{ x = 0, y = 0, w = 128, h = 14, why = "status bar" }]
            [screen.NowPlaying]
            rects = [{ x = 0, y = 90, w = 128, h = 20, why = "time" }]
            pixels = 4
            "#,
        )
        .unwrap();
        let m = f.for_shot("NowPlaying", None, true);
        assert_eq!((m.rects.len(), m.pixels), (2, Some(4)));
        assert_eq!(f.for_shot("Home", None, true).rects.len(), 1);
        let extra = Masks { rects: vec![rect(0, 20, 10, 10)], pixels: Some(0), channel: None };
        let m = f.for_shot("NowPlaying", Some(&extra), true);
        assert_eq!((m.rects.len(), m.pixels), (3, Some(0)));
        assert_eq!(f.for_shot("NowPlaying", Some(&extra), false).rects.len(), 1);
    }

    #[test]
    fn committed_masks_load() {
        let f = MaskFile::load(&crate::util::root().join("tests/suite/masks.toml")).unwrap();
        assert!(f.screen.contains_key("*"));
    }

    #[test]
    fn png_round_trip() {
        let fb: Vec<u16> = (0..W * H).map(|i| (i * 2654435761usize) as u16).collect();
        let dir = std::env::temp_dir().join(format!("orpheus-mask-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f.png");
        std::fs::write(&p, png_frame(&fb).unwrap()).unwrap();
        assert_eq!(read_frame(&p).unwrap(), fb);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
