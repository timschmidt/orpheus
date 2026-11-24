//! Shared test support: the generated pictures, capture sink, feeding
//! modes and image comparison.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;

use orpheus_img::{
    ByteSource, Decoder, Error, Flow, Frame, Options, Orient, Rgb, RowSink, SliceSource, Step,
};

/// Background used for alpha blending in all tests.
pub const BG: Rgb = [0x20, 0x40, 0x60];

/// The decoder arena of the Pictures mode (02 §1.3).
pub const ARENA: usize = 104 * 1024;

const GEN: &str = include_str!("../gen.py");

fn fnv(s: &str) -> u32 {
    s.bytes().fold(0x811C_9DC5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// Directory holding the generated pictures (generated on first use; the
/// name carries a hash of `gen.py`, so editing it regenerates).
pub fn data_dir() -> PathBuf {
    // Once per process: the tests of one binary run on several threads,
    // and they would otherwise generate into the same `gen-<pid>` folder
    // and delete each other's files.
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(generate).clone()
}

fn generate() -> PathBuf {
    if let Ok(d) = std::env::var("ORPHEUS_IMG_DATA") {
        return PathBuf::from(d);
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/orpheus-img");
    let dir = root.join(format!("data-{:08x}", fnv(GEN)));
    if dir.join("manifest.txt").exists() {
        return dir;
    }
    std::fs::create_dir_all(&root).unwrap();
    let tmp = root.join(format!("gen-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/gen.py");
    let st = Command::new("python3").arg(&script).arg(&tmp).status().expect(
        "python3 with numpy and Pillow, ImageMagick, cjpeg and ffmpeg are needed to generate the test pictures",
    );
    assert!(st.success(), "tests/gen.py failed");
    // Another test binary may have finished first; either copy is fine.
    if std::fs::rename(&tmp, &dir).is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    dir
}

pub struct Case {
    pub name: String,
    pub data: Vec<u8>,
    pub w: u32,
    pub h: u32,
    /// Reference RGBA.
    pub rgba: Vec<u8>,
    pub exact: bool,
    /// Allowed difference per channel for "exact" pictures.
    pub tol: u8,
}

impl Case {
    /// Reference blended over [`BG`] exactly as the decoders blend.
    pub fn reference(&self) -> Vec<Rgb> {
        self.rgba.chunks(4).map(|p| orpheus_img::blend([p[0], p[1], p[2]], p[3], BG)).collect()
    }

    pub fn is(&self, ext: &str) -> bool {
        self.name.ends_with(ext)
    }
}

pub fn cases() -> Vec<Case> {
    let dir = data_dir();
    let m = std::fs::read_to_string(dir.join("manifest.txt")).unwrap();
    let mut v = Vec::new();
    for line in m.lines().filter(|l| !l.is_empty()) {
        let mut it = line.split(' ');
        let (f, r, k) = (it.next().unwrap(), it.next().unwrap(), it.next().unwrap());
        let data = std::fs::read(dir.join(f)).unwrap();
        let rf = std::fs::read(dir.join(r)).unwrap();
        let w = u32::from_le_bytes(rf[0..4].try_into().unwrap());
        let h = u32::from_le_bytes(rf[4..8].try_into().unwrap());
        let (exact, tol) = (k != "lossy", u8::from(k == "near"));
        v.push(Case { name: f.to_string(), data, w, h, rgba: rf[8..].to_vec(), exact, tol });
    }
    v
}

/// Collects every delivered row.
#[derive(Default)]
pub struct Capture {
    /// What `wanted` answers (None: the full size).
    pub want: Option<(u32, u32)>,
    pub frame: Option<Frame>,
    pub rows: Vec<Rgb>,
    pub nrows: u32,
    /// Ask to stop after this many rows.
    pub stop_after: Option<u32>,
}

impl RowSink for Capture {
    fn wanted(&self, w: u32, h: u32, _o: Orient) -> (u32, u32) {
        self.want.unwrap_or((w, h))
    }
    fn begin(&mut self, f: &Frame) {
        assert!(self.frame.is_none(), "begin twice");
        self.frame = Some(*f);
    }
    fn row(&mut self, y: u32, px: &[Rgb]) -> Flow {
        let f = self.frame.expect("row before begin");
        assert_eq!(y, self.nrows, "rows out of order");
        assert!(y < f.height, "too many rows");
        assert_eq!(px.len(), f.width as usize, "row width");
        self.rows.extend_from_slice(px);
        self.nrows += 1;
        if self.stop_after.is_some_and(|n| self.nrows >= n) { Flow::Stop } else { Flow::Continue }
    }
}

impl Capture {
    /// Rows in top-down order (BMP delivers bottom-up).
    pub fn top_down(&self) -> Vec<Rgb> {
        let f = self.frame.unwrap();
        if !f.bottom_up {
            return self.rows.clone();
        }
        let w = f.width as usize;
        self.rows.chunks(w).rev().flatten().copied().collect()
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Feed {
    /// Everything available at once.
    All,
    /// A ring of this capacity, refilled on `NeedData`.
    Ring(usize),
    /// A ring of this capacity, `n` bytes added per `NeedData`.
    Trickle(usize, usize),
}

pub struct Outcome {
    pub result: Result<(), Error>,
    pub used: usize,
    pub info: Option<orpheus_img::Info>,
}

/// Decode `data` through `sink` with an arena of `arena` bytes.
pub fn decode(data: &[u8], arena: usize, sink: &mut dyn RowSink, feed: Feed) -> Outcome {
    let mut mem = vec![0u8; arena];
    let mut dec = Decoder::new(&mut mem, Options { background: BG });
    let mut src = match feed {
        Feed::All => SliceSource::new(data),
        Feed::Ring(c) | Feed::Trickle(c, _) => SliceSource::ring(data, c),
    };
    let mut result = Ok(());
    for _ in 0..50_000_000u64 {
        match dec.step(&mut src, sink) {
            Ok(Step::Done) => break,
            Ok(Step::Progress) => {}
            Ok(Step::NeedData(n)) => {
                assert!(n <= orpheus_img::MAX_PEEK, "NeedData({n}) beyond MAX_PEEK");
                assert!(
                    n > src.available() && !src.at_end(),
                    "NeedData({n}) with {} buffered (end {})",
                    src.available(),
                    src.at_end()
                );
                match feed {
                    Feed::All => panic!("NeedData with everything buffered"),
                    Feed::Ring(_) => src.refill(),
                    Feed::Trickle(_, k) => src.trickle(k),
                }
            }
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    Outcome { result, used: dec.arena_used(), info: dec.info() }
}

/// PSNR in dB between two equally long pixel lists.
pub fn psnr(a: &[Rgb], b: &[[f64; 3]]) -> f64 {
    assert_eq!(a.len(), b.len());
    let mut se = 0.0;
    for (p, q) in a.iter().zip(b) {
        for c in 0..3 {
            let d = f64::from(p[c]) - q[c];
            se += d * d;
        }
    }
    let mse = se / (a.len() as f64 * 3.0);
    if mse == 0.0 { f64::INFINITY } else { 10.0 * (255.0 * 255.0 / mse).log10() }
}

pub fn to_f(a: &[Rgb]) -> Vec<[f64; 3]> {
    a.iter().map(|p| [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]).collect()
}

/// Average `s` × `s` blocks (partial blocks at the right and bottom edges).
pub fn box_down(px: &[Rgb], w: u32, h: u32, s: u32) -> (u32, u32, Vec<[f64; 3]>) {
    let (dw, dh) = (w.div_ceil(s), h.div_ceil(s));
    let mut out = Vec::with_capacity((dw * dh) as usize);
    for by in 0..dh {
        for bx in 0..dw {
            let mut acc = [0.0; 3];
            let mut n = 0.0;
            for y in by * s..((by + 1) * s).min(h) {
                for x in bx * s..((bx + 1) * s).min(w) {
                    let p = px[(y * w + x) as usize];
                    for c in 0..3 {
                        acc[c] += f64::from(p[c]);
                    }
                    n += 1.0;
                }
            }
            out.push([acc[0] / n, acc[1] / n, acc[2] / n]);
        }
    }
    (dw, dh, out)
}

/// Area-average resample of `px` (`w` × `h`) to `ow` × `oh`, or bilinear
/// (pixel centres aligned) where enlarging.
pub fn resample(px: &[Rgb], w: u32, h: u32, ow: u32, oh: u32) -> Vec<[f64; 3]> {
    let get = |x: u32, y: u32| px[(y * w + x) as usize];
    let axis = |n_in: u32, n_out: u32, i: u32| -> Vec<(u32, f64)> {
        if n_out <= n_in {
            let a = f64::from(i) * f64::from(n_in) / f64::from(n_out);
            let b = f64::from(i + 1) * f64::from(n_in) / f64::from(n_out);
            let mut v = Vec::new();
            let mut x = a.floor() as u32;
            while f64::from(x) < b && x < n_in {
                let lo = f64::from(x).max(a);
                let hi = f64::from(x + 1).min(b);
                if hi > lo {
                    v.push((x, (hi - lo) / (b - a)));
                }
                x += 1;
            }
            v
        } else {
            let p = ((f64::from(i) + 0.5) * f64::from(n_in) / f64::from(n_out) - 0.5)
                .clamp(0.0, f64::from(n_in - 1));
            let x0 = p.floor() as u32;
            let x1 = (x0 + 1).min(n_in - 1);
            let f = p - f64::from(x0);
            vec![(x0, 1.0 - f), (x1, f)]
        }
    };
    let mut out = Vec::with_capacity((ow * oh) as usize);
    for j in 0..oh {
        let ys = axis(h, oh, j);
        for i in 0..ow {
            let xs = axis(w, ow, i);
            let mut acc = [0.0; 3];
            for &(y, wy) in &ys {
                for &(x, wx) in &xs {
                    let p = get(x, y);
                    for c in 0..3 {
                        acc[c] += f64::from(p[c]) * wx * wy;
                    }
                }
            }
            out.push(acc);
        }
    }
    out
}

/// RGB565 to RGB888 by bit replication.
pub fn from565(c: u16) -> Rgb {
    let (r, g, b) = ((c >> 11) & 31, (c >> 5) & 63, c & 31);
    [((r << 3) | (r >> 2)) as u8, ((g << 2) | (g >> 4)) as u8, ((b << 3) | (b >> 2)) as u8]
}

/// Write an RGB picture as PNG (through ImageMagick) for looking at.
pub fn save_png(path: &std::path::Path, w: u32, h: u32, px: &[Rgb]) {
    let ppm = path.with_extension("ppm");
    let mut d = format!("P6\n{w} {h}\n255\n").into_bytes();
    for p in px {
        d.extend_from_slice(p);
    }
    std::fs::write(&ppm, d).unwrap();
    let ok = Command::new("magick").arg(&ppm).arg(path).status().is_ok_and(|s| s.success());
    if ok {
        let _ = std::fs::remove_file(&ppm);
    }
}

/// The output directory for pictures the tests produce.
pub fn out_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp/orpheus-img/out");
    std::fs::create_dir_all(&d).unwrap();
    d
}
