//! Pictures through the scaler into a 128 × 128 RGB565 screen (the Pictures
//! viewer), compared with a host reference downscale (05 §7.6 acceptance:
//! PSNR ≥ 30 dB); the album-art path from a tag to the thumbnail cache; a
//! contact sheet to look at in `target/tmp/yoton-img/out/`.

#![allow(clippy::drop_non_drop)] // ends the scaler's borrow of the buffer before reading it

mod common;

use common::*;
use yoton_img::thumb::{self, CacheFile, ThumbKey, ThumbMeta};
use yoton_img::{
    Decoder, Fit, Flow, Frame, Options, Orient, Placement, Rgb, RowSink, Scaler, SliceSource, Target, scale,
};

/// Passes everything to the scaler, remembering the frame and what the
/// scaler wanted.
struct Spy<'s, 'b, 'f> {
    inner: &'s mut Scaler<'b, 'f>,
    frame: Option<Frame>,
    wanted: (u32, u32),
}

impl RowSink for Spy<'_, '_, '_> {
    fn wanted(&self, w: u32, h: u32, o: Orient) -> (u32, u32) {
        self.inner.wanted(w, h, o)
    }
    fn begin(&mut self, f: &Frame) {
        self.frame = Some(*f);
        self.wanted = self.inner.wanted(f.full_width, f.full_height, f.orient);
        self.inner.begin(f);
    }
    fn row(&mut self, y: u32, px: &[Rgb]) -> Flow {
        self.inner.row(y, px)
    }
}

struct Rendered {
    fb: Vec<u16>,
    lay: scale::Layout,
    info: yoton_img::Info,
    /// Delivered below what the scaler wanted, or point-sampled.
    reduced: bool,
}

/// Decode `data` into a `w` × `h` RGB565 box.
fn render(data: &[u8], p: Placement, arena: usize) -> Rendered {
    let mut fb = vec![0u16; usize::from(p.w) * usize::from(p.h)];
    let mut work = vec![0u32; Scaler::need_words(usize::from(p.w.max(p.h)))];
    let mut mem = vec![0u8; arena];
    let mut sc =
        Scaler::new(&mut work, p, Target { buf: &mut fb, stride: usize::from(p.w), x: 0, y: 0 }).unwrap();
    let mut spy = Spy { inner: &mut sc, frame: None, wanted: (0, 0) };
    let mut dec = Decoder::new(&mut mem, Options { background: BG });
    dec.run(&mut SliceSource::new(data), &mut spy).unwrap();
    let f = spy.frame.unwrap();
    let reduced = f.approximate || f.width < spy.wanted.0 || f.height < spy.wanted.1;
    assert!(sc.complete());
    let lay = sc.layout();
    drop(sc);
    Rendered { fb, lay, info: dec.info().unwrap(), reduced }
}

/// The reference: full reference picture, oriented, laid out like the
/// scaler does, area-averaged (or bilinear when enlarging).
fn reference(c: &Case, p: &Placement, orient: Orient) -> Vec<[f64; 3]> {
    let r = c.reference();
    let (ow, oh) = orient.size(c.w, c.h);
    let mut o = vec![[0u8; 3]; r.len()];
    for y in 0..c.h {
        for x in 0..c.w {
            let (a, b) = orient.apply(x, y, c.w, c.h);
            o[(b * ow + a) as usize] = r[(y * c.w + x) as usize];
        }
    }
    let lay = scale::layout(p, ow, oh);
    // Crop to the visible part.
    let mut crop: Vec<Rgb> = Vec::new();
    for y in lay.ry..lay.ry + lay.rh {
        for x in lay.rx..lay.rx + lay.rw {
            crop.push(o[(y * ow + x) as usize]);
        }
    }
    let img = resample(&crop, lay.rw, lay.rh, lay.ow, lay.oh);
    let bg = [f64::from(p.background[0]), f64::from(p.background[1]), f64::from(p.background[2])];
    let mut out = vec![bg; usize::from(p.w) * usize::from(p.h)];
    for y in 0..lay.oh {
        for x in 0..lay.ow {
            out[((lay.dy + y) * u32::from(p.w) + lay.dx + x) as usize] = img[(y * lay.ow + x) as usize];
        }
    }
    out
}

