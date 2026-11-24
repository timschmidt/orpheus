//! Fuzz-target bodies, shared by the `cargo fuzz` targets (nightly, in
//! `fuzz/`) and the stable smoke run (`conformance fuzz-smoke`).
//!
//! A target panics only for a real bug: a panic inside a decoder, a broken
//! contract (`NeedData` that cannot be satisfied, an empty PCM block), or a
//! stream that does not end within the iteration budget.

use yoton_audio::{Format, SeekStep};

use crate::drive::{self, Cfg, Fail, Next};
use crate::rng::Rng;

/// Ring refill granularities the fuzzer chooses from.
const CHUNKS: [usize; 5] = [0, 1, 7, 509, 4096];

fn cfg(sel: u8, force: Option<Format>) -> Cfg {
    Cfg { ring: 32 * 1024, chunk: CHUNKS[usize::from(sel) % CHUNKS.len()], arena: 72 * 1024, force }
}

fn verdict(r: Result<(), Fail>) {
    match r {
        Ok(()) | Err(Fail::Probe | Fail::Open(_) | Fail::ArenaBudget { .. } | Fail::Decode(_)) => {}
        Err(f @ (Fail::Contract(_) | Fail::NoEnd { .. })) => panic!("decoder bug: {f:?}"),
    }
}

/// Decode `file` to the end; after `seek_after` blocks, seek to `target`.
fn run(file: &[u8], cfg: &Cfg, seek_after: Option<(u32, u64)>) -> Result<(), Fail> {
    let mut src = if cfg.ring == 0 {
        yoton_audio::SliceSource::new(file)
    } else {
        yoton_audio::SliceSource::ring(file, cfg.ring)
    };
    let (format, hint) = drive::probe(&mut src, cfg, None)?;
    let mut arena = vec![0u8; cfg.arena];
    let (mut d, _, _) = drive::open(&mut src, cfg, format, &hint, &mut arena)?;
    let (mut out, mut corrupt, mut ns) = (Vec::new(), 0u32, 0u128);
    let max = drive::budget(file.len());
    let mut blocks = 0u32;
    let mut iters = 0u64;
    loop {
        iters += 1;
        if iters > max {
            return Err(Fail::NoEnd { iters });
        }
        if let Some((after, target)) = seek_after
            && blocks == after
        {
            blocks += 1;
            loop {
                iters += 1;
                if iters > max {
                    return Err(Fail::NoEnd { iters });
                }
                match d.seek(&mut src, target).map_err(Fail::Decode)? {
                    SeekStep::Done(_) => break,
                    SeekStep::NeedData(n) => {
                        if n <= yoton_audio::ByteSource::available(&src) || n > yoton_audio::MAX_PEEK {
                            return Err(Fail::Contract("seek NeedData out of range"));
                        }
                        if yoton_audio::ByteSource::at_end(&src) {
                            return Err(Fail::Contract("seek NeedData at the end"));
                        }
                        drive::refill(&mut src, cfg);
                    }
                }
            }
        }
        out.clear();
        match drive::step(&mut d, &mut src, cfg, &mut out, &mut corrupt, &mut ns)? {
            Some(Next::End) => return Ok(()),
            Some(Next::Pcm) => blocks += 1,
            None => {}
        }
    }
}

/// Probe and decode anything. Byte 0 selects the refill pattern, bytes 1–2
/// an optional seek; the rest is the file.
pub fn decode_any(data: &[u8]) {
    decode_with(data, None);
}

/// Decode with a forced decoder (the decoder's frame entry, whatever the
/// probe would say).
pub fn decode_with(data: &[u8], force: Option<Format>) {
    if data.len() < 3 {
        return;
    }
    let cfg = cfg(data[0], force);
    let s = u16::from_le_bytes([data[1], data[2]]);
    let seek = (s != 0).then(|| (u32::from(s & 7), u64::from(s >> 3) * 1153));
    verdict(run(&data[3..], &cfg, seek));
}

pub fn wav(data: &[u8]) {
    decode_with(data, Some(Format::Wav));
}

pub fn mp3(data: &[u8]) {
    decode_with(data, Some(Format::Mpeg));
}

pub fn flac(data: &[u8]) {
    decode_with(data, Some(Format::Flac));
}

pub fn vorbis(data: &[u8]) {
    decode_with(data, Some(Format::Ogg(yoton_audio::OggCodec::Vorbis)));
}

/// The Ogg packet reader alone, over a simulated ring.
pub fn ogg(data: &[u8]) {
    use yoton_audio::{ByteSource, SliceSource};
    use yoton_ogg::{Next, Reader};
    let Some((&sel, file)) = data.split_first() else { return };
    let chunk = CHUNKS[usize::from(sel) % CHUNKS.len()];
    let mut src = SliceSource::ring(file, 32 * 1024);
    let mut r = Reader::new();
    for _ in 0..drive::budget(file.len()) {
        match r.next(&mut src) {
            Next::Packet(p) => {
                let v = yoton_audio::View::of(&src);
                let _ = p.byte(&v, p.len.saturating_sub(1));
                r.commit(&mut src, &p);
            }
            Next::Need(n) => {
                assert!(n > src.available() && !src.at_end(), "ogg NeedData contract");
                if chunk == 0 { src.refill() } else { src.trickle(chunk) }
            }
            Next::End => return,
            Next::Corrupt => {}
        }
    }
    panic!("ogg reader does not finish");
}

