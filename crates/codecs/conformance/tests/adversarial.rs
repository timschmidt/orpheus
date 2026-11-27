//! Adversarial inputs for the decoders, containers and tag readers (VER-3,
//! 07 §3.7), over the committed seed files of every codec target
//! (`tests/fuzz-corpus/<target>/seed-*`, real encoder output from the
//! conformance corpus):
//!
//! * every file cut at every byte of its first 640 bytes (where the
//!   headers are) and at spread points beyond, decoded with the format
//!   forced and through the probe, on the device ring refilled in 509-byte
//!   steps: no panic, no broken `NeedData` contract, an end within the
//!   budget (the fuzz bodies' verdict);
//! * the arena one byte smaller than `inspect` planned: `open` refuses or
//!   decodes the same; and a reused arena (filled with another track's
//!   bytes, as on the player) decodes exactly what a zeroed one does;
//! * length fields of WAV, FLAC, ID3v2 and Ogg at their edge values;
//! * the tag readers on every cut and every damaged byte of tagged seeds.
//!
//! The every-byte run over whole files is `#[ignore]`d
//! (`cargo test -p orpheus-codec-conformance --test adversarial -- --ignored`).

#[path = "../../../../tests/adversarial.rs"]
mod adversarial;

use adversarial::{damaged, edges, prefixes, put_be, put_le};
use orpheus_audio::{DecodeError, Format, OggCodec, SliceSource, Step, Unsupported};
use orpheus_codec_conformance::drive::{self, Cfg, Fail};
use orpheus_codec_conformance::fuzz;

const CODECS: [(&str, Format); 6] = [
    ("wav", Format::Wav),
    ("mp3", Format::Mpeg),
    ("flac", Format::Flac),
    ("vorbis", Format::Ogg(OggCodec::Vorbis)),
    ("opus", Format::Ogg(OggCodec::Opus)),
    ("speex", Format::Ogg(OggCodec::Speex)),
];

/// The seed files of a target, without the fuzz bodies' 3-byte prefix.
fn seeds(target: &str) -> Vec<(String, Vec<u8>)> {
    let dir = adversarial::root().join("tests").join("fuzz-corpus").join(target);
    let mut v: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("seed-"))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
        .collect();
    v.sort();
    assert!(!v.is_empty(), "no seeds for {target}");
    v
}

/// `file` decoded through the fuzz bodies (forced decoder, and probed),
/// with selector `sel` (refill pattern) and no seek.
fn decode_both(file: &[u8], sel: u8, force: Format) {
    let mut d = vec![sel, 0, 0];
    d.extend_from_slice(file);
    fuzz::decode_with(&d, Some(force));
    fuzz::decode_any(&d);
}

/// `file` through the forced decoder only (device ring, 509-byte refills).
fn decode_forced(file: &[u8], force: Format) {
    let mut d = vec![3, 0, 0];
    d.extend_from_slice(file);
    fuzz::decode_with(&d, Some(force));
}

/// Cut points: every byte of the first `head` bytes, then `spread` more.
fn cuts(len: usize, head: usize, spread: usize) -> Vec<usize> {
    let mut v: Vec<usize> = (0..len.min(head)).collect();
    if len > head {
        let step = ((len - head) / spread.max(1)).max(1);
        v.extend((head..len).step_by(step));
    }
    v
}

fn truncations(head: usize, spread: usize) {
    for (target, format) in CODECS {
        for (name, seed) in seeds(target) {
            let file = &seed[3..];
            for n in cuts(file.len(), head, spread) {
                eprintln!("{target}/{name} cut at {n}");
                decode_both(&file[..n], 3, format);
            }
        }
    }
}

#[test]
fn every_codec_cut_in_its_headers() {
    truncations(640, 24);
}

#[test]
#[ignore = "long: every byte of every seed (minutes)"]
fn every_codec_cut_at_every_byte() {
    truncations(usize::MAX, 0);
}

