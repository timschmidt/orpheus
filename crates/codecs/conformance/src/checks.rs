//! The per-file conformance checks of 07 §3.3.

use std::path::{Path, PathBuf};

use orpheus_audio::{CodecId, OpenError};

use crate::compare::{self, Diff};
use crate::drive::{self, Cfg, Decoded, Fail};
use crate::rng::Rng;

/// One line of `tests/corpus/manifest.tsv`.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub file: PathBuf,
    pub reference: Option<PathBuf>,
    /// Reference is `s32le` (else `s16le`).
    pub wide: bool,
    /// The decoder must refuse the file (`Unsupported`).
    pub expect_unsupported: bool,
    /// Frames of the source signal, when the decode must reproduce its
    /// length exactly (gapless).
    pub src_frames: Option<u64>,
}

/// Parse the manifest: `file  ref  fmt  expect  src_frames` (tab separated,
/// paths relative to the corpus directory, `-` for none).
pub fn manifest(dir: &Path) -> Result<Vec<Entry>, String> {
    let text = std::fs::read_to_string(dir.join("manifest.tsv")).map_err(|e| {
        format!("{}: {e} (run `cargo xtask test codecs --gen`)", dir.join("manifest.tsv").display())
    })?;
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 5 {
            return Err(format!("bad manifest line: {line}"));
        }
        out.push(Entry {
            name: f[0].to_string(),
            file: dir.join(f[0]),
            reference: (f[1] != "-").then(|| dir.join(f[1])),
            wide: f[2] == "s32",
            expect_unsupported: f[3] == "unsupported",
            src_frames: f[4].parse().ok(),
        });
    }
    Ok(out)
}

/// Arena budgets of 04 §4.1 for the codec this file will use.
#[must_use]
pub fn budget_for(file: &[u8], name: &str) -> usize {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "mp3" | "mp2" | "mp1" => 28 * 1024,
        "ogg" | "oga" => 72 * 1024,
        "flac" => match compare::flac_max_block(file) {
            Some(b) if b <= 4608 => 38 * 1024,
            _ => 66 * 1024,
        },
        "opus" => 64 * 1024,
        "spx" => 12 * 1024,
        _ => 4 * 1024,
    }
}

#[must_use]
pub fn lossy(codec: CodecId) -> bool {
    matches!(
        codec,
        CodecId::Mp1
            | CodecId::Mp2
            | CodecId::Mp3
            | CodecId::Vorbis
            | CodecId::Opus
            | CodecId::Speex
            | CodecId::Aac
    )
}

