//! The firmware's gapless hand-over (`join::decode_gapless`: one stream
//! ring read ahead into the next file, the next decoder opened on the bytes
//! already there, in the same arena) against each part decoded alone: no
//! sample inserted, lost or changed at any join (AU-5, 04 §2.2).

use orpheus_codec_conformance::drive::{Cfg, decode_file};
use orpheus_codec_conformance::join::{self, DEVICE_RING, decode_gapless};

const ARENA: usize = 64 * 1024;
const FLAC: &[u8] = include_bytes!("../../flac/tests/data/sine_44100_2.flac");
const FLAC24: &[u8] = include_bytes!("../../flac/tests/data/sine_48000_1_24.flac");
const MP3: &[u8] = include_bytes!("../../mp3/tests/data/sine1k_44100_1.mp3");

/// A 16-bit stereo WAV of `frames`, optionally behind an ID3v2 tag of
/// `id3` bytes and followed by a `LIST` chunk (a tag the decoder never
/// reads).
fn wav(frames: &[(i16, i16)], id3: usize, list: bool) -> Vec<u8> {
    let mut v = Vec::new();
    if id3 > 0 {
        let body = id3 - 10;
        v.extend_from_slice(b"ID3\x03\x00\x00");
        // Syncsafe size.
        v.extend((0..4).map(|i| ((body >> (21 - 7 * i)) & 0x7F) as u8));
        v.extend(std::iter::repeat_n(0u8, body));
    }
    let data = frames.len() * 4;
    let extra = if list { 8 + 64 } else { 0 };
    v.extend_from_slice(b"RIFF");
    v.extend(((4 + 24 + 8 + data + extra) as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend(16u32.to_le_bytes());
    v.extend(1u16.to_le_bytes());
    v.extend(2u16.to_le_bytes());
    v.extend(44_100u32.to_le_bytes());
    v.extend((44_100u32 * 4).to_le_bytes());
    v.extend(4u16.to_le_bytes());
    v.extend(16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend((data as u32).to_le_bytes());
    for &(l, r) in frames {
        v.extend(l.to_le_bytes());
        v.extend(r.to_le_bytes());
    }
    if list {
        v.extend_from_slice(b"LIST");
        v.extend(64u32.to_le_bytes());
        v.extend(std::iter::repeat_n(b'x', 64));
    }
    v
}

fn alone(file: &[u8], ext: &str) -> Vec<i32> {
    decode_file(file, &Cfg::device(ARENA), Some(ext)).unwrap().pcm
}

#[test]
fn split_wav_joins_sample_exact() {
    // A continuous signal cut at odd places, the cuts landing anywhere in
    // the ring; a tag after the first part, a tag larger than the ring
    // before the second.
    let sig: Vec<(i16, i16)> =
        (0..100_000i32).map(|i| (((i * 37) % 30_000) as i16, ((i * 91) % 25_000 - 12_000) as i16)).collect();
    for cut in [1usize, 4093, 33_333, 99_999] {
        let a = wav(&sig[..cut], 0, true);
        let b = wav(&sig[cut..], DEVICE_RING + 3000, false);
        let mut arena = vec![0u8; ARENA];
        let (out, corrupt) = decode_gapless(&[&a, &b], &[Some("wav"), Some("wav")], &mut arena).unwrap();
        assert_eq!(corrupt, 0);
        let want: Vec<i32> = sig.iter().flat_map(|&(l, r)| [i32::from(l), i32::from(r)]).collect();
        assert_eq!(out.len(), want.len(), "cut {cut}: samples inserted or lost");
        assert!(
            out == want,
            "cut {cut}: first difference at {:?}",
            out.iter().zip(&want).position(|(x, y)| x != y)
        );
    }
}

#[test]
fn codec_parts_join_as_decoded_alone() {
    let sets: [&[(&[u8], &str)]; 4] = [
        &[(FLAC, "flac"), (FLAC, "flac"), (FLAC, "flac")],
        &[(MP3, "mp3"), (MP3, "mp3"), (MP3, "mp3"), (MP3, "mp3")],
        &[(FLAC24, "flac"), (FLAC24, "flac")],
        // Different codecs one after the other in the same arena.
        &[(MP3, "mp3"), (FLAC, "flac"), (MP3, "mp3"), (FLAC24, "flac")],
    ];
    for set in sets {
        let files: Vec<&[u8]> = set.iter().map(|p| p.0).collect();
        let exts: Vec<Option<&str>> = set.iter().map(|p| Some(p.1)).collect();
        let mut arena = vec![0u8; ARENA];
        let (out, corrupt) = decode_gapless(&files, &exts, &mut arena).unwrap();
        assert_eq!(corrupt, 0);
        let want: Vec<i32> = set.iter().flat_map(|p| alone(p.0, p.1)).collect();
        assert_eq!(out.len(), want.len(), "{exts:?}");
        assert!(out == want, "{exts:?}");
    }
}

/// The corpus's album splits (`cargo xtask test codecs` makes the corpus;
/// skipped when it is absent).
#[test]
fn corpus_album_splits() {
    let dir = std::env::var_os("ORPHEUS_CORPUS").map_or_else(
        || std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/corpus"),
        std::path::PathBuf::from,
    );
    for j in join::manifest(&dir) {
        join::check(&j).unwrap_or_else(|e| panic!("{}: {e}", j.name));
    }
}
