//! `cargo xtask brand [--check]` (BRAND-1, 09 §6): the brand assets from one
//! source.
//!
//! Inputs: `docs/brand/palette.toml` and the lyre `docs/logo/orpheus-lyre.svg`.
//! Outputs (committed, in `docs/brand/generated/`): the mono and small lyre
//! variants, the favicon set, app icons, the 1200 × 630 social card, the
//! wordmark in the firmware's bold bitmap font, and the website's CSS custom
//! properties. Every palette pairing is contrast-checked for the web and for
//! the RGB565 screen. `--check` regenerates in memory and fails if a file on
//! disk differs (the gate's freshness check).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use resvg::{tiny_skia, usvg};
use serde::Deserialize;

use crate::args::Args;
use crate::util::{log, root};

/// The palette file as written.
#[derive(Deserialize)]
struct PaletteFile {
    brand: BTreeMap<String, String>,
    dark: BTreeMap<String, String>,
    light: BTreeMap<String, String>,
}

/// Roles every scheme defines, in CSS order.
pub const ROLES: [&str; 10] =
    ["bg", "surface", "text", "text-dim", "primary", "on-primary", "link", "accent", "warning", "good"];

/// Pairings checked: (foreground, background, minimum ratio, also on the
/// screen). 4.5:1 is AA for text; 3:1 is AA for non-text (focus outlines,
/// badges, the progress colour as a fill).
const PAIRS: [(&str, &str, f64, bool); 12] = [
    ("text", "bg", 4.5, true),
    ("text", "surface", 4.5, true),
    ("text-dim", "bg", 4.5, true),
    ("text-dim", "surface", 4.5, true),
    ("on-primary", "primary", 4.5, true),
    ("warning", "bg", 4.5, true),
    ("good", "bg", 4.5, false),
    ("link", "bg", 4.5, false),
    ("link", "surface", 4.5, false),
    ("accent", "bg", 4.5, false),
    ("primary", "bg", 3.0, false),
    ("accent", "surface", 3.0, false),
];

/// A parsed palette: brand colours and the two schemes.
pub struct Palette {
    pub brand: BTreeMap<String, [u8; 3]>,
    pub dark: BTreeMap<String, [u8; 3]>,
    pub light: BTreeMap<String, [u8; 3]>,
}

impl Palette {
    fn scheme(&self, dark: bool) -> &BTreeMap<String, [u8; 3]> {
        if dark { &self.dark } else { &self.light }
    }
}

