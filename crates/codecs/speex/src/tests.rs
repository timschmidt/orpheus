use std::vec::Vec;

use super::*;
use orpheus_audio::SliceSource;

fn decode_all(file: &[u8], ring: usize, chunk: usize) -> (StreamInfo, Vec<i16>, usize) {
    let mut src = if ring == 0 { SliceSource::new(file) } else { SliceSource::ring(file, ring) };
    let hint = ContainerInfo::default();
    let plan = loop {
        match SpeexDecoder::inspect(&src, &hint) {
            Ok(p) => break p,
            Err(OpenError::NeedData(n)) => {
                assert!(n > src.available() && n <= MAX_PEEK && !src.at_end());
                src.trickle(chunk);
            }
            Err(e) => panic!("{e:?}"),
        }
    };
    let mut arena = std::vec![0u8; plan.arena];
    let (mut d, info) = SpeexDecoder::open(&mut src, &mut arena, &hint).unwrap();
    assert_eq!(info, plan.info);
    let mut out = Vec::new();
    for _ in 0..1_000_000 {
        match d.decode(&mut src) {
            Ok(Step::Pcm(PcmBlock::I16(s))) => {
                assert!(!s.is_empty());
                out.extend_from_slice(s);
            }
            Ok(Step::Pcm(PcmBlock::I32(_))) => panic!("i32 output"),
            Ok(Step::NeedData(n)) => {
                assert!(n > src.available() && !src.at_end() && n <= MAX_PEEK);
                if chunk == 0 { src.refill() } else { src.trickle(chunk) }
            }
            Ok(Step::EndOfStream) => return (info, out, d.arena_used()),
            Err(e) => panic!("{e:?}"),
        }
    }
    panic!("no end");
}

/// RMS difference in dBFS.
fn rms_db(a: &[i16], b: &[i16]) -> f64 {
    let n = a.len().max(1) as f64;
    let s: f64 = a.iter().zip(b).map(|(&x, &y)| (f64::from(x) - f64::from(y)).powi(2)).sum();
    10.0 * (s / n).max(1e-30).log10() - 20.0 * 32768f64.log10()
}

/// Decode `SPX_IN` into raw `s16le` at `SPX_OUT` (a development aid).
#[test]
#[ignore = "development aid: needs SPX_IN and SPX_OUT"]
fn dump() {
    let input = std::env::var("SPX_IN").unwrap();
    let output = std::env::var("SPX_OUT").unwrap();
    let file = std::fs::read(input).unwrap();
    let (info, pcm, used) = decode_all(&file, 0, 0);
    std::eprintln!("{info:?} arena {used}");
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    std::fs::write(output, bytes).unwrap();
    if let Ok(r) = std::env::var("SPX_REF") {
        let rb = std::fs::read(r).unwrap();
        let rf: Vec<i16> = rb.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        let first = pcm.iter().zip(&rf).position(|(a, b)| a != b);
        std::eprintln!(
            "len {} ref {} rms {:.1} dBFS first diff {:?} max {}",
            pcm.len(),
            rf.len(),
            rms_db(&pcm, &rf),
            first,
            pcm.iter().zip(&rf).map(|(&a, &b)| (i32::from(a) - i32::from(b)).abs()).max().unwrap_or(0)
        );
    }
}

/// 0.4 s narrowband mono, CBR quality 8 (sub-mode 5).
const NB: &[u8] = include_bytes!("../tests/data/nb_q8.spx");
const NB_REF: &[u8] = include_bytes!("../tests/data/nb_q8.s16");
/// 0.3 s wideband stereo, VBR quality 5.
const WB2: &[u8] = include_bytes!("../tests/data/wb_stereo_vbr.spx");
const WB2_REF: &[u8] = include_bytes!("../tests/data/wb_stereo_vbr.s16");
/// 0.2 s ultra-wideband mono, three frames per packet.
const UWB: &[u8] = include_bytes!("../tests/data/uwb_fpp3.spx");
const UWB_REF: &[u8] = include_bytes!("../tests/data/uwb_fpp3.s16");

fn s16(b: &[u8]) -> Vec<i16> {
    b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()
}

