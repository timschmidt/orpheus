use std::vec::Vec;

use super::*;
use orpheus_audio::SliceSource;

fn synchsafe(n: u32) -> [u8; 4] {
    [(n >> 21) as u8 & 0x7F, (n >> 14) as u8 & 0x7F, (n >> 7) as u8 & 0x7F, n as u8 & 0x7F]
}

fn frame(ver: u8, id: &[u8; 4], flags: u16, body: &[u8]) -> Vec<u8> {
    let mut f = Vec::new();
    f.extend_from_slice(id);
    if ver == 4 {
        f.extend_from_slice(&synchsafe(body.len() as u32));
    } else {
        f.extend_from_slice(&(body.len() as u32).to_be_bytes());
    }
    f.extend_from_slice(&flags.to_be_bytes());
    f.extend_from_slice(body);
    f
}

fn tag(ver: u8, flags: u8, frames: &[Vec<u8>], padding: usize) -> Vec<u8> {
    let body: Vec<u8> = frames.iter().flatten().copied().chain(std::iter::repeat_n(0, padding)).collect();
    let mut t = std::vec![b'I', b'D', b'3', ver, 0, flags];
    t.extend_from_slice(&synchsafe(body.len() as u32));
    t.extend_from_slice(&body);
    t
}

const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F', 0xFF, 0xD9];

fn v23_frames() -> Vec<Vec<u8>> {
    let mut apic = std::vec![0u8];
    apic.extend_from_slice(b"image/jpeg\0");
    apic.push(3);
    apic.extend_from_slice(b"cover\0");
    apic.extend_from_slice(JPEG);
    let mut uslt = std::vec![0u8];
    uslt.extend_from_slice(b"eng\0Some lyrics");
    std::vec![
        frame(3, b"TIT2", 0, b"\0Caf\xE9 au lait"),
        frame(3, b"TPE1", 0, b"\x01\xFF\xFEA\0r\0t\0"),
        frame(3, b"TALB", 0, b"\0Album"),
        frame(3, b"TRCK", 0, b"\x003/12"),
        frame(3, b"TCON", 0, b"\0(17)"),
        frame(3, b"TYER", 0, b"\x001999"),
        frame(3, b"TXXX", 0, b"\0replaygain_track_gain\0-6.48 dB"),
        frame(3, b"TXXX", 0, b"\0REPLAYGAIN_TRACK_PEAK\x000.988525"),
        frame(3, b"COMM", 0, b"\0engiTunSMPB\0 00000000 00000210 000003C4 0000000000ABCDEF"),
        frame(3, b"APIC", 0, &apic),
        frame(3, b"USLT", 0, &uslt),
    ]
}

fn read(file: &[u8], format: Format) -> Tags {
    read_all(&mut SliceSource::new(file), format, Opts::default()).unwrap()
}

/// The same through a small simulated ring with refills on `NeedData`.
fn read_ring(file: &[u8], format: Format, cap: usize, chunk: usize) -> Tags {
    let mut src = SliceSource::ring(file, cap);
    let mut r = TagReader::new(format, Opts::default());
    let mut tags = Tags::default();
    for _ in 0..1_000_000 {
        match r.step(&mut src, &mut tags).unwrap() {
            MetaStep::Done => return tags,
            MetaStep::Continue => {}
            MetaStep::NeedData(n) => {
                assert!(
                    n > src.available() && !src.at_end(),
                    "NeedData({n}) with {} buffered",
                    src.available()
                );
                src.trickle(chunk);
            }
        }
    }
    panic!("no end");
}

fn at(file: &[u8], l: &Located) -> Vec<u8> {
    let start = l.offset as usize;
    if l.unsync {
        let mut v: Vec<u8> = file[start..].to_vec();
        let n = io::unsync_in_place(&mut v);
        v.truncate(n);
        v[l.skip as usize..l.skip as usize + l.len as usize].to_vec()
    } else {
        file[start..start + l.len as usize].to_vec()
    }
}