fn parse_hex(s: &str) -> Result<[u8; 3]> {
    let h = s.strip_prefix('#').filter(|h| h.len() == 6).ok_or_else(|| anyhow!("`{s}` is not #rrggbb"))?;
    let v = u32::from_str_radix(h, 16).map_err(|_| anyhow!("`{s}` is not #rrggbb"))?;
    Ok([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

pub fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

pub fn parse_palette(text: &str) -> Result<Palette> {
    let f: PaletteFile = toml::from_str(text).context("palette.toml")?;
    let conv = |m: &BTreeMap<String, String>, what: &str| -> Result<BTreeMap<String, [u8; 3]>> {
        m.iter()
            .map(|(k, v)| Ok((k.clone(), parse_hex(v).with_context(|| format!("[{what}] {k}"))?)))
            .collect()
    };
    let p = Palette {
        brand: conv(&f.brand, "brand")?,
        dark: conv(&f.dark, "dark")?,
        light: conv(&f.light, "light")?,
    };
    for (name, s) in [("dark", &p.dark), ("light", &p.light)] {
        for r in ROLES {
            if !s.contains_key(r) {
                bail!("[{name}] lacks `{r}`");
            }
        }
        if let Some(k) = s.keys().find(|k| !ROLES.contains(&k.as_str())) {
            bail!("[{name}] has unknown role `{k}`");
        }
    }
    for k in ["lyre", "on-lyre"] {
        if !p.brand.contains_key(k) {
            bail!("[brand] lacks `{k}`");
        }
    }
    Ok(p)
}

/// WCAG 2.x relative luminance of an sRGB colour.
pub fn luminance(c: [u8; 3]) -> f64 {
    let ch = |v: u8| {
        let s = f64::from(v) / 255.0;
        if s <= 0.039_28 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2])
}

pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// The colour as the screen shows it: truncated to RGB565 and expanded
/// again (the firmware's `rgb` and `to_rgb888`).
pub fn on_screen(c: [u8; 3]) -> [u8; 3] {
    orpheus_ui::color::to_rgb888(orpheus_ui::color::rgb(c[0], c[1], c[2]))
}

/// Every failing pairing, as text. Empty when the palette passes.
pub fn contrast_failures(p: &Palette) -> Vec<String> {
    let mut out = Vec::new();
    for dark in [false, true] {
        let s = p.scheme(dark);
        let name = if dark { "dark" } else { "light" };
        for (fg, bg, min, screen) in PAIRS {
            let (f, b) = (s[fg], s[bg]);
            let web = contrast(f, b);
            if web < min {
                out.push(format!("{name}: {fg} on {bg} is {web:.2}:1 on the web (needs {min}:1)"));
            }
            if screen {
                let scr = contrast(on_screen(f), on_screen(b));
                if scr < min {
                    out.push(format!(
                        "{name}: {fg} on {bg} is {scr:.2}:1 on the RGB565 screen (needs {min}:1)"
                    ));
                }
            }
        }
    }
    out
}

/// How the palette maps onto the firmware's theme tables (README.md):
/// palette role → firmware role, for `Theme::Dark`/`Theme::Light` with
/// `Accent::Blue`.
pub const FIRMWARE_MAP: [(&str, orpheus_ui::theme::Role); 8] = {
    use orpheus_ui::theme::Role;
    [
        ("bg", Role::Bg),
        ("surface", Role::Surface),
        ("text", Role::Text),
        ("text-dim", Role::TextDim),
        ("primary", Role::Accent),
        ("on-primary", Role::OnAccent),
        ("warning", Role::Warning),
        ("good", Role::Good),
    ]
};

/// Roles whose RGB565 value differs from the firmware's current theme
/// table (reported, not enforced: the switch-over is a follow-up).
pub fn firmware_differences(p: &Palette) -> Vec<String> {
    use orpheus_ui::theme::{Accent, Theme};
    let mut out = Vec::new();
    for (dark, theme) in [(true, Theme::Dark), (false, Theme::Light)] {
        let fw = theme.palette(Accent::Blue);
        for (role, fw_role) in FIRMWARE_MAP {
            let c = p.scheme(dark)[role];
            let mine = orpheus_ui::color::rgb(c[0], c[1], c[2]);
            if mine != fw.get(fw_role) {
                out.push(format!("{theme:?}.{fw_role:?} ({role})"));
            }
        }
    }
    out
}

// ---------------------------------------------------------------- the lyre

const LOGO: &str = "docs/logo/orpheus-lyre.svg";
const PALETTE: &str = "docs/brand/palette.toml";
pub const GENERATED: &str = "docs/brand/generated";

/// The source lyre, split: the tile and the drawing on it.
struct Lyre {
    tile: String,
    body: String,
}

fn split_lyre(svg: &str, p: &Palette) -> Result<Lyre> {
    let lyre = hex(p.brand["lyre"]);
    let on = hex(p.brand["on-lyre"]);
    let tile = format!(r#"<rect x="0" y="0" width="240" height="240" rx="60" fill="{lyre}"/>"#);
    let at = svg.find(&tile).ok_or_else(|| {
        anyhow!("{LOGO}: the tile `{tile}` was not found (the logo and palette.toml [brand] lyre must agree)")
    })?;
    let end = svg.rfind("</svg>").ok_or_else(|| anyhow!("{LOGO}: no </svg>"))?;
    let body = svg[at + tile.len()..end].trim().to_string();
    if !body.contains(&format!(r#"stroke="{on}""#)) || !body.contains(&format!(r#"fill="{on}""#)) {
        bail!("{LOGO}: the drawing is not in [brand] on-lyre {on}");
    }
    Ok(Lyre { tile, body })
}

const SVG_HEAD: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 240 240" width="240" height="240" role="img" aria-labelledby="t">"#;
const GEN_NOTE: &str =
    "<!-- Generated by `cargo xtask brand` from docs/logo/orpheus-lyre.svg; do not edit. -->";

/// One colour, no tile: `currentColor` takes the surrounding text colour
/// when inlined; as a file it draws black.
fn lyre_mono(l: &Lyre, p: &Palette) -> String {
    let body = l.body.replace(&hex(p.brand["on-lyre"]), "currentColor");
    format!("{SVG_HEAD}\n  <title id=\"t\">Orpheus</title>\n  {GEN_NOTE}\n  {body}\n</svg>\n")
}

/// For 16–48 px: a tighter tile and heavier strokes so the strings survive.
fn lyre_small(l: &Lyre) -> String {
    let tile = l.tile.replace(r#"rx="60""#, r#"rx="48""#);
    let body = l
        .body
        .replace(r#"stroke-width="1""#, r#"stroke-width="1.6""#)
        .replace(r#"stroke-width="2""#, r#"stroke-width="2.6""#);
    format!(
        "{SVG_HEAD}\n  <title id=\"t\">Orpheus</title>\n  {GEN_NOTE}\n  {tile}\n  <g transform=\"translate(120 120) scale(1.1) translate(-120 -120)\">\n  {body}\n  </g>\n</svg>\n"
    )
}

/// The full lyre on a square tile (app icons: the platform rounds them).
fn lyre_square(l: &Lyre) -> String {
    let tile = l.tile.replace(r#" rx="60""#, "");
    format!("{SVG_HEAD}\n  <title id=\"t\">Orpheus</title>\n  {tile}\n  {}\n</svg>\n", l.body)
}

// ------------------------------------------------------------- rasterising

pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.set_compression(png::Compression::High);
        let mut wr = e.write_header()?;
        wr.write_image_data(rgba)?;
    }
    Ok(out)
}

fn render_svg(svg: &str, w: u32, h: u32) -> Result<Vec<u8>> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).context("parse generated SVG")?;
    let mut pm = tiny_skia::Pixmap::new(w, h).ok_or_else(|| anyhow!("pixmap {w}x{h}"))?;
    let size = tree.size();
    let t = tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
    resvg::render(&tree, t, &mut pm.as_mut());
    encode_png(w, h, &pm.take_demultiplied())
}

/// An ICO holding PNG images (Vista and later read these).
fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, data) in images {
        let s = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[s, s, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in images {
        out.extend_from_slice(data);
    }
    out
}

// ------------------------------------------------- text in the bitmap font

/// SVG rectangles drawing `text` in a firmware font, one rectangle per run
/// of equal pixels, at `scale` SVG units per pixel. Returns the markup and
/// the width in SVG units. `(x, baseline)` place the pen.
pub fn pixel_text(
    font: &orpheus_ui::font::Font,
    text: &str,
    scale: f64,
    x: f64,
    baseline: f64,
    crisp: bool,
) -> (String, f64) {
    let mut out = String::new();
    let mut pen = 0i32;
    for ch in text.chars() {
        let Some(g) = font.glyph(ch) else {
            pen += i32::from(font.ascent) / 2;
            continue;
        };
        let max = (1u32 << g.bpp) - 1;
        let px = |col: usize, row: usize| -> u32 {
            let bit = (row * usize::from(g.w) + col) * usize::from(g.bpp);
            let byte = g.bits[bit / 8];
            let shift = 8 - usize::from(g.bpp) - (bit % 8);
            let raw = (u32::from(byte) >> shift) & max;
            if crisp && raw > 0 { if raw * 2 >= max { max } else { 0 } } else { raw }
        };
        for row in 0..usize::from(g.h) {
            let mut col = 0;
            while col < usize::from(g.w) {
                let v = px(col, row);
                if v == 0 {
                    col += 1;
                    continue;
                }
                let start = col;
                while col < usize::from(g.w) && px(col, row) == v {
                    col += 1;
                }
                let rx = x + f64::from(pen + i32::from(g.x) + start as i32) * scale;
                let ry = baseline + f64::from(row as i32 - i32::from(g.top)) * scale;
                let rw = (col - start) as f64 * scale;
                let op = if v == max {
                    String::new()
                } else {
                    format!(r#" fill-opacity="{:.2}""#, f64::from(v) / f64::from(max))
                };
                let _ = write!(
                    out,
                    r#"<rect x="{}" y="{}" width="{}" height="{}"{op}/>"#,
                    num(rx),
                    num(ry),
                    num(rw),
                    num(scale)
                );
            }
        }
        pen += i32::from(g.adv);
    }
    (out, f64::from(pen) * scale)
}

/// A number for SVG: integers without a fraction, otherwise two decimals.
fn num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 { format!("{}", v.round() as i64) } else { format!("{v:.2}") }
}

/// "Orpheus" in the firmware's bold UI font, `currentColor`, for the site
/// header (inlined) and documents.
fn wordmark() -> String {
    let f = &orpheus_ui::font::UI_BOLD;
    let top = f64::from(f.ascent) + 1.0;
    let (rects, w) = pixel_text(f, "Orpheus", 1.0, 0.0, top, true);
    let h = top + f64::from(f.descent);
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w} {h}\" role=\"img\" aria-label=\"Orpheus\" shape-rendering=\"crispEdges\" fill=\"currentColor\">{rects}</svg>\n",
        w = num(w),
        h = num(h)
    )
}

fn social_card(l: &Lyre, p: &Palette) -> String {
    let d = &p.dark;
    let (bg, text, dim, link) = (hex(d["bg"]), hex(d["text"]), hex(d["text-dim"]), hex(d["link"]));
    let bold = &orpheus_ui::font::UI_BOLD;
    let ui = &orpheus_ui::font::UI;
    let x0 = 520.0;
    let (word, _) = pixel_text(bold, "Orpheus", 11.0, x0, 250.0, true);
    let (l1, _) = pixel_text(ui, "Pure-Rust firmware for", 4.0, x0, 345.0, false);
    let (l2, _) = pixel_text(ui, "the Yoton YM03 player", 4.0, x0, 410.0, false);
    let (url, _) = pixel_text(ui, "timschmidt.github.io/orpheus", 3.0, x0, 520.0, false);
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1200 630" width="1200" height="630">
  <rect width="1200" height="630" fill="{bg}"/>
  <g transform="translate(110 135) scale(1.5)">{tile}{body}</g>
  <g fill="{text}" shape-rendering="crispEdges">{word}</g>
  <g fill="{dim}" shape-rendering="crispEdges">{l1}{l2}</g>
  <g fill="{link}" shape-rendering="crispEdges">{url}</g>
</svg>
"#,
        tile = l.tile,
        body = l.body,
    )
}

// --------------------------------------------------------------------- CSS

fn css(p: &Palette) -> String {
    let vars = |dark: bool| -> String {
        let s = p.scheme(dark);
        let mut v = String::new();
        for r in ROLES {
            let _ = writeln!(v, "  --o-{r}: {};", hex(s[r]));
        }
        v
    };
    let brand: String = p.brand.iter().map(|(k, c)| format!("  --o-{k}: {};\n", hex(*c))).collect();
    format!(
        "/* Generated by `cargo xtask brand` from docs/brand/palette.toml; do not edit. */\n\
         :root {{\n  color-scheme: light dark;\n{brand}{light}}}\n\
         @media (prefers-color-scheme: dark) {{\n  :root:not([data-theme=\"light\"]) {{\n{dark_in}  }}\n}}\n\
         :root[data-theme=\"dark\"] {{\n  color-scheme: dark;\n{dark}}}\n\
         :root[data-theme=\"light\"] {{\n  color-scheme: light;\n}}\n",
        light = vars(false),
        dark = vars(true),
        dark_in = vars(true).lines().map(|l| format!("  {l}\n")).collect::<String>(),
    )
}

// ----------------------------------------------------------------- command

/// Every generated file: path relative to the repository root, contents.
pub fn generate() -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let r = root();
    let palette = parse_palette(&std::fs::read_to_string(r.join(PALETTE)).context(PALETTE)?)?;
    let fails = contrast_failures(&palette);
    if !fails.is_empty() {
        bail!("palette contrast:\n  {}", fails.join("\n  "));
    }
    let logo = std::fs::read_to_string(r.join(LOGO)).context(LOGO)?;
    let lyre = split_lyre(&logo, &palette)?;
    let small = lyre_small(&lyre);
    let square = lyre_square(&lyre);
    let g = Path::new(GENERATED);
    let mut out: Vec<(PathBuf, Vec<u8>)> = vec![
        (g.join("orpheus-lyre-mono.svg"), lyre_mono(&lyre, &palette).into_bytes()),
        (g.join("orpheus-lyre-small.svg"), small.clone().into_bytes()),
        (g.join("wordmark.svg"), wordmark().into_bytes()),
        (g.join("palette.css"), css(&palette).into_bytes()),
    ];
    let mut icos = Vec::new();
    for s in [16, 32, 48] {
        let png = render_svg(&small, s, s)?;
        out.push((g.join(format!("favicon-{s}.png")), png.clone()));
        icos.push((s, png));
    }
    out.push((g.join("favicon.ico"), ico(&icos)));
    out.push((g.join("apple-touch-icon.png"), render_svg(&square, 180, 180)?));
    out.push((g.join("icon-192.png"), render_svg(&logo, 192, 192)?));
    out.push((g.join("icon-512.png"), render_svg(&logo, 512, 512)?));
    out.push((g.join("social-card.png"), render_svg(&social_card(&lyre, &palette), 1200, 630)?));
    Ok(out)
}

