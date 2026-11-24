use std::vec::Vec;

use super::*;
use orpheus_audio::SliceSource;

fn le16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn le32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

/// A RIFF/WAVE file with the given fmt fields and data; `junk` bytes of a
/// LIST chunk come before the data chunk.
fn wav(tag: u16, ch: u16, rate: u32, bits: u16, align: u16, data: &[u8], junk: usize) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    le32(&mut v, 0);
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    le32(&mut v, 16);
    le16(&mut v, tag);
    le16(&mut v, ch);
    le32(&mut v, rate);
    le32(&mut v, rate * u32::from(align));
    le16(&mut v, align);
    le16(&mut v, bits);
    if junk > 0 {
        v.extend_from_slice(b"LIST");
        le32(&mut v, junk as u32);
        v.extend(std::iter::repeat_n(0x55, junk + (junk & 1)));
    }
    v.extend_from_slice(b"data");
    le32(&mut v, data.len() as u32);
    v.extend_from_slice(data);
    v
}

/// Decode everything through a simulated ring of `cap` bytes refilled in
/// `chunk`-byte steps; returns samples widened to i32 and the info.
fn run(file: &[u8], cap: usize, chunk: usize) -> (StreamInfo, Vec<i32>) {
    let mut src = SliceSource::ring(file, cap);
    let hint = ContainerInfo::default();
    let mut tries = 0;
    let plan = loop {
        match WavDecoder::inspect(&src, &hint) {
            Ok(p) => break p,
            Err(OpenError::NeedData(n)) => {
                // The ring must be able to hold what the decoder asks for.
                assert!(n <= cap && tries < 100_000, "inspect wants {n} of a {cap}-byte ring");
                tries += 1;
                src.trickle(chunk);
            }
            Err(e) => panic!("{e:?}"),
        }
    };
    let mut arena = std::vec![0u8; plan.arena];
    let (mut d, info) = WavDecoder::open(&mut src, &mut arena, &hint).unwrap();
    assert_eq!(info, plan.info);
    let mut out = Vec::new();
    for _ in 0..1_000_000 {
        match d.decode(&mut src) {
            Ok(Step::Pcm(PcmBlock::I16(s))) => out.extend(s.iter().map(|&x| i32::from(x))),
            Ok(Step::Pcm(PcmBlock::I32(s))) => out.extend_from_slice(s),
            Ok(Step::NeedData(n)) => {
                assert!(n > src.available() && n <= orpheus_audio::MAX_PEEK);
                src.trickle(chunk);
            }
            Ok(Step::EndOfStream) => return (info, out),
            Err(DecodeError::Corrupt) => {}
            Err(e) => panic!("{e:?}"),
        }
    }
    panic!("no end");
}

#[test]
fn pcm16_stereo_through_small_ring() {
    let samples: Vec<i16> = (0..2000).map(|i| (i * 37 - 30000) as i16).collect();
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    for (cap, chunk, junk) in
        [(usize::MAX, usize::MAX, 3000), (4099, 4099, 3000), (4099, 7, 3000), (1001, 333, 0), (64, 7, 0)]
    {
        let file = wav(1, 2, 44100, 16, 4, &data, junk);
        let (info, out) = run(&file, cap.min(file.len()), chunk);
        assert_eq!((info.rate, info.channels, info.bits), (44100, 2, 16));
        assert_eq!(info.frames, Some(1000));
        let want: Vec<i32> = samples.iter().map(|&x| i32::from(x)).collect();
        assert_eq!(out, want, "cap {cap} chunk {chunk}");
    }
}

#[test]
fn pcm24_and_u8() {
    let data = [0x01u8, 0x02, 0x83, 0xFF, 0xFF, 0x7F];
    let (info, out) = run(&wav(1, 1, 48000, 24, 3, &data, 0), 47, 1);
    assert_eq!(info.sample, SampleKind::I32);
    assert_eq!(out, std::vec![0x8302_0100u32 as i32, 0x7FFF_FF00]);
    let (_, out) = run(&wav(1, 1, 8000, 8, 1, &[0, 128, 255], 0), 100, 100);
    assert_eq!(out, std::vec![-32768, 0, 127 << 8]);
}

#[test]
fn float_and_g711() {
    let data: Vec<u8> = [0.5f32, -1.0, 2.0].iter().flat_map(|f| f.to_le_bytes()).collect();
    let (_, out) = run(&wav(3, 1, 8000, 32, 4, &data, 0), 100, 100);
    assert_eq!(out, std::vec![1 << 30, i32::MIN, i32::MAX]);
    let (_, out) = run(&wav(7, 1, 8000, 8, 1, &[0xFF, 0x00], 0), 100, 100);
    assert_eq!(out, std::vec![0, -32124]);
}

#[test]
fn seek_pcm() {
    let samples: Vec<i16> = (0..4000).map(|i| i as i16).collect();
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let file = wav(1, 1, 8000, 16, 2, &data, 10);
    let mut src = SliceSource::ring(&file, 512);
    let hint = ContainerInfo::default();
    let mut arena = std::vec![0u8; ARENA];
    let (mut d, _) = WavDecoder::open(&mut src, &mut arena, &hint).unwrap();
    assert_eq!(d.seek(&mut src, 1234).unwrap(), SeekStep::Done(1234));
    src.refill();
    match d.decode(&mut src).unwrap() {
        Step::Pcm(PcmBlock::I16(s)) => assert_eq!(s[0], 1234),
        other => panic!("{other:?}"),
    }
    assert_eq!(d.seek(&mut src, 9999).unwrap(), SeekStep::Done(4000));
    src.refill();
    assert!(matches!(d.decode(&mut src).unwrap(), Step::EndOfStream));
}

#[test]
fn ima_block_decodes() {
    // One mono IMA block: header predictor 100, index 0, then 4 bytes.
    let data = [100u8, 0, 0, 0, 0x71, 0x00, 0x00, 0x00];
    let file = wav(0x11, 1, 8000, 4, 8, &data, 0);
    let (info, out) = run(&file, 100, 100);
    assert_eq!(info.frames, Some(9));
    let mut s = Ima { predictor: 100, index: 0 };
    let mut want = std::vec![100];
    for n in [1u8, 7, 0, 0, 0, 0, 0, 0] {
        want.push(i32::from(s.expand(n)));
    }
    assert_eq!(out, want);
}

#[test]
fn robust_against_garbage() {
    let samples: Vec<u8> = (0..3000u32).map(|i| (i * 7) as u8).collect();
    for (tag, bits, align) in [(1u16, 16u16, 4u16), (0x11, 4, 256), (2, 4, 256), (3, 32, 8)] {
        let file = wav(tag, 2, 22050, bits, align, &samples, 0);
        let mut x = 0x9E37_79B9u32;
        for _ in 0..200 {
            let mut f = file.clone();
            for _ in 0..8 {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let i = x as usize % f.len();
                f[i] ^= 1 << (x >> 29);
            }
            let cut = (x as usize >> 3) % f.len();
            f.truncate(cut.max(1));
            let mut src = SliceSource::ring(&f, 777);
            let mut arena = std::vec![0u8; ARENA];
            let Ok((mut d, _)) = WavDecoder::open(&mut src, &mut arena, &ContainerInfo::default()) else {
                continue;
            };
            let mut done = false;
            for _ in 0..100_000 {
                match d.decode(&mut src) {
                    Ok(Step::EndOfStream) => {
                        done = true;
                        break;
                    }
                    Ok(Step::NeedData(_)) => src.refill(),
                    _ => {}
                }
            }
            assert!(done);
        }
    }
}