#[test]
fn matches_libspeex() {
    for (file, reference, rate, ch) in
        [(NB, NB_REF, 8000, 1), (WB2, WB2_REF, 16000, 2), (UWB, UWB_REF, 32000, 1)]
    {
        let (info, pcm, used) = decode_all(file, 0, 0);
        assert_eq!((info.rate, info.channels, info.codec), (rate, ch, CodecId::Speex));
        let r = s16(reference);
        assert_eq!(pcm.len(), r.len(), "gapless length");
        let db = rms_db(&pcm, &r);
        assert!(db < -90.0, "rms difference {db:.1} dBFS");
        assert!(used <= 12 * 1024, "arena {used}");
    }
}

#[test]
fn ring_and_trickle_give_the_same_samples() {
    for file in [NB, WB2, UWB] {
        let (_, whole, _) = decode_all(file, 0, 0);
        for (ring, chunk) in [(4096, 1), (32 * 1024, 7), (1024, 333)] {
            let (_, p, _) = decode_all(file, ring, chunk);
            assert_eq!(p, whole, "ring {ring} chunk {chunk}");
        }
    }
}

#[test]
fn arena_plan_by_mode() {
    assert!(arena_for(0) < arena_for(1) && arena_for(1) < arena_for(2));
    assert!(arena_for(2) <= 12 * 1024, "{}", arena_for(2));
    for file in [NB, WB2, UWB] {
        let src = SliceSource::new(file);
        let plan = SpeexDecoder::inspect(&src, &ContainerInfo::default()).unwrap();
        let (_, _, used) = decode_all(file, 0, 0);
        assert!(used <= plan.arena);
        // A smaller arena is refused cleanly.
        let mut small = std::vec![0u8; plan.arena / 2];
        let mut src = SliceSource::new(file);
        assert!(matches!(
            SpeexDecoder::open(&mut src, &mut small, &ContainerInfo::default()),
            Err(OpenError::Unsupported(Unsupported::ArenaTooSmall { .. }))
        ));
    }
}

/// Seek to `t`, then decode `len` frames.
fn seek_then(file: &[u8], targets: &[u64], len: usize, bisect: bool) -> Vec<(u64, Vec<i16>)> {
    let mut src = SliceSource::ring(file, 32 * 1024);
    let hint = ContainerInfo::default();
    let plan = SpeexDecoder::inspect(&src, &hint).unwrap();
    let mut arena = std::vec![0u8; plan.arena];
    let (mut d, info) = SpeexDecoder::open(&mut src, &mut arena, &hint).unwrap();
    if bisect {
        d.skim_limit = 0;
    }
    let ch = usize::from(info.channels);
    let mut res = Vec::new();
    for &t in targets {
        let landed = loop {
            match d.seek(&mut src, t).unwrap() {
                SeekStep::Done(f) => break f,
                SeekStep::NeedData(n) => {
                    assert!(n > src.available() && !src.at_end());
                    src.refill();
                }
            }
        };
        let mut out = Vec::new();
        while out.len() < len * ch {
            match d.decode(&mut src).unwrap() {
                Step::Pcm(PcmBlock::I16(s)) => out.extend_from_slice(s),
                Step::Pcm(PcmBlock::I32(_)) => panic!(),
                Step::NeedData(_) => src.refill(),
                Step::EndOfStream => break,
            }
        }
        out.truncate(len * ch);
        res.push((landed, out));
    }
    res
}

#[test]
fn seek_lands_on_target() {
    for file in [NB, WB2, UWB] {
        let (info, whole, _) = decode_all(file, 0, 0);
        let ch = usize::from(info.channels);
        let total = (whole.len() / ch) as u64;
        let targets = [total / 2, 0, 17, total - 1, total, total / 3];
        for ((landed, pcm), &t) in seek_then(file, &targets, 500, false)
            .iter()
            .chain(&seek_then(file, &targets, 500, true))
            .zip(targets.iter().cycle())
        {
            assert_eq!(*landed, t);
            let s = t as usize * ch;
            let want = &whole[s..(s + 500 * ch).min(whole.len())];
            assert_eq!(pcm.len(), want.len(), "seek to {t}");
            assert!(pcm.is_empty() || rms_db(pcm, want) < -80.0, "seek to {t}: {:.1}", rms_db(pcm, want));
        }
    }
}

