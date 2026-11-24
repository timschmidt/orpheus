use std::vec::Vec;

use md5::{Digest, Md5};

use super::*;
use orpheus_audio::SliceSource;

/// 0.15 s of two sines, 44.1 kHz stereo 16-bit, `flac -5 -b 1152`.
const S16: &[u8] = include_bytes!("../tests/data/sine_44100_2.flac");
/// 0.1 s of a sine, 48 kHz mono 24-bit, `flac -8`.
const S24: &[u8] = include_bytes!("../tests/data/sine_48000_1_24.flac");

fn decode_all(file: &[u8], ring: usize, chunk: usize) -> (StreamInfo, Vec<i32>) {
    let mut src = if ring == 0 { SliceSource::new(file) } else { SliceSource::ring(file, ring) };
    let hint = ContainerInfo::default();
    let plan = FlacDecoder::inspect(&src, &hint).unwrap();
    let mut arena = std::vec![0u8; plan.arena];
    let (mut d, info) = FlacDecoder::open(&mut src, &mut arena, &hint).unwrap();
    let mut out = Vec::new();
    for _ in 0..1_000_000 {
        match d.decode(&mut src) {
            Ok(Step::Pcm(PcmBlock::I16(s))) => out.extend(s.iter().map(|&x| i32::from(x))),
            Ok(Step::Pcm(PcmBlock::I32(s))) => out.extend_from_slice(s),
            Ok(Step::NeedData(n)) => {
                assert!(n > src.available() && !src.at_end() && n <= MAX_PEEK);
                src.trickle(chunk);
            }
            Ok(Step::EndOfStream) => return (info, out),
            Err(e) => panic!("{e:?}"),
        }
    }
    panic!("no end");
}

fn md5_of(pcm: &[i32], wide: bool, bits: u8) -> [u8; 16] {
    let shift = if wide { 32 - u32::from(bits) } else { 16 - u32::from(bits) };
    let bytes = usize::from(bits).div_ceil(8);
    let mut h = Md5::new();
    for &s in pcm {
        h.update(&(s >> shift).to_le_bytes()[..bytes]);
    }
    h.finalize().into()
}

fn streaminfo_md5(file: &[u8]) -> [u8; 16] {
    file[26..42].try_into().unwrap()
}

#[test]
fn md5_matches_16_and_24_bit() {
    let (info, pcm) = decode_all(S16, 0, 0);
    assert_eq!((info.rate, info.channels, info.bits, info.sample), (44100, 2, 16, SampleKind::I16));
    assert_eq!(info.frames, Some(6615));
    assert_eq!(pcm.len(), 6615 * 2);
    assert_eq!(md5_of(&pcm, false, 16), streaminfo_md5(S16));
    let (info, pcm) = decode_all(S24, 0, 0);
    assert_eq!((info.bits, info.sample), (24, SampleKind::I32));
    assert_eq!(md5_of(&pcm, true, 24), streaminfo_md5(S24));
}

#[test]
fn same_output_through_small_rings() {
    let (_, whole) = decode_all(S16, 0, 0);
    for (ring, chunk) in [(MAX_PEEK + 1024, 1000), (MAX_PEEK + 7, 13)] {
        let (_, pcm) = decode_all(S16, ring, chunk);
        assert_eq!(pcm, whole, "ring {ring} chunk {chunk}");
    }
}

#[test]
fn seeks_are_exact() {
    let (_, whole) = decode_all(S16, 0, 0);
    for target in [0u64, 1, 1151, 1152, 3000, 6614, 6615] {
        let mut src = SliceSource::ring(S16, MAX_PEEK + 1024);
        let hint = ContainerInfo::default();
        let mut arena = std::vec![0u8; FlacDecoder::inspect(&src, &hint).unwrap().arena];
        let (mut d, _) = FlacDecoder::open(&mut src, &mut arena, &hint).unwrap();
        let landed = loop {
            match d.seek(&mut src, target).unwrap() {
                SeekStep::Done(f) => break f,
                SeekStep::NeedData(_) => src.refill(),
            }
        };
        assert_eq!(landed, target);
        let mut got = Vec::new();
        while got.len() < 100 {
            match d.decode(&mut src).unwrap() {
                Step::Pcm(PcmBlock::I16(s)) => got.extend(s.iter().map(|&x| i32::from(x))),
                Step::NeedData(_) => src.refill(),
                _ => break,
            }
        }
        let t = target as usize * 2;
        let n = got.len().min(100).min(whole.len() - t);
        assert_eq!(&got[..n], &whole[t..t + n], "target {target}");
    }
}

#[test]
fn corrupt_frames_are_skipped() {
    let mut f = S16.to_vec();
    // Inside the audio (the front holds an 8 KiB PADDING block).
    let mid = f.len() - 3000;
    for b in &mut f[mid..mid + 40] {
        *b ^= 0x5A;
    }
    let mut src = SliceSource::new(&f);
    let hint = ContainerInfo::default();
    let mut arena = std::vec![0u8; FlacDecoder::inspect(&src, &hint).unwrap().arena];
    let (mut d, _) = FlacDecoder::open(&mut src, &mut arena, &hint).unwrap();
    let (mut corrupt, mut samples) = (0, 0);
    for _ in 0..100_000 {
        match d.decode(&mut src) {
            Ok(Step::Pcm(b)) => samples += b.len(),
            Ok(Step::EndOfStream) => break,
            Ok(Step::NeedData(_)) => panic!("all data was there"),
            Err(_) => corrupt += 1,
        }
    }
    assert!(corrupt >= 1, "corrupt {corrupt} samples {samples}");
    // One 1152-sample frame lost, the rest decoded.
    assert_eq!(samples, (6615 - 1152) * 2);
}