/// Decode up to `blocks` PCM blocks of `file` (format forced) with an
/// arena of `arena` bytes prefilled with `fill`; `None` if open refused.
fn decode_with_arena(
    file: &[u8],
    format: Format,
    arena: usize,
    fill: u8,
    blocks: usize,
) -> Option<(Vec<i32>, Option<DecodeError>)> {
    let cfg = Cfg { ring: 32 * 1024, chunk: 4096, arena: usize::MAX, force: Some(format) };
    let mut src = SliceSource::ring(file, cfg.ring);
    let (format, hint) = drive::probe(&mut src, &cfg, None).ok()?;
    // The plan first (inspect), then the open with the arena given.
    let mut big = vec![0u8; 1 << 20];
    let Ok((_, _, plan)) = drive::open(&mut src, &cfg, format, &hint, &mut big) else { return None };
    let mut src = SliceSource::ring(file, cfg.ring);
    src.refill();
    let size = if arena == usize::MAX { plan } else { arena.min(plan + 64) };
    let mut lease = vec![fill; size];
    let Ok((mut d, _)) = orpheus_codecs::open(format, &mut src, &mut lease, &hint) else { return None };
    let mut out = Vec::new();
    let mut n = 0;
    let mut err = None;
    for _ in 0..drive::budget(file.len()) {
        match d.decode(&mut src) {
            Ok(Step::Pcm(b)) => {
                match b {
                    orpheus_audio::PcmBlock::I16(s) => out.extend(s.iter().map(|&x| i32::from(x))),
                    orpheus_audio::PcmBlock::I32(s) => out.extend_from_slice(s),
                }
                n += 1;
                if n == blocks {
                    break;
                }
            }
            Ok(Step::NeedData(_)) => {
                if orpheus_audio::ByteSource::at_end(&src) {
                    break;
                }
                src.trickle(4096);
            }
            Ok(Step::EndOfStream) => break,
            Err(DecodeError::Corrupt) => {}
            Err(e) => {
                err = Some(e);
                break;
            }
        }
    }
    assert!(d.arena_used() <= size, "decoder used {} of a {size}-byte arena", d.arena_used());
    Some((out, err))
}

