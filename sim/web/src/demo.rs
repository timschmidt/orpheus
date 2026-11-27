//! The demo card (WEB-2): real, freely licensed works — public-domain and
//! CC0 recordings of Bach, Beethoven, Chopin, Satie, Sousa and Caruso, a
//! LibriVox reading, a Mutopia MIDI file, paintings, a NASA photograph, a
//! Hubble picture and two Project Gutenberg books — one for every format
//! the browser build opens. `sim/web/demo/sources.toml` pins each by
//! source page, URL and SHA-256 and says what was done to it;
//! `cargo xtask wasm --demo` derives `sim/web/demo/card/` from it
//! (committed, with `CREDITS.txt`). The page fetches the files packed as
//! one bundle ([`crate::bundle`]); host tests read the directory.

use crate::card::File;

/// The tracks every test and the page's self-test rely on.
pub const ARIA: &str = "/Music/Bach/Goldberg Variations/01 Aria.mp3";
/// The FLAC excerpt (lossless, 44.1 kHz stereo).
pub const VARIATION: &str = "/Music/Bach/Goldberg Variations/05 Variatio 4.flac";
/// The MIDI file (the synthesiser plays it).
pub const PRELUDE_MIDI: &str = "/MIDI/Bach - Prelude in C major, BWV 846.mid";
/// The credits on the card.
pub const CREDITS: &str = "/CREDITS.txt";

/// Every file of a card tree, paths relative to `dir` with a leading
/// `/`, sorted (the bundle's order).
pub fn read_tree(dir: &std::path::Path) -> std::io::Result<Vec<File>> {
    fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut Vec<File>) -> std::io::Result<()> {
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if p.is_dir() {
                walk(base, &p, out)?;
            } else {
                let rel = p.strip_prefix(base).map_err(std::io::Error::other)?;
                let path: Vec<String> =
                    rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                out.push(File { path: format!("/{}", path.join("/")), data: std::fs::read(&p)? });
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// The committed demo set (`sim/web/demo/card/`), for host builds and tests.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn committed() -> Vec<File> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("demo/card");
    read_tree(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_set_has_every_format_and_its_credits() {
        let files = committed();
        let has = |ext: &str| files.iter().any(|f| f.path.to_lowercase().ends_with(ext));
        for ext in [
            ".mp3", ".flac", ".ogg", ".opus", ".spx", ".wav", ".aiff", ".au", ".mid", ".lrc", ".m3u8",
            ".jpg", ".png", ".bmp", ".gif", ".qoi", ".epub", ".txt",
        ] {
            assert!(has(ext), "no {ext} file on the demo card");
        }
        for p in [ARIA, VARIATION, PRELUDE_MIDI, CREDITS] {
            assert!(files.iter().any(|f| f.path == p), "{p}");
        }
        let credits = files.iter().find(|f| f.path == CREDITS).unwrap();
        let credits = std::str::from_utf8(&credits.data).unwrap();
        // Every other file is credited (the project's own files too).
        for f in &files {
            if f.path != CREDITS {
                assert!(credits.contains(&f.path[1..]), "{} missing from CREDITS.txt", f.path);
            }
        }
    }

    /// Every picture decodes with the player's decoders (`orpheus-img`
    /// through the simulator's viewer path) to a screen that is not blank.
    #[test]
    fn every_picture_decodes() {
        let pics: Vec<_> = committed().into_iter().filter(|f| f.path.starts_with("/Pictures/")).collect();
        assert_eq!(pics.len(), 5);
        for f in pics {
            let (px, info) = orpheus_sim::pictures::decode(&f.data, 128, 128, false, 0)
                .unwrap_or_else(|e| panic!("{}: {e:?}", f.path));
            assert!(info.width >= 184 && info.height >= 135, "{}: {}x{}", f.path, info.width, info.height);
            let distinct: std::collections::BTreeSet<u16> = px.iter().copied().collect();
            assert!(distinct.len() > 16, "{}: {} colours", f.path, distinct.len());
        }
    }

    #[test]
    fn the_bundle_carries_the_set() {
        let files = committed();
        let b = crate::bundle::encode(&files);
        let back = crate::bundle::decode(&b).unwrap();
        assert_eq!(back.len(), files.len());
        assert!(back.iter().zip(&files).all(|(a, b)| a.path == b.path && a.data == b.data));
    }
}
