use std::vec::Vec;

use super::*;
use yoton_audio::SliceSource;

fn corpus(name: &str) -> Option<Vec<u8>> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/corpus").join(name))
        .ok()
}

fn run(d: &mut VorbisDecoder<'_>, src: &mut SliceSource<'_>, max: usize) -> Vec<i16> {
    let mut out = Vec::new();
    for _ in 0..1_000_000 {
        match d.decode(src) {
            Ok(Step::Pcm(PcmBlock::I16(s))) => out.extend_from_slice(s),
            Ok(Step::NeedData(_)) => src.refill(),
            Ok(Step::EndOfStream) => break,
            Ok(_) => unreachable!(),
            Err(e) => std::eprintln!("err {e:?}"),
        }
        if out.len() >= max {
            break;
        }
    }
    out
}

#[test]
fn seek_to_start_matches_linear_decode() {
    let Some(f) = corpus("vorbis/q5_44100_2.ogg") else { return };
    let mut src = SliceSource::ring(&f, 32 * 1024);
    let mut arena = std::vec![0u8; ARENA_MAX];
    let (mut d, _) = VorbisDecoder::open(&mut src, &mut arena, &ContainerInfo::default()).unwrap();
    let all = run(&mut d, &mut src, usize::MAX);
    std::eprintln!("audio_at {:?} pos {:?}", d.audio_at.map(|a| a.0), d.pos);
    loop {
        match d.seek(&mut src, 0).unwrap() {
            SeekStep::Done(_) => break,
            SeekStep::NeedData(_) => src.refill(),
        }
    }
    let again = run(&mut d, &mut src, 8192);
    std::eprintln!("first {:?}\nagain {:?}", &all[..8], &again[..8]);
    let n = again.len().min(all.len());
    let first = (0..n).position(|i| all[i] != again[i]);
    assert_eq!(first, None);
}

#[test]
fn ring_and_whole_file_agree() {
    for name in ["vorbis/q10_44100_2.ogg", "vorbis/bigcomment_q3.ogg"] {
        let Some(f) = corpus(name) else { return };
        let mut outs = Vec::new();
        for ring in [0usize, 32 * 1024] {
            let mut src = if ring == 0 { SliceSource::new(&f) } else { SliceSource::ring(&f, ring) };
            let mut arena = std::vec![0u8; ARENA_MAX];
            let (mut d, _) = VorbisDecoder::open(&mut src, &mut arena, &ContainerInfo::default()).unwrap();
            let o = run(&mut d, &mut src, usize::MAX);
            std::eprintln!("{name} ring {ring}: {} samples, end pos {}", o.len(), src.position());
            outs.push(o);
        }
        assert_eq!(outs[0], outs[1], "{name}");
    }
}

#[test]
fn struct_size() {
    std::eprintln!("VorbisDecoder {} B", core::mem::size_of::<VorbisDecoder<'_>>());
}
