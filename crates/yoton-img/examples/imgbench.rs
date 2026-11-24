//! Host speed and arena high-water marks: every picture in a directory
//! decoded into a 128 × 128 screen (contain fit) and into a 40 × 30
//! thumbnail, with the 104 KiB Pictures arena.
//!
//! `cargo run -p yoton-img --release --example imgbench [DIR]`; DIR
//! defaults to the pictures the tests generate (`target/tmp/yoton-img/data-*`).

use std::time::Instant;

use yoton_img::{
    Decoder, Flow, Frame, Options, Orient, Placement, Rgb, RowSink, Scaler, SliceSource, Target,
};

struct Spy<'s, 'b, 'f> {
    inner: &'s mut Scaler<'b, 'f>,
    frame: Option<Frame>,
}

impl RowSink for Spy<'_, '_, '_> {
    fn wanted(&self, w: u32, h: u32, o: Orient) -> (u32, u32) {
        self.inner.wanted(w, h, o)
    }
    fn begin(&mut self, f: &Frame) {
        self.frame = Some(*f);
        self.inner.begin(f);
    }
    fn row(&mut self, y: u32, px: &[Rgb]) -> Flow {
        self.inner.row(y, px)
    }
}

/// Decode once; returns (delivered frame, arena bytes used).
fn once(data: &[u8], w: u16, h: u16, arena: &mut [u8]) -> (Option<Frame>, usize, bool) {
    let mut fb = vec![0u16; usize::from(w) * usize::from(h)];
    let mut work = vec![0u32; Scaler::need_words(usize::from(w.max(h)))];
    let mut sc = Scaler::new(
        &mut work,
        Placement::fit(w, h),
        Target { buf: &mut fb, stride: usize::from(w), x: 0, y: 0 },
    )
    .expect("scaler");
    let mut spy = Spy { inner: &mut sc, frame: None };
    let mut dec = Decoder::new(arena, Options::default());
    let ok = dec.run(&mut SliceSource::new(data), &mut spy).is_ok();
    (spy.frame, dec.arena_used(), ok)
}

fn main() {
    let dir = std::env::args().nth(1).map(std::path::PathBuf::from).unwrap_or_else(|| {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/yoton-img");
        std::fs::read_dir(&root)
            .ok()
            .and_then(|r| {
                r.flatten()
                    .map(|e| e.path())
                    .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("data-")))
            })
            .expect("no pictures: run `cargo test -p yoton-img` first or pass a directory")
    });
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| ["jpg", "png", "gif", "bmp", "qoi"].contains(&&*e.to_string_lossy()))
        })
        .collect();
    names.sort();
    let mut arena = vec![0u8; 104 * 1024];
    println!(
        "{:<26} {:>10} {:>11} {:>8} {:>9} {:>8} {:>11} {:>8}",
        "file", "size", "screen", "arena B", "ms", "MPix/s", "thumb", "ms"
    );
    for p in names {
        let data = std::fs::read(&p).unwrap();
        let (frame, used, ok) = once(&data, 128, 128, &mut arena);
        let Some(f) = frame else { continue };
        let reps = (2_000_000 / (u64::from(f.full_width) * u64::from(f.full_height)).max(1)).clamp(1, 200);
        let t = Instant::now();
        for _ in 0..reps {
            once(&data, 128, 128, &mut arena);
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0 / reps as f64;
        let mpix = f64::from(f.full_width) * f64::from(f.full_height) / ms / 1000.0;
        let (tf, _, _) = once(&data, 40, 30, &mut arena);
        let t = Instant::now();
        for _ in 0..reps {
            once(&data, 40, 30, &mut arena);
        }
        let tms = t.elapsed().as_secs_f64() * 1000.0 / reps as f64;
        let tf = tf.unwrap();
        println!(
            "{:<26} {:>10} {:>11} {:>8} {:>9.3} {:>8.1} {:>11} {:>8.3}{}",
            p.file_name().unwrap().to_string_lossy(),
            format!("{}x{}", f.full_width, f.full_height),
            format!("{}x{}", f.width, f.height),
            used,
            ms,
            mpix,
            format!("{}x{}", tf.width, tf.height),
            tms,
            if ok { "" } else { "  (error)" }
        );
    }
}