#[test]
fn id3v23_fields() {
    let mut file = tag(3, 0, &v23_frames(), 100);
    file.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
    let t = read(&file, Format::Mpeg);
    assert_eq!(t.title.as_str(), "Café au lait");
    assert_eq!(t.artist.as_str(), "Art");
    assert_eq!(t.album.as_str(), "Album");
    assert_eq!((t.track, t.tracks, t.year), (3, 12, 1999));
    assert_eq!(t.genre.as_str(), "Rock");
    assert_eq!(t.gains.track_gain, Some(-648));
    assert_eq!(t.gains.track_peak, Some(988_525));
    assert_eq!(t.gapless, Some(Gapless { delay: 0x210, padding: 0x3C4 }));
    let p = t.picture.unwrap();
    assert_eq!((p.mime, p.kind), (Mime::Jpeg, 3));
    assert_eq!(at(&file, &p.at), JPEG);
    let l = t.lyrics.unwrap();
    assert_eq!(l.kind, LyricsKind::Unsynced(Encoding::Latin1));
    assert_eq!(at(&file, &l.at), b"Some lyrics");
    for (cap, chunk) in [(64 * 1024, 64 * 1024), (600, 7), (1024, 100)] {
        assert_eq!(read_ring(&file, Format::Mpeg, cap, chunk), t, "ring {cap}/{chunk}");
    }
}

#[test]
fn id3v24_unsync_frame_and_dli() {
    // The picture data contains FF E0 / FF D9: unsynchronised as FF 00 E0.
    let mut raw = std::vec![0u8];
    raw.extend_from_slice(b"image/jpeg\0\x03\0");
    raw.extend_from_slice(JPEG);
    let logical_len = raw.len() as u32;
    let mut body = synchsafe(logical_len).to_vec();
    for (i, &b) in raw.iter().enumerate() {
        body.push(b);
        if b == 0xFF && raw.get(i + 1).is_some_and(|&n| n & 0xE0 == 0xE0 || n == 0) {
            body.push(0);
        }
    }
    let frames = std::vec![frame(4, b"TIT2", 0, b"\x03T\xC3\xADtulo"), frame(4, b"APIC", 0x0003, &body)];
    let file = tag(4, 0, &frames, 0);
    let t = read(&file, Format::Mpeg);
    assert_eq!(t.title.as_str(), "Título");
    let p = t.picture.unwrap();
    assert!(p.at.unsync);
    assert_eq!(at(&file, &p.at), JPEG);
    assert_eq!(read_ring(&file, Format::Mpeg, 300, 11), t);
}

#[test]
fn id3v23_whole_tag_unsync() {
    let frames = v23_frames();
    let plain: Vec<u8> = frames.iter().flatten().copied().collect();
    let mut body = Vec::new();
    for (i, &b) in plain.iter().enumerate() {
        body.push(b);
        if b == 0xFF && plain.get(i + 1).is_none_or(|&n| n & 0xE0 == 0xE0 || n == 0) {
            body.push(0);
        }
    }
    let mut file = std::vec![b'I', b'D', b'3', 3, 0, 0x80];
    file.extend_from_slice(&synchsafe(body.len() as u32));
    file.extend_from_slice(&body);
    let t = read(&file, Format::Mpeg);
    assert_eq!(t.title.as_str(), "Café au lait");
    assert_eq!(t.artist.as_str(), "Art");
    let p = t.picture.unwrap();
    assert!(p.at.unsync);
    assert_eq!(at(&file, &p.at), JPEG);
    assert_eq!(read_ring(&file, Format::Mpeg, 256, 5), t);
}

#[test]
fn id3v22() {
    let f = |id: &[u8; 3], b: &[u8]| {
        let mut v = id.to_vec();
        v.extend_from_slice(&(b.len() as u32).to_be_bytes()[1..]);
        v.extend_from_slice(b);
        v
    };
    let mut pic = std::vec![0u8];
    pic.extend_from_slice(b"PNG\x03\0");
    pic.extend_from_slice(b"\x89PNG");
    let frames = std::vec![f(b"TT2", b"\0Old"), f(b"TP1", b"\0Timer"), f(b"PIC", &pic)];
    let file = tag(2, 0, &frames, 0);
    let t = read(&file, Format::Mpeg);
    assert_eq!((t.title.as_str(), t.artist.as_str()), ("Old", "Timer"));
    assert_eq!(t.picture.unwrap().mime, Mime::Png);
    assert_eq!(at(&file, &t.picture.unwrap().at), b"\x89PNG");
}