#[test]
fn damaged_streams_end_cleanly() {
    let mut rng = 0x2545_F491u32;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        rng
    };
    for file in [NB, WB2, UWB] {
        for cut in [0, 30, 100, file.len() / 2, file.len() - 1] {
            let f = &file[..cut];
            let mut src = SliceSource::new(f);
            let hint = ContainerInfo::default();
            if SpeexDecoder::inspect(&src, &hint).is_err() {
                continue;
            }
            let mut arena = std::vec![0u8; ARENA_MIN * 4];
            let Ok((mut d, _)) = SpeexDecoder::open(&mut src, &mut arena, &hint) else { continue };
            let mut ended = false;
            for _ in 0..10_000 {
                match d.decode(&mut src) {
                    Ok(Step::EndOfStream) => {
                        ended = true;
                        break;
                    }
                    Ok(Step::NeedData(_)) => panic!("NeedData at the end of a truncated file"),
                    _ => {}
                }
            }
            assert!(ended);
        }
        for _ in 0..200 {
            let mut f = file.to_vec();
            for _ in 0..1 + next() % 8 {
                let i = next() as usize % f.len();
                f[i] ^= 1 << (next() % 8);
            }
            let mut src = SliceSource::new(&f);
            let hint = ContainerInfo::default();
            let Ok(plan) = SpeexDecoder::inspect(&src, &hint) else { continue };
            let mut arena = std::vec![0u8; plan.arena];
            let Ok((mut d, _)) = SpeexDecoder::open(&mut src, &mut arena, &hint) else { continue };
            let mut ended = false;
            for _ in 0..10_000 {
                if let Ok(Step::EndOfStream) = d.decode(&mut src) {
                    ended = true;
                    break;
                }
            }
            assert!(ended);
        }
    }
}

/// The header parser refuses what it cannot decode.
#[test]
fn header_checks() {
    let src = SliceSource::new(&NB[..20]);
    assert!(matches!(
        SpeexDecoder::inspect(&src, &ContainerInfo::default()),
        Err(OpenError::Unsupported(Unsupported::BadHeader))
    ));
    let mut src = SliceSource::ring(NB, 64);
    src.seek(0).unwrap();
    src.trickle(30);
    assert!(matches!(SpeexDecoder::inspect(&src, &ContainerInfo::default()), Err(OpenError::NeedData(_))));
}

/// Seek error against the linear decode by pre-roll length, over the
/// files named in `SPX_FILES` (a development aid for choosing
/// [`PREROLL_FRAMES`]).
#[test]
#[ignore = "development aid: needs SPX_FILES"]
fn preroll_survey() {
    let files = std::env::var("SPX_FILES").unwrap();
    for name in files.split(',') {
        let file = std::fs::read(name).unwrap();
        let (info, whole, _) = decode_all(&file, 0, 0);
        let ch = usize::from(info.channels);
        let total = (whole.len() / ch) as u64;
        let mut line = std::string::String::new();
        for pre in [16u64, 24, 32, 40, 48, 64] {
            let mut worst = -200.0f64;
            let mut src = SliceSource::new(&file);
            let hint = ContainerInfo::default();
            let plan = SpeexDecoder::inspect(&src, &hint).unwrap();
            let mut arena = std::vec![0u8; plan.arena];
            let (mut d, _) = SpeexDecoder::open(&mut src, &mut arena, &hint).unwrap();
            d.preroll = pre * frame_size(d.head.mode) as u64;
            if std::env::var("SPX_BISECT").is_ok() {
                d.skim_limit = 0;
            }
            for k in 1..20u64 {
                let t = total * k / 20 + k * 7;
                let SeekStep::Done(_) = d.seek(&mut src, t).unwrap() else { panic!() };
                let mut out = Vec::new();
                while out.len() < 2048 * ch {
                    match d.decode(&mut src).unwrap() {
                        Step::Pcm(PcmBlock::I16(s)) => out.extend_from_slice(s),
                        Step::EndOfStream => break,
                        _ => {}
                    }
                }
                out.truncate(2048 * ch);
                let s = t as usize * ch;
                let want = &whole[s..(s + out.len()).min(whole.len())];
                worst = worst.max(rms_db(&out, want));
            }
            line += &std::format!(" {pre}:{worst:.0}");
        }
        std::eprintln!("{name}:{line}");
    }
}
