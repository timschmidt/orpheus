//! Drive a decoder over a file the way the pipeline will: probe (skipping
//! leading tags), inspect, lease an arena, open, decode frame by frame
//! through a simulated stream ring, checking the decoder contract on every
//! call.

use yoton_audio::probe::Probe;
use yoton_audio::{
    ByteSource, ContainerInfo, DecodeError, Format, MAX_PEEK, OpenError, PcmBlock, SeekSource, SeekStep,
    SliceSource, Step, StreamInfo,
};
use yoton_codecs::AnyDecoder;

#[derive(Debug, Clone, Copy)]
pub struct Cfg {
    /// Simulated ring capacity in bytes; 0: the whole file is available.
    pub ring: usize,
    /// Bytes added per refill; 0: fill the ring completely.
    pub chunk: usize,
    /// Arena budget in bytes (the lease the pipeline would give).
    pub arena: usize,
    /// Force this format instead of probing (fuzzing a decoder directly).
    pub force: Option<Format>,
}

impl Cfg {
    #[must_use]
    pub fn whole(arena: usize) -> Self {
        Self { ring: 0, chunk: 0, arena, force: None }
    }
    /// The device ring: 32 KiB, refilled in 4 KiB card-read bursts.
    #[must_use]
    pub fn device(arena: usize) -> Self {
        Self { ring: 32 * 1024, chunk: 4096, arena, force: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail {
    /// The probe did not recognise the stream.
    Probe,
    Open(OpenError),
    /// The stream needs more arena than the budget.
    ArenaBudget {
        need: usize,
        budget: usize,
    },
    /// The decoder broke the interface contract.
    Contract(&'static str),
    /// No end of stream within the iteration budget.
    NoEnd {
        iters: u64,
    },
    Decode(DecodeError),
}

#[derive(Debug, Clone, Default)]
pub struct Decoded {
    pub info: StreamInfo,
    /// Samples, interleaved, widened to `i32` (I16 output keeps its scale).
    pub pcm: Vec<i32>,
    pub wide: bool,
    pub arena_plan: usize,
    pub arena_used: usize,
    pub corrupt: u32,
    pub iters: u64,
    /// Nanoseconds spent inside `decode` calls.
    pub decode_ns: u128,
}

pub fn refill(src: &mut SliceSource<'_>, cfg: &Cfg) {
    if cfg.chunk == 0 { src.refill() } else { src.trickle(cfg.chunk) }
}

fn source<'d>(file: &'d [u8], cfg: &Cfg) -> SliceSource<'d> {
    if cfg.ring == 0 { SliceSource::new(file) } else { SliceSource::ring(file, cfg.ring) }
}

/// Probe, skipping leading ID3v2 tags. Returns the format and the hint.
pub fn probe(
    src: &mut SliceSource<'_>,
    cfg: &Cfg,
    ext: Option<&str>,
) -> Result<(Format, ContainerInfo), Fail> {
    if let Some(f) = cfg.force {
        return Ok((f, ContainerInfo { format: f, ..ContainerInfo::default() }));
    }
    for _ in 0..10_000 {
        match yoton_audio::probe(src, ext.map(str::as_bytes)) {
            Probe::Found(f) => return Ok((f, ContainerInfo { format: f, ..ContainerInfo::default() })),
            Probe::Skip(n) => {
                let to = src.position().saturating_add(n);
                if src.seek(to).is_err() {
                    return Err(Fail::Probe);
                }
                refill(src, cfg);
            }
            Probe::NeedData(_) if !src.at_end() => refill(src, cfg),
            Probe::NeedData(_) | Probe::Unknown => return Err(Fail::Probe),
        }
    }
    Err(Fail::Probe)
}

/// Inspect and open; returns the decoder and plan size.
pub fn open<'a>(
    src: &mut SliceSource<'_>,
    cfg: &Cfg,
    format: Format,
    hint: &ContainerInfo,
    arena: &'a mut [u8],
) -> Result<(AnyDecoder<'a>, StreamInfo, usize), Fail> {
    let mut tries = 0u32;
    let plan = loop {
        match yoton_codecs::inspect(format, src, hint) {
            Ok(p) => break p,
            Err(OpenError::NeedData(n)) => {
                if n <= src.available() || n > MAX_PEEK {
                    return Err(Fail::Contract("inspect NeedData out of range"));
                }
                if src.at_end() || tries > 100_000 {
                    return Err(Fail::Open(OpenError::NeedData(n)));
                }
                tries += 1;
                refill(src, cfg);
            }
            Err(e) => return Err(Fail::Open(e)),
        }
    };
    if plan.arena > cfg.arena {
        return Err(Fail::ArenaBudget { need: plan.arena, budget: cfg.arena });
    }
    let (d, info) = yoton_codecs::open(format, src, arena, hint).map_err(Fail::Open)?;
    if info != plan.info {
        return Err(Fail::Contract("open info differs from inspect"));
    }
    Ok((d, info, plan.arena))
}

/// Append a block to `out`.
fn push(out: &mut Vec<i32>, b: &PcmBlock<'_>) {
    match b {
        PcmBlock::I16(s) => out.extend(s.iter().map(|&x| i32::from(x))),
        PcmBlock::I32(s) => out.extend_from_slice(s),
    }
}

pub enum Next {
    Pcm,
    End,
}

/// One decode call with contract checks; appends PCM to `out`.
pub fn step(
    d: &mut AnyDecoder<'_>,
    src: &mut SliceSource<'_>,
    cfg: &Cfg,
    out: &mut Vec<i32>,
    corrupt: &mut u32,
    ns: &mut u128,
) -> Result<Option<Next>, Fail> {
    let t = std::time::Instant::now();
    let r = d.decode(src);
    *ns += t.elapsed().as_nanos();
    match r {
        Ok(Step::Pcm(b)) => {
            if b.is_empty() {
                return Err(Fail::Contract("empty PCM block"));
            }
            push(out, &b);
            Ok(Some(Next::Pcm))
        }
        Ok(Step::NeedData(n)) => {
            if n <= src.available() {
                return Err(Fail::Contract("NeedData(n) with n bytes already buffered"));
            }
            if n > MAX_PEEK {
                return Err(Fail::Contract("NeedData above MAX_PEEK"));
            }
            if src.at_end() {
                return Err(Fail::Contract("NeedData at the end of the stream"));
            }
            refill(src, cfg);
            Ok(None)
        }
        Ok(Step::EndOfStream) => Ok(Some(Next::End)),
        Err(DecodeError::Corrupt) => {
            *corrupt += 1;
            Ok(None)
        }
        Err(e) => Err(Fail::Decode(e)),
    }
}

/// Iteration budget for a file of `len` bytes.
#[must_use]
pub fn budget(len: usize) -> u64 {
    200_000 + 8 * len as u64
}

/// Decode a whole file.
pub fn decode_file(file: &[u8], cfg: &Cfg, ext: Option<&str>) -> Result<Decoded, Fail> {
    let mut src = source(file, cfg);
    let (format, hint) = probe(&mut src, cfg, ext)?;
    let mut arena = vec![0u8; cfg.arena];
    let (mut d, info, plan) = open(&mut src, cfg, format, &hint, &mut arena)?;
    let mut out = Decoded {
        info,
        wide: info.sample == yoton_audio::SampleKind::I32,
        arena_plan: plan,
        ..Decoded::default()
    };
    let max = budget(file.len());
    loop {
        out.iters += 1;
        if out.iters > max {
            return Err(Fail::NoEnd { iters: out.iters });
        }
        if let Some(Next::End) =
            step(&mut d, &mut src, cfg, &mut out.pcm, &mut out.corrupt, &mut out.decode_ns)?
        {
            break;
        }
    }
    out.arena_used = d.arena_used();
    Ok(out)
}

/// Open a file, seek to each target in turn and decode `len` frames after
/// it. Returns `(landed frame, samples)` per target.
pub fn seek_and_decode(
    file: &[u8],
    cfg: &Cfg,
    ext: Option<&str>,
    targets: &[u64],
    frames: usize,
) -> Result<Vec<(u64, Vec<i32>)>, Fail> {
    let mut src = source(file, cfg);
    let (format, hint) = probe(&mut src, cfg, ext)?;
    let mut arena = vec![0u8; cfg.arena];
    let (mut d, info, _) = open(&mut src, cfg, format, &hint, &mut arena)?;
    let ch = usize::from(info.channels);
    let mut res = Vec::new();
    let max = budget(file.len());
    for &t in targets {
        let mut iters = 0u64;
        let landed = loop {
            iters += 1;
            if iters > max {
                return Err(Fail::NoEnd { iters });
            }
            match d.seek(&mut src, t).map_err(Fail::Decode)? {
                SeekStep::Done(f) => break f,
                SeekStep::NeedData(n) => {
                    if n <= src.available() || n > MAX_PEEK {
                        return Err(Fail::Contract("seek NeedData out of range"));
                    }
                    if src.at_end() {
                        return Err(Fail::Contract("seek NeedData at the end"));
                    }
                    refill(&mut src, cfg);
                }
            }
        };
        let mut pcm = Vec::new();
        let (mut corrupt, mut ns) = (0u32, 0u128);
        while pcm.len() < frames * ch {
            iters += 1;
            if iters > max {
                return Err(Fail::NoEnd { iters });
            }
            if let Some(Next::End) = step(&mut d, &mut src, cfg, &mut pcm, &mut corrupt, &mut ns)? {
                break;
            }
        }
        pcm.truncate(frames * ch);
        res.push((landed, pcm));
    }
    Ok(res)
}