/// Probe only.
pub fn probe(data: &[u8]) {
    let src = yoton_audio::SliceSource::new(data);
    let _ = yoton_audio::probe(&src, Some(b"mp3"));
    let _ = yoton_audio::probe(&src, None);
}

/// A named target body.
pub type Target = (&'static str, fn(&[u8]));

/// Tag reader over a simulated ring: byte 0 picks the container and the
/// refill size. It must finish without breaking the `NeedData` contract
/// and with a bounded number of `Continue`s.
pub fn meta(data: &[u8]) {
    use yoton_audio::{ByteSource, SliceSource};
    use yoton_meta::{MetaStep, Opts, TagReader, Tags};
    let Some((&sel, file)) = data.split_first() else { return };
    let format = [Format::Mpeg, Format::Flac, Format::Wav, Format::Aiff][usize::from(sel & 3)];
    let chunk = CHUNKS[usize::from(sel >> 2) % CHUNKS.len()];
    let mut src = SliceSource::ring(file, 32 * 1024);
    let mut r = TagReader::new(format, Opts::default());
    let mut tags = Tags::default();
    let mut continues = 0usize;
    for _ in 0..drive::budget(file.len()) {
        match r.step(&mut src, &mut tags) {
            Ok(MetaStep::Done) | Err(_) => return,
            Ok(MetaStep::Continue) => {
                continues += 1;
                assert!(continues <= file.len() + 16, "tag reader spins");
            }
            Ok(MetaStep::NeedData(n)) => {
                assert!(n > src.available() && !src.at_end(), "tag reader NeedData contract");
                if chunk == 0 { src.refill() } else { src.trickle(chunk) }
            }
        }
    }
    panic!("tag reader does not finish");
}

/// LRC and SYLT readers over arbitrary bytes.
pub fn lyrics(data: &[u8]) {
    use yoton_audio::SliceSource;
    use yoton_meta::lrc::{LrcReader, LrcStep};
    use yoton_meta::sylt::{SyltReader, SyltStep};
    let mut src = SliceSource::ring(data, 4096);
    let mut r = LrcReader::new(yoton_meta::CodePage::Windows1251);
    for _ in 0..drive::budget(data.len()) {
        match r.next(&mut src) {
            LrcStep::Done => break,
            LrcStep::NeedData(_) => src.trickle(97),
            LrcStep::Line(_) => {}
        }
    }
    let mut src = SliceSource::new(data);
    let mut s = SyltReader::new(data.len() as u32, yoton_meta::CodePage::Latin1);
    for _ in 0..drive::budget(data.len()) {
        if !matches!(s.next(&mut src), SyltStep::Entry(_)) {
            return;
        }
    }
    panic!("SYLT reader does not finish");
}

/// The targets, by name (for the smoke run and the cargo-fuzz wrappers).
pub const TARGETS: &[Target] = &[
    ("decode_any", decode_any),
    ("wav", wav),
    ("mp3", mp3),
    ("flac", flac),
    ("vorbis", vorbis),
    ("ogg", ogg),
    ("probe", probe),
    ("meta", meta),
    ("lyrics", lyrics),
];

/// One mutation of `seed` (bit flips, byte overwrites, truncation, splices
/// of random bytes, duplicated or deleted ranges), with a random 3-byte
/// control prefix.
#[must_use]
pub fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut f = seed.to_vec();
    let rounds = 1 + rng.below(8);
    for _ in 0..rounds {
        let len = f.len().max(1) as u64;
        match rng.below(7) {
            0 => {
                let i = rng.below(len) as usize;
                if let Some(b) = f.get_mut(i) {
                    *b ^= 1 << rng.below(8);
                }
            }
            1 => {
                let i = rng.below(len) as usize;
                if let Some(b) = f.get_mut(i) {
                    *b = rng.next_u64() as u8;
                }
            }
            2 => f.truncate(rng.below(len) as usize),
            3 => {
                let at = rng.below(len) as usize;
                let n = rng.below(256) as usize;
                let junk: Vec<u8> = (0..n).map(|_| rng.next_u64() as u8).collect();
                let at = at.min(f.len());
                f.splice(at..at, junk);
            }
            4 => {
                let a = rng.below(len) as usize;
                let n = (rng.below(2048) as usize).min(f.len().saturating_sub(a));
                let copy: Vec<u8> = f[a..a + n].to_vec();
                let at = rng.below(len) as usize;
                let at = at.min(f.len());
                f.splice(at..at, copy);
            }
            5 => {
                let a = rng.below(len) as usize;
                let n = (rng.below(4096) as usize).min(f.len().saturating_sub(a));
                f.drain(a..a + n);
            }
            _ => {
                // Interesting values in header-sized fields.
                let i = rng.below(len.min(256)) as usize;
                let v = [0u8, 0xFF, 0x7F, 0x80, 1][rng.below(5) as usize];
                if let Some(b) = f.get_mut(i) {
                    *b = v;
                }
            }
        }
    }
    let mut out = vec![rng.next_u64() as u8, rng.next_u64() as u8, rng.next_u64() as u8];
    out.extend_from_slice(&f);
    out
}