fn ape_items(items: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (k, v, flags) in items {
        body.extend_from_slice(&(v.len() as u32).to_le_bytes());
        body.extend_from_slice(&flags.to_le_bytes());
        body.extend_from_slice(k.as_bytes());
        body.push(0);
        body.extend_from_slice(v);
    }
    let mut f = body.clone();
    f.extend_from_slice(b"APETAGEX");
    f.extend_from_slice(&2000u32.to_le_bytes());
    f.extend_from_slice(&(body.len() as u32 + 32).to_le_bytes());
    f.extend_from_slice(&(items.len() as u32).to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes());
    f.extend_from_slice(&[0; 8]);
    f
}

fn id3v1(title: &str, track: u8, genre: u8) -> Vec<u8> {
    let mut v = std::vec![0u8; 128];
    v[..3].copy_from_slice(b"TAG");
    v[3..3 + title.len()].copy_from_slice(title.as_bytes());
    v[33..38].copy_from_slice(b"V1art");
    v[93..97].copy_from_slice(b"2001");
    v[126] = track;
    v[127] = genre;
    v
}

#[test]
fn trailing_ape_and_id3v1() {
    let mut file = tag(3, 0, &[frame(3, b"TIT2", 0, b"\0Lead")], 0);
    file.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00, 1, 2, 3, 4]);
    let mut cover = b"front.jpg\0".to_vec();
    cover.extend_from_slice(JPEG);
    file.extend_from_slice(&ape_items(&[
        ("Title", b"Ape title", 0),
        ("Album", b"Ape album", 0),
        ("REPLAYGAIN_ALBUM_GAIN", b"+2.10 dB", 0),
        ("Cover Art (Front)", &cover, 2),
        ("Lyrics", b"[00:01.00]la", 0),
    ]));
    file.extend_from_slice(&id3v1("V1 title", 7, 8));
    let t = read(&file, Format::Mpeg);
    // ID3v2 wins, APE fills, ID3v1 fills what is left.
    assert_eq!(t.title.as_str(), "Lead");
    assert_eq!(t.album.as_str(), "Ape album");
    assert_eq!(t.artist.as_str(), "V1art");
    assert_eq!((t.year, t.track), (2001, 7));
    assert_eq!(t.genre.as_str(), "Jazz");
    assert_eq!(t.gains.album_gain, Some(210));
    assert_eq!(at(&file, &t.picture.unwrap().at), JPEG);
    assert_eq!(at(&file, &t.lyrics.unwrap().at), b"[00:01.00]la");
    assert_eq!(read_ring(&file, Format::Mpeg, 512, 64), t);
}

#[test]
fn flac_comments_and_picture() {
    let mut f = b"fLaC".to_vec();
    f.extend_from_slice(&[0x00, 0, 0, 34]);
    f.extend_from_slice(&[0; 34]);
    let mut vc = Vec::new();
    vc.extend_from_slice(&7u32.to_le_bytes());
    vc.extend_from_slice(b"vendor!");
    let items: [&[u8]; 6] = [
        b"TITLE=Flac title",
        b"artist=Someone",
        b"TRACKNUMBER=5",
        b"TRACKTOTAL=9",
        b"R128_TRACK_GAIN=-512",
        b"LYRICS=[00:00.10]hi",
    ];
    vc.extend_from_slice(&(items.len() as u32).to_le_bytes());
    for i in items {
        vc.extend_from_slice(&(i.len() as u32).to_le_bytes());
        vc.extend_from_slice(i);
    }
    f.push(4);
    f.extend_from_slice(&(vc.len() as u32).to_be_bytes()[1..]);
    f.extend_from_slice(&vc);
    let mut pic = Vec::new();
    pic.extend_from_slice(&3u32.to_be_bytes());
    pic.extend_from_slice(&9u32.to_be_bytes());
    pic.extend_from_slice(b"image/png");
    pic.extend_from_slice(&4u32.to_be_bytes());
    pic.extend_from_slice(b"desc");
    pic.extend_from_slice(&[0; 16]);
    pic.extend_from_slice(&4u32.to_be_bytes());
    pic.extend_from_slice(b"\x89PNG");
    f.push(0x80 | 6);
    f.extend_from_slice(&(pic.len() as u32).to_be_bytes()[1..]);
    f.extend_from_slice(&pic);
    f.extend_from_slice(&[0xFF, 0xF8, 0, 0]);
    let t = read(&f, Format::Flac);
    assert_eq!((t.title.as_str(), t.artist.as_str()), ("Flac title", "Someone"));
    assert_eq!((t.track, t.tracks), (5, 9));
    assert_eq!(t.gains.r128_track, Some(-200));
    assert_eq!(t.picture.unwrap().mime, Mime::Png);
    assert_eq!(at(&f, &t.picture.unwrap().at), b"\x89PNG");
    assert_eq!(at(&f, &t.lyrics.unwrap().at), b"[00:00.10]hi");
    assert_eq!(read_ring(&f, Format::Flac, 200, 3), t);
}