#[derive(Debug, Clone, Copy)]
pub struct Opts {
    pub robust: bool,
    pub seeks: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Row {
    pub name: String,
    pub info: Option<orpheus_audio::StreamInfo>,
    pub failures: Vec<String>,
    pub reference: String,
    pub md5: Option<bool>,
    pub seek: String,
    pub robust: String,
    pub budget: usize,
    pub arena_plan: usize,
    pub arena_used: usize,
    pub ns_per_frame: f64,
    pub realtime: f64,
}

fn ext(name: &str) -> Option<&str> {
    name.rsplit_once('.').map(|(_, e)| e)
}

fn fail_text(f: &Fail) -> String {
    format!("{f:?}")
}

pub fn check(e: &Entry, opts: Opts) -> Row {
    let mut row = Row { name: e.name.clone(), ..Row::default() };
    let file = match std::fs::read(&e.file) {
        Ok(f) => f,
        Err(err) => {
            row.failures.push(format!("read: {err}"));
            return row;
        }
    };
    let ext = ext(&e.name);
    row.budget = budget_for(&file, &e.name);
    crate::set_context(&format!("{} linear decode", e.name));
    let linear = drive::decode_file(&file, &Cfg::whole(row.budget), ext);
    if e.expect_unsupported {
        match linear {
            Err(Fail::Open(OpenError::Unsupported(u))) => {
                row.reference = format!("refused ({u:?}) as expected");
            }
            Err(Fail::ArenaBudget { need, budget }) => {
                row.reference = format!("refused: arena {need} > {budget} as expected");
            }
            Ok(_) => row.failures.push("decoded a stream that must be refused".into()),
            Err(f) => row.failures.push(format!("expected Unsupported, got {}", fail_text(&f))),
        }
        return row;
    }
    let a = match linear {
        Ok(a) => a,
        Err(f) => {
            row.failures.push(format!("decode: {}", fail_text(&f)));
            return row;
        }
    };
    row.info = Some(a.info);
    row.arena_plan = a.arena_plan;
    row.arena_used = a.arena_used;
    let ch = usize::from(a.info.channels.max(1));
    let frames = a.pcm.len() / ch;
    if a.corrupt > 0 {
        row.failures.push(format!("{} corrupt frames in a clean file", a.corrupt));
    }
    if a.arena_used > row.budget {
        row.failures.push(format!("arena high-water {} over budget {}", a.arena_used, row.budget));
    }
    if let Some(n) = a.info.frames
        && n != frames as u64
    {
        row.failures.push(format!("StreamInfo.frames {n} but decoded {frames}"));
    }
    if frames > 0 {
        row.ns_per_frame = a.decode_ns as f64 / frames as f64;
        row.realtime = (frames as f64 / f64::from(a.info.rate.max(1))) / (a.decode_ns as f64 * 1e-9);
    }
    // Restartability: the device ring and a trickling ring give the same
    // samples as the whole file at once. The Ogg codecs also through the
    // firmware's 16 KiB ring (one-second pages of 10–28 KB do not fit in
    // it: they are read packet by packet).
    let ogg = matches!(a.info.codec, CodecId::Opus | CodecId::Speex | CodecId::Vorbis);
    let firmware = Cfg { ring: 16 * 1024, ..Cfg::device(row.budget) };
    for (label, cfg) in [
        ("device ring", Cfg::device(row.budget)),
        ("trickle ring", Cfg { chunk: 1531, ..Cfg::device(row.budget) }),
        ("firmware ring", firmware),
    ] {
        if label == "firmware ring" && !ogg {
            continue;
        }
        crate::set_context(&format!("{} {label}", e.name));
        match drive::decode_file(&file, &cfg, ext) {
            Ok(b) if b.pcm == a.pcm => {}
            Ok(b) => {
                let d = compare::diff(&b.pcm, &a.pcm, 1.0);
                row.failures
                    .push(format!("{label} differs from linear decode at sample {:?}", d.first_mismatch));
            }
            Err(f) => row.failures.push(format!("{label}: {}", fail_text(&f))),
        }
    }
    let full = if a.wide { 2_147_483_648.0 } else { 32768.0 };
    compare_reference(e, &a, full, &mut row);
    if a.info.codec == CodecId::Flac
        && let Some(want) = compare::flac_streaminfo_md5(&file)
    {
        let ok = compare::flac_md5(&a.pcm, a.wide, a.info.bits) == want;
        row.md5 = Some(ok);
        if !ok {
            row.failures.push("FLAC MD5 mismatch".into());
        }
    }
    if let Some(n) = e.src_frames
        && n != frames as u64
    {
        row.failures.push(format!("gapless: decoded {frames} frames, source has {n}"));
    }
    if opts.seeks > 0 && frames > 0 {
        seek_check(e, &file, &a, opts.seeks, &mut row);
    }
    if opts.robust {
        robust_check(e, &file, &mut row);
    }
    row
}

fn compare_reference(e: &Entry, a: &Decoded, full: f64, row: &mut Row) {
    let Some(rp) = &e.reference else {
        row.reference = "no reference".into();
        return;
    };
    let rb = match std::fs::read(rp) {
        Ok(b) => b,
        Err(err) => {
            row.failures.push(format!("reference: {err}"));
            return;
        }
    };
    if e.wide != a.wide {
        row.failures.push(format!("output width {} but reference {}", a.wide, e.wide));
        return;
    }
    let r = compare::load_raw(&rb, e.wide);
    let d: Diff = compare::diff(&a.pcm, &r, full);
    if d.exact() {
        row.reference = "bit-exact".into();
        return;
    }
    row.reference =
        format!("rms {:.1} dBFS, max {} LSB, len {}/{}", d.rms_db, d.max_abs, d.len_ours, d.len_ref);
    if lossy(a.info.codec) {
        if d.len_ours != d.len_ref {
            row.failures.push(format!("length {} vs reference {} (gapless)", d.len_ours, d.len_ref));
        }
        if d.rms_db > -80.0 {
            row.failures.push(format!("rms difference {:.1} dBFS > -80", d.rms_db));
        }
    } else {
        row.failures.push(format!("not bit-exact: first mismatch at {:?}", d.first_mismatch));
    }
}

fn seek_check(e: &Entry, file: &[u8], a: &Decoded, n: usize, row: &mut Row) {
    let ch = usize::from(a.info.channels.max(1));
    let total = (a.pcm.len() / ch) as u64;
    let mut rng = Rng::new(0x5EED ^ total);
    let mut targets: Vec<u64> = (0..n).map(|_| rng.below(total)).collect();
    targets.push(0);
    targets.push(total.saturating_sub(1));
    targets.push(total);
    let len = 2048;
    crate::set_context(&format!("{} seek", e.name));
    let res = match drive::seek_and_decode(file, &Cfg::device(row.budget), ext(&e.name), &targets, len) {
        Ok(r) => r,
        Err(f) => {
            row.failures.push(format!("seek: {}", fail_text(&f)));
            row.seek = "error".into();
            return;
        }
    };
    let full = if a.wide { 2_147_483_648.0 } else { 32768.0 };
    let (mut exact, mut close, mut converging) = (0usize, 0usize, 0usize);
    let mut worst = f64::NEG_INFINITY;
    for (&t, (landed, pcm)) in targets.iter().zip(&res) {
        if *landed != t {
            row.failures.push(format!("seek to {t} landed at {landed}"));
            continue;
        }
        let start = (t as usize * ch).min(a.pcm.len());
        let want = &a.pcm[start..(start + len * ch).min(a.pcm.len())];
        let d = compare::diff(pcm, want, full);
        if d.exact() {
            exact += 1;
        } else if lossy(a.info.codec) && d.len_ours == d.len_ref && d.rms_db <= -80.0 {
            close += 1;
        } else if a.info.codec == CodecId::ImaQt && d.len_ours == d.len_ref && d.max_abs < 128 {
            // QuickTime IMA: a mid-stream start carries a DC offset below
            // 128 LSB (see the WAV decoder's seek).
            close += 1;
        } else if a.info.codec == CodecId::Opus && d.len_ours == d.len_ref && d.rms_db <= OPUS_SEEK_DB {
            // Opus restarts from a page before the target with a fresh
            // decoder (RFC 7845 §4.6 pre-roll). CELT converges to the
            // linear decode exactly within the pre-roll; SILK's integer
            // LPC/LTP synthesis keeps a residual difference for seconds on
            // stationary tones (measured −57…−78 dBFS on the synthetic
            // corpus; noise-like input converges exactly after ~0.5 s).
            converging += 1;
            worst = worst.max(d.rms_db);
        } else {
            row.failures.push(format!(
                "seek to {t}: output differs from linear decode (first mismatch {:?}, len {}/{}, rms {:.1})",
                d.first_mismatch, d.len_ours, d.len_ref, d.rms_db
            ));
        }
    }
    row.seek = format!(
        "{exact}/{} exact{}{}",
        targets.len(),
        if close > 0 { format!(", {close} close") } else { String::new() },
        if converging > 0 {
            format!(", {converging} converging (worst {worst:.1} dB)")
        } else {
            String::new()
        }
    );
}

/// Seek tolerance for Opus after the pre-roll (see `seek_check`).
const OPUS_SEEK_DB: f64 = -50.0;

/// Truncations, bit flips and splices: the decoder must end (EndOfStream or
/// a clean refusal) within the iteration budget and never break the
/// contract. A panic aborts the run with the case printed by the hook.
fn robust_check(e: &Entry, file: &[u8], row: &mut Row) {
    let mut rng = Rng::new(0xBAD ^ file.len() as u64);
    let len = file.len();
    let mut cases: Vec<(String, Vec<u8>)> = Vec::new();
    for cut in [8, 64, len / 100, len / 3, len / 2, len * 99 / 100, len.saturating_sub(1)] {
        cases.push((format!("truncated at {cut}"), file[..cut.min(len)].to_vec()));
    }
    for k in 0..8 {
        let mut f = file.to_vec();
        let flips = 1 + rng.below(64) as usize;
        for _ in 0..flips {
            let i = rng.below(len as u64) as usize;
            f[i] ^= 1 << rng.below(8);
        }
        cases.push((format!("bit flips #{k} ({flips})"), f));
    }
    for k in 0..2 {
        let mut f = file.to_vec();
        let at = rng.below(len as u64) as usize;
        let junk: Vec<u8> = (0..1000).map(|_| rng.next_u64() as u8).collect();
        f.splice(at..at, junk);
        cases.push((format!("garbage spliced #{k}"), f));
    }
    let mut bad = 0;
    for (label, f) in &cases {
        crate::set_context(&format!("{} robustness: {label}", e.name));
        match drive::decode_file(f, &Cfg::device(row.budget), ext(&e.name)) {
            Ok(_)
            | Err(
                Fail::Probe
                | Fail::Open(_)
                | Fail::ArenaBudget { .. }
                | Fail::Decode(orpheus_audio::DecodeError::Unsupported(_)),
            ) => {}
            Err(f) => {
                bad += 1;
                row.failures.push(format!("robustness {label}: {}", fail_text(&f)));
            }
        }
    }
    row.robust = format!("{}/{} ok", cases.len() - bad, cases.len());
}
