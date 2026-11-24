use std::vec::Vec;

use super::*;
use orpheus_audio::SliceSource;

/// 0.3 s of a 1 kHz sine at 44.1 kHz mono, 32 kbit/s, with a LAME tag
/// (made by ffmpeg/libmp3lame).
const SINE: &[u8] = include_bytes!("../tests/data/sine1k_44100_1.mp3");

fn decode_all(file: &[u8], ring: usize) -> (StreamInfo, Vec<i16>, u32) {
    let mut src = if ring == 0 { SliceSource::new(file) } else { SliceSource::ring(file, ring) };
    let hint = ContainerInfo::default();
    let mut arena = std::vec![0u8; ARENA];
    let (mut d, info) = Mp3Decoder::open(&mut src, &mut arena, &hint).unwrap();
    let (mut out, mut corrupt) = (Vec::new(), 0);
    for _ in 0..1_000_000 {
        match d.decode(&mut src) {
            Ok(Step::Pcm(PcmBlock::I16(s))) => out.extend_from_slice(s),
            Ok(Step::Pcm(_)) => panic!("i32 output"),
            Ok(Step::NeedData(n)) => {
                assert!(n > src.available() && !src.at_end());
                src.refill();
            }
            Ok(Step::EndOfStream) => return (info, out, corrupt),
            Err(DecodeError::Corrupt) => corrupt += 1,
            Err(e) => panic!("{e:?}"),
        }
    }
    panic!("no end");
}

#[test]
fn gapless_length_and_signal() {
    let (info, pcm, corrupt) = decode_all(SINE, 0);
    assert_eq!(corrupt, 0);
    assert_eq!((info.rate, info.channels), (44100, 1));
    assert_eq!(info.frames, Some(13230));
    assert_eq!(pcm.len(), 13230);
    let peak = pcm.iter().map(|s| i32::from(*s).abs()).max().unwrap_or(0);
    assert!(peak > 3000, "peak {peak}");
    // Same samples through a tiny ring that splits most frames.
    let (_, ring, _) = decode_all(SINE, 1000);
    assert_eq!(ring, pcm);
}

#[test]
fn vbr_tag() {
    // MPEG-1 layer III 128k 44.1k stereo; Xing at 4 + 32; frames=10,
    // LAME delay 576, padding 1000.
    let mut f = std::vec![0u8; 417];
    f[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
    f[36..40].copy_from_slice(b"Info");
    f[40..44].copy_from_slice(&1u32.to_be_bytes());
    f[44..48].copy_from_slice(&10u32.to_be_bytes());
    f[48..52].copy_from_slice(b"LAME");
    let v = (576u32 << 12) | 1000;
    f[48 + 21..48 + 24].copy_from_slice(&v.to_be_bytes()[1..]);
    let h = MpegHeader::parse([0xFF, 0xFB, 0x90, 0x00]).unwrap();
    let t = parse_vbr_tag(&View::new(&f, &[]), &h);
    assert_eq!(t, VbrTag { present: true, frames: Some(10), bytes: None, gapless: Some((576, 1000)) });
}

#[test]
fn seek_is_sample_exact() {
    let (_, all, _) = decode_all(SINE, 0);
    for target in [0u64, 1, 1151, 1152, 5000, 13229] {
        let mut src = SliceSource::ring(SINE, 700);
        let mut arena = std::vec![0u8; ARENA];
        let (mut d, _) = Mp3Decoder::open(&mut src, &mut arena, &ContainerInfo::default()).unwrap();
        loop {
            match d.seek(&mut src, target).unwrap() {
                SeekStep::Done(f) => {
                    assert_eq!(f, target);
                    break;
                }
                SeekStep::NeedData(_) => src.refill(),
            }
        }
        let mut got = Vec::new();
        while got.len() < 64 {
            match d.decode(&mut src).unwrap() {
                Step::Pcm(PcmBlock::I16(s)) => got.extend_from_slice(s),
                Step::NeedData(_) => src.refill(),
                _ => break,
            }
        }
        let t = target as usize;
        let n = got.len().min(64).min(all.len() - t);
        assert_eq!(&got[..n], &all[t..t + n], "target {target}");
    }
}

#[test]
fn garbage_terminates() {
    let mut x = 0x1234_5678u32;
    let mut data: Vec<u8> = (0..20_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect();
    // Sprinkle plausible headers so the decoder locks on and decodes noise.
    for i in (0..data.len() - 4).step_by(417) {
        data[i..i + 4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
    }
    let mut src = SliceSource::ring(&data, 4096);
    let mut arena = std::vec![0u8; ARENA];
    let Ok((mut d, _)) = Mp3Decoder::open(&mut src, &mut arena, &ContainerInfo::default()) else { return };
    for _ in 0..1_000_000 {
        match d.decode(&mut src) {
            Ok(Step::EndOfStream) => return,
            Ok(Step::NeedData(_)) => src.refill(),
            _ => {}
        }
    }
    panic!("no end");
}