#[test]
fn screen_golden_psnr() {
    let mut sheet: Vec<(String, Vec<Rgb>)> = Vec::new();
    let mut report = String::from("\nfile                       mode      PSNR dB\n");
    let mut failed = Vec::new();
    for c in cases() {
        for fit in [Fit::Contain, Fit::Cover, Fit::Actual { x: 20, y: 10 }] {
            let p = Placement { dither: false, background: [0, 0, 0], ..Placement::fit(128, 128) };
            let p = Placement { fit, ..p };
            let r = render(&c.data, p, ARENA);
            let want = reference(&c, &p, r.info.orient);
            let got: Vec<Rgb> = r.fb.iter().map(|&v| from565(v)).collect();
            let db = psnr(&got, &want);
            let mode = format!("{fit:?}");
            let mode = mode.split(' ').next().unwrap();
            let note = if r.reduced { "  (reduced: arena or point-sampled)" } else { "" };
            report += &format!("{:<26} {mode:<8} {db:6.1}{note}\n", c.name);
            // RGB565 rounding alone limits this to about 40 dB. Pictures
            // the decoder had to reduce below what the box wants are only
            // checked for being the right picture.
            let min = if r.reduced { 20.0 } else { 30.0 };
            if db < min {
                failed.push(format!("{} {fit:?}: {db:.1} dB", c.name));
            }
            if fit == Fit::Contain {
                sheet.push((c.name.clone(), got));
            }
        }
    }
    println!("{report}");
    assert!(failed.is_empty(), "{failed:#?}");
    // Contact sheet of the contain-fit pictures, 8 per row.
    let cols = 8u32;
    let rows = (sheet.len() as u32).div_ceil(cols);
    let (w, h) = (cols * 130, rows * 130);
    let mut px = vec![[255u8, 255, 255]; (w * h) as usize];
    for (i, (_, img)) in sheet.iter().enumerate() {
        let (ox, oy) = ((i as u32 % cols) * 130 + 1, (i as u32 / cols) * 130 + 1);
        for y in 0..128 {
            for x in 0..128 {
                px[((oy + y) * w + ox + x) as usize] = img[(y * 128 + x) as usize];
            }
        }
    }
    let names: Vec<&str> = sheet.iter().map(|(n, _)| n.as_str()).collect();
    std::fs::write(out_dir().join("sheet.txt"), names.join("\n")).unwrap();
    save_png(&out_dir().join("sheet.png"), w, h, &px);
}

#[test]
fn exif_and_user_rotation() {
    let cs = cases();
    let c = cs.iter().find(|c| c.name == "j_exif6.jpg").unwrap();
    let base = Placement { dither: false, ..Placement::fit(128, 128) };
    let check = |p: Placement, o: Orient| {
        let r = render(&c.data, p, ARENA);
        let got: Vec<Rgb> = r.fb.iter().map(|&v| from565(v)).collect();
        let db = psnr(&got, &reference(c, &p, o));
        assert!(db >= 30.0, "{p:?}: {db:.1}");
        r
    };
    // EXIF 6 honoured: displayed upright (rotated 90° CW), portrait.
    let r = check(base, Orient::CW90);
    assert_eq!(r.info.orient, Orient::from_exif(6));
    assert!(r.lay.oh > r.lay.ow, "{:?}", r.lay);
    // EXIF ignored plus a user rotation of 90° CCW.
    check(Placement { exif: false, rotate: Orient::CCW90, ..base }, Orient::CCW90);
    // Both: EXIF 6 then CCW90 is the stored orientation.
    check(Placement { rotate: Orient::CCW90, ..base }, Orient::NORMAL);
    // 180° and mirrored variants.
    check(Placement { exif: false, rotate: Orient::R180, ..base }, Orient::R180);
    check(Placement { exif: false, rotate: Orient::from_exif(5), ..base }, Orient::from_exif(5));
}

#[test]
fn dithered_close_to_reference() {
    for c in cases().iter().filter(|c| c.name == "j_420_q85.jpg" || c.name == "p_rgb8.png") {
        let p = Placement::fit(128, 128);
        let r = render(&c.data, p, ARENA);
        let got: Vec<Rgb> = r.fb.iter().map(|&v| from565(v)).collect();
        let db = psnr(&got, &reference(c, &p, Orient::NORMAL));
        assert!(db >= 30.0, "{}: {db:.1}", c.name);
    }
}

/// An in-memory bucket file.
#[derive(Default)]
struct MemFile(Vec<u8>);

impl CacheFile for MemFile {
    type Error = ();
    fn read_at(&mut self, off: u32, buf: &mut [u8]) -> Result<usize, ()> {
        let off = (off as usize).min(self.0.len());
        let n = buf.len().min(self.0.len() - off);
        buf[..n].copy_from_slice(&self.0[off..off + n]);
        Ok(n)
    }
    fn write_at(&mut self, off: u32, data: &[u8]) -> Result<(), ()> {
        let end = off as usize + data.len();
        if self.0.len() < end {
            self.0.resize(end, 0);
        }
        self.0[off as usize..end].copy_from_slice(data);
        Ok(())
    }
}

fn synchsafe(n: u32) -> [u8; 4] {
    [(n >> 21) as u8 & 0x7F, (n >> 14) as u8 & 0x7F, (n >> 7) as u8 & 0x7F, n as u8 & 0x7F]
}

