//! Gapless joins (04 §2.2): album tracks decoded back to back, each
//! decoder re-opened in the same arena as the previous one ends — what
//! the firmware does at a track boundary. "Zero inserted samples" means the
//! join has exactly the source's length; lossless joins must equal the
//! source sample for sample.

use std::path::{Path, PathBuf};

use yoton_audio::SliceSource;

use crate::compare;
use crate::drive::{self, Cfg, Next};

#[derive(Debug, Clone)]
pub struct Join {
    pub name: String,
    pub source: PathBuf,
    pub parts: Vec<PathBuf>,
    pub exact: bool,
    pub refs: Vec<PathBuf>,
}

/// `gapless.tsv` of the corpus (empty if absent).
#[must_use]
pub fn manifest(dir: &Path) -> Vec<Join> {
    let Ok(text) = std::fs::read_to_string(dir.join("gapless.tsv")) else { return Vec::new() };
    text.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f.len() >= 5).then(|| Join {
                name: f[0].to_string(),
                source: dir.join(f[1]),
                parts: f[2].split(',').map(|p| dir.join(p)).collect(),
                exact: f[3] == "exact",
                refs: if f[4] == "-" { Vec::new() } else { f[4].split(',').map(|p| dir.join(p)).collect() },
            })
        })
        .collect()
}

/// The firmware's codec arena.
const ARENA: usize = 72 * 1024;

/// Decode `parts` back to back through one arena (device ring per part).
fn decode_joined(parts: &[PathBuf], arena: &mut [u8]) -> Result<(Vec<i32>, u32), String> {
    let mut out = Vec::new();
    let mut corrupt = 0u32;
    for part in parts {
        let file = std::fs::read(part).map_err(|e| format!("{}: {e}", part.display()))?;
        let cfg = Cfg::device(ARENA);
        let mut src = SliceSource::ring(&file, cfg.ring);
        let ext = part.extension().and_then(|e| e.to_str());
        let (format, hint) = drive::probe(&mut src, &cfg, ext).map_err(|e| format!("{e:?}"))?;
        let (mut d, _, _) =
            drive::open(&mut src, &cfg, format, &hint, arena).map_err(|e| format!("{e:?}"))?;
        let mut ns = 0u128;
        let max = drive::budget(file.len());
        for i in 0.. {
            if i > max {
                return Err("no end".into());
            }
            if let Some(Next::End) = drive::step(&mut d, &mut src, &cfg, &mut out, &mut corrupt, &mut ns)
                .map_err(|e| format!("{e:?}"))?
            {
                break;
            }
        }
    }
    Ok((out, corrupt))
}

/// Run one join check; `Ok(summary)` or `Err(failure)`.
pub fn check(j: &Join) -> Result<String, String> {
    let mut arena = vec![0u8; ARENA];
    let (joined, corrupt) = decode_joined(&j.parts, &mut arena)?;
    if corrupt > 0 {
        return Err(format!("{corrupt} corrupt frames"));
    }
    let (source, _) = decode_joined(core::slice::from_ref(&j.source), &mut arena)?;
    if joined.len() != source.len() {
        return Err(format!(
            "joined {} samples, source {} (inserted or lost samples)",
            joined.len(),
            source.len()
        ));
    }
    if j.exact {
        let d = compare::diff(&joined, &source, 32768.0);
        return if d.exact() {
            Ok(format!("{} tracks, {} frames, bit-exact join", j.parts.len(), joined.len() / 2))
        } else {
            Err(format!("join differs from the source at sample {:?}", d.first_mismatch))
        };
    }
    let mut r = Vec::new();
    for p in &j.refs {
        let b = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
        r.extend(compare::load_raw(&b, false));
    }
    let d = compare::diff(&joined, &r, 32768.0);
    if d.len_ours != d.len_ref || d.rms_db > -80.0 {
        return Err(format!(
            "join vs per-track references: len {}/{}, rms {:.1} dBFS",
            d.len_ours, d.len_ref, d.rms_db
        ));
    }
    Ok(format!(
        "{} tracks, {} frames = source length, rms {:.1} dBFS vs references",
        j.parts.len(),
        joined.len() / 2,
        d.rms_db
    ))
}