#[test]
fn arena_one_byte_short_and_reused() {
    for (target, format) in CODECS {
        for (name, seed) in seeds(target) {
            let file = &seed[3..];
            let Some(zeroed) = decode_with_arena(file, format, usize::MAX, 0x00, 24) else { continue };
            assert!(
                !matches!(zeroed.1, Some(DecodeError::Unsupported(Unsupported::ArenaTooSmall { .. }))),
                "{target}/{name}: the planned arena is not enough"
            );
            // Planned size exactly, but the arena holds another track's
            // bytes: the same PCM.
            for fill in [0xFFu8, 0xA5, 0x80] {
                let reused = decode_with_arena(file, format, usize::MAX, fill, 24);
                assert_eq!(reused.as_ref(), Some(&zeroed), "{target}/{name}: arena filled with {fill:#x}");
            }
            // One byte short of the plan: refused by `open`, or by the
            // first decode that needs the room (Vorbis parses its set-up
            // header there) with `ArenaTooSmall`, after the same PCM.
            let cfg = Cfg { ring: 0, chunk: 0, arena: 1 << 20, force: Some(format) };
            let plan = drive::decode_file(file, &cfg, None).map_or(0, |d| d.arena_plan);
            if plan > 0 {
                for short in [plan - 1, plan / 2, 0] {
                    if let Some((pcm, err)) = decode_with_arena(file, format, short, 0x5A, 24) {
                        let full = &zeroed.0;
                        assert!(
                            full.starts_with(&pcm),
                            "{target}/{name}: {short}-byte arena decodes differently"
                        );
                        if pcm.len() < full.len() || err != zeroed.1 {
                            assert!(
                                matches!(
                                    err,
                                    Some(DecodeError::Unsupported(Unsupported::ArenaTooSmall { .. }))
                                ),
                                "{target}/{name}: {short}-byte arena stopped with {err:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

/// `f` on a seed with `patch` applied for every edge value of the field.
fn with_field(file: &[u8], off: usize, n: usize, be: bool, real: u64, f: &mut dyn FnMut(&[u8], u64)) {
    if off + n > file.len() {
        return;
    }
    for v in edges(8 * n as u32, real) {
        let mut b = file.to_vec();
        if be {
            put_be(&mut b, off, n, v);
        } else {
            put_le(&mut b, off, n, v);
        }
        f(&b, v);
    }
}

fn le32(b: &[u8], o: usize) -> u64 {
    u64::from(u32::from_le_bytes(b[o..o + 4].try_into().unwrap()))
}

#[test]
fn wav_chunk_lengths_at_their_edges() {
    for (_, seed) in seeds("wav") {
        let file = &seed[3..seed.len().min(3 + 4096)];
        if &file[..4] != b"RIFF" {
            continue;
        }
        // RIFF size, then every chunk's size field in turn.
        with_field(file, 4, 4, false, le32(file, 4), &mut |b, _| decode_both(b, 2, Format::Wav));
        let mut o = 12;
        while o + 8 <= file.len() {
            let len = le32(file, o + 4);
            with_field(file, o + 4, 4, false, len, &mut |b, _| decode_both(b, 2, Format::Wav));
            if &file[o..o + 4] == b"fmt " {
                // Channels, rate, block align, bits at their edges.
                for (f, n) in [(10, 2), (12, 4), (20, 2), (22, 2), (8, 2)] {
                    let real = u64::from(file[o + f]) | u64::from(file[o + f + 1]) << 8;
                    with_field(file, o + f, n, false, real, &mut |b, _| decode_both(b, 2, Format::Wav));
                }
            }
            let next = o as u64 + 8 + len + (len & 1);
            if next > file.len() as u64 {
                break;
            }
            o = next as usize;
        }
    }
}

#[test]
fn flac_metadata_lengths_at_their_edges() {
    for (_, seed) in seeds("flac") {
        let file = &seed[3..seed.len().min(3 + 4096)];
        let Some(start) = file.windows(4).position(|w| w == b"fLaC") else { continue };
        let mut o = start + 4;
        while o + 4 <= file.len() {
            let len = u64::from(file[o + 1]) << 16 | u64::from(file[o + 2]) << 8 | u64::from(file[o + 3]);
            with_field(file, o + 1, 3, true, len, &mut |b, _| decode_both(b, 1, Format::Flac));
            // The block type byte: every type, last flag both ways.
            for t in [0u8, 1, 3, 4, 6, 7, 0x7F, 0x80, 0x84, 0xFF] {
                let mut b = file.to_vec();
                b[o] = t;
                decode_both(&b, 1, Format::Flac);
            }
            if file[o] & 0x7F == 0 && o + 4 + 18 <= file.len() {
                // STREAMINFO: block sizes, frame sizes, rate/channels/bits,
                // total samples.
                for (f, n) in [(0, 2), (2, 2), (4, 3), (7, 3), (10, 4), (14, 4)] {
                    with_field(file, o + 4 + f, n, true, 0, &mut |b, _| decode_both(b, 1, Format::Flac));
                }
            }
            if file[o] & 0x80 != 0 {
                break;
            }
            o += 4 + len as usize;
        }
    }
}

#[test]
fn ogg_page_fields_at_their_edges() {
    for (target, format) in &CODECS[3..] {
        for (_, seed) in seeds(target) {
            let file = &seed[3..seed.len().min(3 + 4096)];
            // The first three pages: segment counts, lacing values, the
            // granule position, the page sequence and the header type.
            let mut o = 0;
            for _ in 0..3 {
                if o + 27 > file.len()
                    || &file[o..o + 4] != b"OggS"
                    || o + 27 + usize::from(file[o + 26]) > file.len()
                {
                    break;
                }
                let segs = usize::from(file[o + 26]);
                for v in edges(8, segs as u64) {
                    let mut b = file.to_vec();
                    b[o + 26] = v as u8;
                    decode_forced(&b, *format);
                    let mut x = vec![0u8, 0, 0];
                    x.extend_from_slice(&b);
                    fuzz::ogg(&x[2..]);
                }
                for i in 0..segs.min(2) {
                    for v in [0u8, 1, 254, 255] {
                        let mut b = file.to_vec();
                        b[o + 27 + i] = v;
                        decode_forced(&b, *format);
                    }
                }
                with_field(file, o + 6, 8, false, 0, &mut |b, _| decode_forced(b, *format));
                with_field(file, o + 18, 4, false, 0, &mut |b, _| decode_forced(b, *format));
                for t in [0u8, 1, 2, 4, 7, 0xFF] {
                    let mut b = file.to_vec();
                    b[o + 5] = t;
                    decode_forced(&b, *format);
                }
                let body: usize = file[o + 27..o + 27 + segs].iter().map(|&x| usize::from(x)).sum();
                o += 27 + segs + body;
            }
        }
    }
}

/// An ID3v2.3 tag with one frame of `body` (size fields as given).
fn id3(tag_size: u32, frame_id: &[u8; 4], frame_size: u32, body: &[u8], flags: u8) -> Vec<u8> {
    let syncsafe =
        |v: u32| [(v >> 21) as u8 & 0x7F, (v >> 14) as u8 & 0x7F, (v >> 7) as u8 & 0x7F, v as u8 & 0x7F];
    let mut b = b"ID3\x03\x00".to_vec();
    b.push(flags);
    b.extend(syncsafe(tag_size));
    b.extend_from_slice(frame_id);
    b.extend(frame_size.to_be_bytes());
    b.extend([0, 0]);
    b.extend_from_slice(body);
    b
}

fn meta_all(file: &[u8]) {
    for sel in 0..8u8 {
        let mut d = vec![sel];
        d.extend_from_slice(file);
        fuzz::meta(&d);
    }
}

#[test]
fn id3_sizes_and_frames_at_their_edges() {
    let mp3 = seeds("mp3").remove(0).1;
    let audio = &mp3[3..mp3.len().min(3 + 2048)];
    let frames: [(&[u8; 4], &[u8]); 8] = [
        (b"TIT2", b"\x00Title"),
        (b"TPE1", b"\x01\xFF\xFEA\x00r\x00"),
        (b"TALB", b"\x03Album\x00"),
        (b"APIC", b"\x00image/jpeg\x00\x03\x00\xFF\xD8\xFF"),
        (b"USLT", b"\x00eng\x00lyrics"),
        (b"SYLT", b"\x00eng\x02\x01\x00x\x00\x00\x00\x00\x10"),
        (b"TXXX", b"\x00desc\x00value"),
        (b"COMM", b"\x00eng\x00\x00c"),
    ];
    for (id, body) in frames {
        let real = body.len() as u64;
        for fsize in edges(32, real) {
            for tsize in [0u32, 10, real as u32 + 10, 0x0FFF_FFFF] {
                for flags in [0u8, 0x40, 0x80, 0xFF] {
                    let mut f = id3(tsize, id, fsize as u32, body, flags);
                    meta_all(&f);
                    f.extend_from_slice(audio);
                    meta_all(&f);
                    decode_both(&f, 3, Format::Mpeg);
                }
            }
        }
    }
}

#[test]
fn tag_readers_on_every_cut_and_damaged_byte() {
    for (_, seed) in seeds("meta") {
        let file = &seed[1..];
        let head = &file[..file.len().min(512)];
        prefixes(head, meta_all);
        damaged(head, |_, d| {
            let mut x = d.to_vec();
            x.extend_from_slice(&file[head.len()..]);
            let mut s = vec![seed[0]];
            s.extend_from_slice(&x);
            fuzz::meta(&s);
        });
    }
}

#[test]
fn lyrics_hostile_lines() {
    let lines: [&[u8]; 10] = [
        b"[00:00.00]",
        b"[99:99.99]overflow",
        b"[4294967295:00.00]x",
        b"[-1:00.00]neg",
        b"[00:00.00][00:01.00][00:02.00]repeated stamps",
        b"[ar:\xFF\xFE]",
        b"[",
        b"]]]]",
        b"[00:00.000000000000000000001]",
        b"[offset:-99999999999999999999]",
    ];
    for l in lines {
        let mut f = l.to_vec();
        f.push(b'\n');
        fuzz::lyrics(&f);
        fuzz::lyrics(&f.repeat(300));
        fuzz::lyrics(l);
    }
    fuzz::lyrics(&[b'['; 70_000]);
    fuzz::lyrics(&vec![b'x'; 70_000]);
    let _: Option<Fail> = None;
}
