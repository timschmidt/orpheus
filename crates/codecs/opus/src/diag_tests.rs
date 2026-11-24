//! Diagnostic: how fast does a decoder started mid-stream converge to the
//! linear decode? (Prints per-20 ms RMS differences.)

use std::vec::Vec;

use yoton_audio::{ContainerInfo, Decoder, PcmBlock, SeekStep, SliceSource, Step};

use crate::OpusDecoder;

fn decode_all(d: &mut OpusDecoder<'_>, src: &mut SliceSource<'_>, max: usize) -> Vec<i16> {
    let mut out = Vec::new();
    for _ in 0..1_000_000 {
        match d.decode(src) {
            Ok(Step::Pcm(PcmBlock::I16(s))) => out.extend_from_slice(s),
            Ok(Step::NeedData(_)) => src.refill(),
            Ok(Step::EndOfStream) => break,
            _ => {}
        }
        if out.len() >= max {
            break;
        }
    }
    out
}

#[test]
fn seek_convergence() {
    let Ok(path) = std::env::var("OPUS_DIAG") else { return };
    let file = std::fs::read(path).unwrap();
    let mut arena = std::vec![0u8; 64 * 1024];
    let mut src = SliceSource::new(&file);
    let (mut d, info) = OpusDecoder::open(&mut src, &mut arena, &ContainerInfo::default()).unwrap();
    let ch = usize::from(info.channels);
    let lin = decode_all(&mut d, &mut src, usize::MAX);
    let mut arena2 = std::vec![0u8; 64 * 1024];
    let mut src2 = SliceSource::new(&file);
    let (mut d2, _) = OpusDecoder::open(&mut src2, &mut arena2, &ContainerInfo::default()).unwrap();
    d2.set_preroll(0);
    let target: u64 = std::env::var("OPUS_DIAG_T").ok().and_then(|s| s.parse().ok()).unwrap_or(48000);
    loop {
        match d2.seek(&mut src2, target).unwrap() {
            SeekStep::Done(f) => {
                assert_eq!(f, target);
                break;
            }
            SeekStep::NeedData(_) => src2.refill(),
        }
    }
    let got = decode_all(&mut d2, &mut src2, usize::MAX);
    let base = target as usize * ch;
    for (k, chunk) in got.chunks(960 * ch).enumerate().take(80) {
        let r = &lin[base + k * 960 * ch..(base + (k + 1) * 960 * ch).min(lin.len())];
        let mut s = 0f64;
        for (a, b) in chunk.iter().zip(r) {
            let e = f64::from(i32::from(*a) - i32::from(*b));
            s += e * e;
        }
        std::println!("{k:3} rms {:.2}", (s / chunk.len() as f64).sqrt());
    }
}