/// An MP3-like file with an ID3v2.4 tag holding `pic` in an APIC frame
/// (unsynchronised when `unsync`).
fn mp3_with_art(pic: &[u8], unsync: bool) -> Vec<u8> {
    let mut raw = vec![0u8];
    raw.extend_from_slice(b"image/jpeg\0\x03cover\0");
    raw.extend_from_slice(pic);
    let mut body = Vec::new();
    let mut flags = 0u16;
    if unsync {
        flags = 0x0003; // unsynchronised, data length indicator
        body.extend_from_slice(&synchsafe(raw.len() as u32));
        for (i, &b) in raw.iter().enumerate() {
            body.push(b);
            if b == 0xFF && raw.get(i + 1).is_none_or(|&n| n & 0xE0 == 0xE0 || n == 0) {
                body.push(0);
            }
        }
    } else {
        body = raw;
    }
    let mut frame = b"APIC".to_vec();
    frame.extend_from_slice(&synchsafe(body.len() as u32));
    frame.extend_from_slice(&flags.to_be_bytes());
    frame.extend_from_slice(&body);
    let mut f = vec![b'I', b'D', b'3', 4, 0, 0];
    f.extend_from_slice(&synchsafe(frame.len() as u32));
    f.extend_from_slice(&frame);
    // A few MPEG frame headers' worth of audio.
    for _ in 0..4 {
        f.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]);
        f.extend_from_slice(&[0u8; 413]);
    }
    f
}

#[test]
fn album_art_from_tag_to_cache() {
    let cs = cases();
    let c = cs.iter().find(|c| c.name == "j_odd_420.jpg").unwrap();
    for unsync in [false, true] {
        let file = mp3_with_art(&c.data, unsync);
        let tags =
            yoton_meta::read_all(&mut SliceSource::new(&file), yoton_audio::Format::Mpeg, Default::default())
                .unwrap();
        let pic = tags.picture.expect("picture");
        assert_eq!(pic.at.unsync, unsync);
        // The art path: seek to the picture, narrow the source, decode into
        // a 52 × 52 thumbnail.
        let mut src = SliceSource::new(&file);
        yoton_audio::SeekSource::seek(&mut src, pic.at.offset).unwrap();
        let mut mem = vec![0u8; 40 * 1024];
        let (stage, arena) = mem.split_at_mut(yoton_img::MAX_PEEK);
        let mut art = if pic.at.unsync {
            yoton_img::source::ArtSource::unsync(&mut src, u64::from(pic.at.len), pic.at.skip, stage)
        } else {
            yoton_img::source::ArtSource::plain(&mut src, u64::from(pic.at.len))
        };
        let p = Placement { fit: Fit::Cover, ..Placement::fit(thumb::ART.w.into(), thumb::ART.h.into()) };
        let mut px = vec![0u16; 52 * 52];
        let mut work = vec![0u32; Scaler::need_words(52)];
        let mut sc = Scaler::new(&mut work, p, Target { buf: &mut px, stride: 52, x: 0, y: 0 }).unwrap();
        let mut dec = Decoder::new(arena, Options::default());
        loop {
            art.pump();
            match dec.step(&mut art, &mut sc).unwrap() {
                yoton_img::Step::Done => break,
                yoton_img::Step::Progress => {}
                yoton_img::Step::NeedData(_) => panic!("all data was there"),
            }
        }
        assert!(sc.complete());
        drop(sc);
        // 333×221 for 52×52 → decoded at 1/2 or 1/4 with a small arena.
        let used = dec.arena_used();
        assert!(used < 40 * 1024 - yoton_img::MAX_PEEK, "{used}");
        let reference_px = {
            let want = reference(c, &p, Orient::NORMAL);
            let got: Vec<Rgb> = px.iter().map(|&v| from565(v)).collect();
            psnr(&got, &want)
        };
        assert!(reference_px >= 28.0, "art PSNR {reference_px:.1}");
        // Into the cache and back.
        let key = ThumbKey {
            locator: [7; 16],
            file_size: file.len() as u64,
            mtime: 0x5A5A_0001,
            part: pic.at.offset as u32,
            w: 52,
            h: 52,
            rotation: 1,
            reserved: 0,
        };
        let mut bucket = MemFile::default();
        let meta = ThumbMeta { orig_w: c.w as u16, orig_h: c.h as u16, approximate: false };
        thumb::ART.store(&mut bucket, &key, &meta, &px).unwrap();
        let mut back = vec![0u16; 52 * 52];
        assert_eq!(thumb::ART.lookup(&mut bucket, &key, &mut back), Ok(Some(meta)));
        assert_eq!(back, px);
    }
}
