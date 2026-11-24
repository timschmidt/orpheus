//! Gapless joins (04 §2.2): album tracks decoded back to back, each
//! decoder re-opened in the same arena as the previous one ends — what
//! the firmware does at a track boundary. "Zero inserted samples" means the
//! join has exactly the source's length; lossless joins must equal the
//! source sample for sample.
//!
//! [`decode_gapless`] is the firmware's hand-over itself: one 16 KiB
//! [`StreamRing`] read ahead into the next file once the current one has
//! been read to its end, the next decoder opened on the bytes already in
//! the ring (probe, tag skip, inspect, open) in the same arena. Each join
//! must come out of it sample for sample as from the per-part decode.

use std::path::{Path, PathBuf};

use orpheus_audio::ring::Target;
use orpheus_audio::{
    ByteSource, ContainerInfo, DecodeError, OpenError, PcmBlock, Probe, SliceSource, Step, StreamRing,
};

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

/// The firmware's stream ring (`fw/firmware/src/audio.rs`).
pub const DEVICE_RING: usize = 16 * 1024;

/// The firmware's reader: where each file is read.
struct Reader<'d> {
    files: &'d [&'d [u8]],
    /// Index of the current file; read positions of the current and the
    /// next one.
    cur: usize,
    pos: [usize; 2],
}

impl Reader<'_> {
    /// `Ring::fill`: read while 512 bytes are free, into the current file
    /// until its end, then into the next one. True if anything was read.
    fn fill(&mut self, r: &mut StreamRing<'_>) -> bool {
        let mut any = false;
        while let Some(sp) = r.space(512, 16384) {
            let (k, data) = match sp.target {
                Target::Current => (0, self.files[self.cur]),
                Target::Next => (1, self.files[self.cur + 1]),
            };
            if let Some(p) = sp.seek {
                self.pos[k] = usize::try_from(p).unwrap_or(usize::MAX).min(data.len());
            }
            let n = sp.buf.len().min(data.len() - self.pos[k]);
            sp.buf[..n].copy_from_slice(&data[self.pos[k]..self.pos[k] + n]);
            let t = sp.target;
            self.pos[k] += n;
            if n == 0 {
                r.end(t);
            } else {
                r.wrote(t, n);
                any = true;
            }
        }
        any
    }
}

/// Decode `files` (with their extensions) as the firmware plays them
/// gapless: see the module documentation. Returns the samples (widened to
/// `i32`) and the corrupt-frame count.
pub fn decode_gapless(
    files: &[&[u8]],
    exts: &[Option<&str>],
    arena: &mut [u8],
) -> Result<(Vec<i32>, u32), String> {
    let mut buf = vec![0u8; DEVICE_RING];
    let mut ring = StreamRing::new(&mut buf);
    let mut rd = Reader { files, cur: 0, pos: [0, 0] };
    let mut out = Vec::new();
    let mut corrupt = 0u32;
    ring.reset(files.first().map(|f| f.len() as u64));
    for i in 0..files.len() {
        if i > 0 {
            // The previous decoder ended: the next stream is current.
            if !ring.hand_over() {
                return Err(format!("part {i}: not read ahead"));
            }
            rd.cur = i;
            rd.pos = [rd.pos[1], 0];
        }
        let ext = exts.get(i).copied().flatten().map(str::as_bytes);
        rd.fill(&mut ring);
        let mut tries = 0u32;
        let format = loop {
            tries += 1;
            if tries > 10_000 {
                return Err(format!("part {i}: probe does not end"));
            }
            match orpheus_audio::probe(&ring, ext) {
                Probe::Found(f) => break f,
                Probe::Skip(n) => {
                    if usize::try_from(n).is_ok_and(|n| n <= ring.available()) {
                        ring.consume(n as usize);
                    } else {
                        ring.skip_to(ring.position() + n);
                        rd.fill(&mut ring);
                    }
                }
                Probe::NeedData(_) if !ring.eof() => {
                    rd.fill(&mut ring);
                }
                _ => return Err(format!("part {i}: unknown format")),
            }
        };
        let hint = ContainerInfo { format, gapless: None, stream_len: None };
        let plan = loop {
            match orpheus_codecs::inspect(format, &ring, &hint) {
                Ok(p) => break p,
                Err(OpenError::NeedData(_)) if !ring.eof() => {
                    if !rd.fill(&mut ring) {
                        return Err(format!("part {i}: inspect needs a larger ring"));
                    }
                }
                Err(e) => return Err(format!("part {i}: {e:?}")),
            }
        };
        let dec_mem = arena.get_mut(..plan.arena).ok_or("arena too small")?;
        let (mut d, _) =
            orpheus_codecs::open(format, &mut ring, dec_mem, &hint).map_err(|e| format!("{e:?}"))?;
        let mut prefetched = false;
        let max = drive::budget(files[i].len());
        for it in 0.. {
            if it > max {
                return Err(format!("part {i}: no end"));
            }
            match d.decode(&mut ring) {
                Ok(Step::Pcm(b)) => {
                    match b {
                        PcmBlock::I16(s) => out.extend(s.iter().map(|&x| i32::from(x))),
                        PcmBlock::I32(s) => out.extend_from_slice(s),
                    }
                    if ring.available() < DEVICE_RING / 2 {
                        rd.fill(&mut ring);
                    }
                }
                Ok(Step::NeedData(_)) => {
                    if ring.eof() {
                        break;
                    }
                    if !rd.fill(&mut ring) {
                        return Err(format!("part {i}: stream needs a larger ring"));
                    }
                }
                Ok(Step::EndOfStream) => break,
                Err(DecodeError::Corrupt) => corrupt += 1,
                Err(e) => return Err(format!("part {i}: {e:?}")),
            }
            // `ReaderAtEnd` → `Prefetch`: read on into the next file.
            if ring.eof() && !prefetched && i + 1 < files.len() {
                prefetched = true;
                ring.begin_next(Some(files[i + 1].len() as u64));
            }
        }
        if !prefetched && i + 1 < files.len() {
            ring.begin_next(Some(files[i + 1].len() as u64));
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
    // The firmware's hand-over gives the same samples.
    let bytes: Vec<Vec<u8>> = j
        .parts
        .iter()
        .map(|p| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display())))
        .collect::<Result<_, _>>()?;
    let files: Vec<&[u8]> = bytes.iter().map(Vec::as_slice).collect();
    let exts: Vec<Option<&str>> = j.parts.iter().map(|p| p.extension().and_then(|e| e.to_str())).collect();
    let (ring, _) = decode_gapless(&files, &exts, &mut arena)?;
    if ring != joined {
        return Err(format!(
            "the firmware's gapless hand-over differs: {} samples vs {} (first difference at {:?})",
            ring.len(),
            joined.len(),
            ring.iter().zip(&joined).position(|(a, b)| a != b)
        ));
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