#[test]
fn wav_info() {
    let mut info = b"INFO".to_vec();
    for (id, v) in [(b"INAM", &b"Wav name\0"[..]), (b"IART", b"Wav artist\0"), (b"ITRK", b"4\0")] {
        info.extend_from_slice(id);
        info.extend_from_slice(&(v.len() as u32).to_le_bytes());
        info.extend_from_slice(v);
        if v.len() % 2 == 1 {
            info.push(0);
        }
    }
    let mut f = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
    f.extend_from_slice(&16u32.to_le_bytes());
    f.extend_from_slice(&[0; 16]);
    f.extend_from_slice(b"data");
    f.extend_from_slice(&4u32.to_le_bytes());
    f.extend_from_slice(&[1, 2, 3, 4]);
    f.extend_from_slice(b"LIST");
    f.extend_from_slice(&(info.len() as u32).to_le_bytes());
    f.extend_from_slice(&info);
    let t = read(&f, Format::Wav);
    assert_eq!((t.title.as_str(), t.artist.as_str(), t.track), ("Wav name", "Wav artist", 4));
}

#[test]
fn garbage_never_panics_or_spins() {
    let seed = tag(3, 0, &v23_frames(), 10);
    let mut x = 0x2545_F491u32;
    for _ in 0..3000 {
        let mut f = seed.clone();
        for _ in 0..1 + (x % 8) {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let i = x as usize % f.len();
            f[i] = (x >> 8) as u8;
        }
        f.truncate(1 + (x as usize >> 4) % f.len());
        for fmt in [Format::Mpeg, Format::Flac, Format::Wav, Format::Aiff] {
            let _ = read(&f, fmt);
            let _ = read_ring(&f, fmt, 256, 17);
        }
    }
}

/// Files from the generated corpus, when present.
#[test]
fn corpus_files() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus");
    let Ok(apic) = std::fs::read(dir.join("mp3/apic_128k.mp3")) else { return };
    let mut src = SliceSource::new(&apic);
    let t = read_all(&mut src, Format::Mpeg, Opts::default()).unwrap();
    assert_eq!(t.title.as_str(), "With picture");
    assert_eq!(&at(&apic, &t.picture.unwrap().at)[..4], b"\x89PNG");
    if let Ok(f) = std::fs::read(dir.join("flac/tagged_44100_2.flac")) {
        let t = read(&f, Format::Flac);
        assert_eq!((t.title.as_str(), t.artist.as_str()), ("Tagged", "Yoton"));
        assert_eq!(&at(&f, &t.picture.unwrap().at)[..4], b"\x89PNG");
    }
    if let Ok(f) = std::fs::read(dir.join("mp3/id3v1_128k.mp3")) {
        assert_eq!(read(&f, Format::Mpeg).title.as_str(), "T1");
    }
    if let Ok(f) = std::fs::read(dir.join("wav/tagged_44100_2.wav")) {
        let t = read(&f, Format::Wav);
        assert_eq!((t.title.as_str(), t.artist.as_str()), ("Tagged", "Yoton"));
    }
}

#[test]
fn sizes() {
    let (t, r) = (core::mem::size_of::<Tags>(), core::mem::size_of::<TagReader>());
    std::eprintln!("Tags {t} B, TagReader {r} B");
    assert!(t <= 1024 && r <= 1024);
}