/// The paths of generated files that differ from what is on disk.
pub fn stale() -> Result<Vec<String>> {
    let r = root();
    let files = generate()?;
    let mut bad: Vec<String> = files
        .iter()
        .filter(|(p, data)| std::fs::read(r.join(p)).ok().as_deref() != Some(data.as_slice()))
        .map(|(p, _)| p.display().to_string())
        .collect();
    // Files in the directory that the generator no longer makes.
    if let Ok(rd) = std::fs::read_dir(r.join(GENERATED)) {
        for e in rd.flatten() {
            let p = Path::new(GENERATED).join(e.file_name());
            if !files.iter().any(|(f, _)| *f == p) {
                bad.push(format!("{} (not generated any more)", p.display()));
            }
        }
    }
    Ok(bad)
}

pub fn cmd(args: &mut Args) -> Result<String> {
    if args.flag("check") {
        let bad = stale()?;
        if !bad.is_empty() {
            bail!(
                "generated brand assets are out of date (run `cargo xtask brand`):\n  {}",
                bad.join("\n  ")
            );
        }
        return Ok("palette contrast passes; generated brand assets are current".into());
    }
    let r = root();
    let files = generate()?;
    std::fs::create_dir_all(r.join(GENERATED))?;
    let mut written = 0;
    for (p, data) in &files {
        let full = r.join(p);
        if std::fs::read(&full).ok().as_deref() != Some(data.as_slice()) {
            std::fs::write(&full, data).with_context(|| full.display().to_string())?;
            log(&format!("wrote {}", p.display()));
            written += 1;
        }
    }
    for e in std::fs::read_dir(r.join(GENERATED))?.flatten() {
        let p = Path::new(GENERATED).join(e.file_name());
        if !files.iter().any(|(f, _)| *f == p) {
            std::fs::remove_file(e.path())?;
            log(&format!("removed {}", p.display()));
        }
    }
    let palette = parse_palette(&std::fs::read_to_string(r.join(PALETTE))?)?;
    let diff = firmware_differences(&palette);
    if diff.is_empty() {
        log("firmware Dark/Light (Blue) themes equal the palette (RGB565)");
    } else {
        log(&format!("firmware themes differ from the palette in: {}", diff.join(", ")));
    }
    Ok(format!("{} files, {written} rewritten", files.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Palette {
        parse_palette(&std::fs::read_to_string(root().join(PALETTE)).unwrap()).unwrap()
    }

    #[test]
    fn contrast_reference_values() {
        assert!((contrast([0, 0, 0], [255, 255, 255]) - 21.0).abs() < 1e-9);
        assert!((contrast([1, 2, 3], [1, 2, 3]) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn committed_palette_passes_and_matches_the_firmware() {
        let p = palette();
        assert_eq!(contrast_failures(&p), Vec::<String>::new());
        assert_eq!(firmware_differences(&p), Vec::<String>::new());
    }

    #[test]
    fn weak_pairing_is_reported_for_web_and_screen() {
        let mut p = palette();
        p.dark.insert("text-dim".into(), [40, 44, 52]);
        let f = contrast_failures(&p);
        assert!(f.iter().any(|s| s.contains("text-dim on bg") && s.contains("web")), "{f:?}");
        assert!(f.iter().any(|s| s.contains("text-dim on bg") && s.contains("screen")), "{f:?}");
    }

    #[test]
    fn palette_parse_errors() {
        assert!(parse_hex("#12345").is_err());
        assert!(parse_hex("123456").is_err());
        assert_eq!(parse_hex("#0a0B0c").unwrap(), [10, 11, 12]);
        let base = std::fs::read_to_string(root().join(PALETTE)).unwrap();
        assert!(parse_palette(&base.replace("good = \"#48cc78\"", "")).is_err());
        assert!(
            parse_palette(&base.replace("good = \"#48cc78\"", "good = \"#48cc78\"\nshiny = \"#000000\""))
                .is_err()
        );
    }

    #[test]
    fn logo_colour_must_match_the_palette() {
        let mut p = palette();
        let logo = std::fs::read_to_string(root().join(LOGO)).unwrap();
        assert!(split_lyre(&logo, &p).is_ok());
        p.brand.insert("lyre".into(), [1, 2, 3]);
        assert!(split_lyre(&logo, &p).is_err());
    }

    #[test]
    fn ico_header() {
        let i = ico(&[(16, vec![1, 2]), (256, vec![3])]);
        assert_eq!(&i[..6], &[0, 0, 1, 0, 2, 0]);
        assert_eq!(i[6], 16);
        assert_eq!(i[22], 0); // 256 is written as 0
        assert_eq!(u32::from_le_bytes(i[18..22].try_into().unwrap()), 38);
        assert_eq!(&i[38..], &[1, 2, 3]);
    }

    #[test]
    fn pixel_text_draws_and_measures() {
        let (r, w) = pixel_text(&orpheus_ui::font::UI_BOLD, "O", 2.0, 0.0, 20.0, false);
        assert!(r.contains("<rect"));
        assert!(w > 0.0);
        let (r, w) = pixel_text(&orpheus_ui::font::UI_BOLD, " ", 2.0, 0.0, 20.0, true);
        assert!(r.is_empty());
        assert!(w > 0.0);
    }
}
