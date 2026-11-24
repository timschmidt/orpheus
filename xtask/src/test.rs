//! `xtask test [host|golden|codecs|fuzz-smoke|codec-size|all]` (07 §3).
//!
//! * `codecs [--gen] [--filter S] [--quick]`: the codec conformance suite
//!   (07 §3.3). `--gen` (re)builds the corpus in `tests/corpus/` with ffmpeg,
//!   sox and flac; it is also built when missing. The checks run in the
//!   `yoton-codec-conformance` binary with overflow checks on.
//! * `fuzz-smoke [--secs N]`: the stable mutation smoke run over every fuzz
//!   target, then each `cargo fuzz` target for N seconds (default 60) when a
//!   nightly toolchain and cargo-fuzz are installed (07 §3.4); inputs kept
//!   under `tests/regress/<target>/` are replayed first.
//! * `codec-size`: build each codec crate for `thumbv7em-none-eabihf`
//!   (release) and report its code and data size.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::args::Args;
use crate::util::{cargo, log, root, run};

pub fn cmd(args: &mut Args) -> Result<String> {
    let bless = args.flag("bless");
    let gen_corpus = args.flag("gen");
    let quick = args.flag("quick");
    let filter = args.value("filter");
    let secs: u64 = args.value("secs").and_then(|s| s.parse().ok()).unwrap_or(60);
    let which = args.next_positional().unwrap_or_else(|| "all".into());
    match which.as_str() {
        "host" => host(),
        "golden" => golden(bless),
        "codecs" => codecs(gen_corpus, filter.as_deref(), quick),
        "fuzz-smoke" => fuzz_smoke(secs),
        "codec-size" => codec_size(),
        "all" => {
            host()?;
            golden(false)?;
            let c = codecs(gen_corpus, None, false)?;
            let f = fuzz_smoke(secs)?;
            Ok(format!("host and golden tests passed; {c}; {f}"))
        }
        other => bail!("unknown test suite `{other}` (host, golden, codecs, fuzz-smoke, codec-size, all)"),
    }
}

fn host() -> Result<String> {
    run(cargo().current_dir(root()).args(["test", "--workspace", "--quiet"]))?;
    Ok("host tests passed".into())
}

fn golden(bless: bool) -> Result<String> {
    // Golden images are ordinary tests of the simulator crate; `--bless`
    // asks them to rewrite their reference PNGs.
    let sim = root().join("sim");
    if !sim.join("Cargo.toml").exists() {
        return Ok("no simulator yet (golden tests not run)".into());
    }
    let mut c = cargo();
    c.current_dir(root()).args(["test", "--quiet", "--package", "yoton-sim", "--test", "golden"]);
    if bless {
        c.env("YOTON_BLESS", "1");
    }
    run(&mut c)?;
    Ok(if bless { "golden images re-blessed".into() } else { "golden tests passed".into() })
}

// ---------------------------------------------------------------------------
// Codec conformance (07 §3.3)

fn corpus_dir() -> PathBuf {
    root().join("tests").join("corpus")
}

/// The conformance binary, built with overflow checks and debug assertions
/// (tests run with checks on, 02 §9.1) in its own target directory.
fn conformance() -> Command {
    let mut c = cargo();
    c.current_dir(root())
        .args(["run", "--quiet", "--release", "-p", "yoton-codec-conformance", "--target-dir"])
        .arg(root().join("target").join("conformance"))
        .env("CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS", "true")
        .env("CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS", "true")
        .env("CARGO_PROFILE_RELEASE_LTO", "false")
        .env("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "16")
        .env("CARGO_PROFILE_RELEASE_DEBUG", "1")
        .arg("--");
    c
}

fn codecs(regen: bool, filter: Option<&str>, quick: bool) -> Result<String> {
    let dir = corpus_dir();
    if regen || !dir.join("manifest.tsv").exists() {
        let n = generate(&dir)?;
        log(&format!("corpus: {n} files in {}", dir.display()));
    }
    let mut c = conformance();
    c.arg("run").arg(&dir);
    if let Some(f) = filter {
        c.args(["--filter", f]);
    }
    if quick {
        c.args(["--no-robust", "--seeks", "4"]);
    }
    run(&mut c)?;
    Ok("codec conformance passed".into())
}

// ---------------------------------------------------------------------------
// Corpus generator

/// The rates the DAC plays natively (04 §3.2).
const NATIVE: [u32; 9] = [8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000];

struct Gen {
    dir: PathBuf,
    manifest: String,
    files: usize,
}

fn tool(name: &str, args: &[String]) -> Result<()> {
    let out =
        Command::new(name).args(args).output().with_context(|| format!("spawn {name} (is it installed?)"))?;
    if !out.status.success() {
        bail!("{name} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_string()).collect()
}

fn p(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn ffmpeg(args: &[String]) -> Result<()> {
    let mut a = s(&["-v", "error", "-nostdin", "-y"]);
    a.extend_from_slice(args);
    tool("ffmpeg", &a)
}

fn sox(args: &[String]) -> Result<()> {
    let mut a = s(&["-R", "-V1"]);
    a.extend_from_slice(args);
    tool("sox", &a)
}

/// Sample frames in a WAV file (from `soxi`).
fn frames_of(path: &Path) -> Result<u64> {
    let out = Command::new("soxi").args(["-s", &p(path)]).output().context("spawn soxi")?;
    String::from_utf8_lossy(&out.stdout).trim().parse().with_context(|| format!("soxi -s {}", path.display()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ref {
    S16,
    S32,
}

#[derive(Clone, Copy)]
enum Sig {
    /// Two sweeps (one per channel) with pink noise mixed in.
    Mix,
    /// Logarithmic sine sweep 20 Hz–20 kHz.
    Sweep,
    Noise,
    /// Impulse train.
    Impulse,
    Silence,
    /// Harmonic tones with vibrato, an envelope and a little noise.
    Music,
    /// Full-scale square wave (clipping).
    Square,
}

impl Sig {
    fn name(self) -> &'static str {
        match self {
            Self::Mix => "mix",
            Self::Sweep => "sweep",
            Self::Noise => "noise",
            Self::Impulse => "impulse",
            Self::Silence => "silence",
            Self::Music => "music",
            Self::Square => "square",
        }
    }
}

impl Gen {
    fn path(&self, rel: &str) -> Result<PathBuf> {
        let p = self.dir.join(rel);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        Ok(p)
    }

    /// A source signal as a WAV file of `bits` (16, 24 or 8) bits.
    fn source(&self, sig: Sig, rate: u32, ch: u32, secs: f32, bits: u32) -> Result<PathBuf> {
        let rel = format!("src/{}_{rate}_{ch}_{bits}_{secs}.wav", sig.name());
        let out = self.path(&rel)?;
        if out.exists() {
            return Ok(out);
        }
        let d = format!("{secs}");
        let (r, c, b) = (rate.to_string(), ch.to_string(), bits.to_string());
        let nyq = (rate / 2 - 100).min(20000);
        match sig {
            Sig::Mix => sox(&[
                s(&["-n", "-r", &r, "-c", &c, "-b", &b]),
                vec![p(&out)],
                s(&["synth", &d, "sine", &format!("100-{nyq}"), "sine", &format!("{}-200", nyq * 3 / 4)]),
                s(&["synth", &d, "pinknoise", "mix", "gain", "-4"]),
            ]
            .concat())?,
            Sig::Sweep => sox(&[
                s(&["-n", "-r", &r, "-c", &c, "-b", &b]),
                vec![p(&out)],
                s(&["synth", &d, "sine", &format!("20:{nyq}"), "gain", "-1"]),
            ]
            .concat())?,
            Sig::Noise => sox(&[
                s(&["-n", "-r", &r, "-c", &c, "-b", &b]),
                vec![p(&out)],
                s(&["synth", &d, "whitenoise", "gain", "-3"]),
            ]
            .concat())?,
            Sig::Square => sox(&[
                s(&["-n", "-r", &r, "-c", &c, "-b", &b]),
                vec![p(&out)],
                s(&["synth", &d, "square", "441", "gain", "-0.01"]),
            ]
            .concat())?,
            Sig::Silence => {
                sox(&[s(&["-n", "-r", &r, "-c", &c, "-b", &b]), vec![p(&out)], s(&["trim", "0", &d])]
                    .concat())?;
            }
            Sig::Impulse | Sig::Music => {
                let expr = if matches!(sig, Sig::Impulse) {
                    format!("if(lt(mod(n\\,{})\\,1)\\,0.9\\,0)", rate / 10)
                } else {
                    "0.3*sin(2*PI*220*t+3*sin(2*PI*5*t))*(0.6+0.4*sin(2*PI*0.7*t))+0.15*sin(2*PI*660*t)*exp(-mod(t\\,0.5)*6)+0.02*(random(0)-0.5)".into()
                };
                let fmt = match bits {
                    24 => "pcm_s24le",
                    8 => "pcm_u8",
                    _ => "pcm_s16le",
                };
                ffmpeg(
                    &[
                        s(&["-f", "lavfi", "-i"]),
                        vec![format!("aevalsrc=exprs={expr}:s={rate}:d={secs}")],
                        s(&["-ac", &c, "-c:a", fmt]),
                        vec![p(&out)],
                    ]
                    .concat(),
                )?;
            }
        }
        Ok(out)
    }

    /// Decode `file` with ffmpeg into the reference and add a manifest line.
    fn add(&mut self, rel: &str, r: Ref, gapless_src: Option<&Path>) -> Result<()> {
        let file = self.dir.join(rel);
        let ref_rel = format!("ref/{}.{}", rel.replace('/', "_"), if r == Ref::S32 { "s32" } else { "s16" });
        let rf = self.path(&ref_rel)?;
        let (f, c) = if r == Ref::S32 { ("s32le", "pcm_s32le") } else { ("s16le", "pcm_s16le") };
        ffmpeg(
            &[s(&["-i"]), vec![p(&file)], s(&["-map", "0:a:0", "-f", f, "-c:a", c]), vec![p(&rf)]].concat(),
        )?;
        let frames = match gapless_src {
            Some(src) => frames_of(src)?.to_string(),
            None => "-".into(),
        };
        let fmt = if r == Ref::S32 { "s32" } else { "s16" };
        writeln!(self.manifest, "{rel}\t{ref_rel}\t{fmt}\tok\t{frames}")?;
        self.files += 1;
        Ok(())
    }

    /// A file the decoders must refuse.
    fn refuse(&mut self, rel: &str) -> Result<()> {
        writeln!(self.manifest, "{rel}\t-\t-\tunsupported\t-")?;
        self.files += 1;
        Ok(())
    }

    /// `ffmpeg -i src <args> out` and register it.
    fn ff(&mut self, src: &Path, rel: &str, args: &[&str], r: Ref, gapless: bool) -> Result<()> {
        let out = self.path(rel)?;
        ffmpeg(&[s(&["-i"]), vec![p(src)], s(args), vec![p(&out)]].concat())?;
        self.add(rel, r, gapless.then_some(src))
    }

    /// `flac <args> -o out src` and register it.
    fn flac(&mut self, src: &Path, rel: &str, args: &[&str], r: Ref) -> Result<()> {
        let out = self.path(rel)?;
        tool("flac", &[s(&["-s", "-f"]), s(args), s(&["-o"]), vec![p(&out), p(src)]].concat())?;
        self.add(rel, r, None)
    }
}

fn generate(dir: &Path) -> Result<usize> {
    for t in ["ffmpeg", "sox", "soxi", "flac"] {
        if Command::new(t).arg("--help").output().is_err() {
            bail!("`{t}` is needed to generate the codec corpus");
        }
    }
    if dir.exists() {
        std::fs::remove_dir_all(dir).with_context(|| format!("remove {}", dir.display()))?;
    }
    std::fs::create_dir_all(dir)?;
    let mut g = Gen { dir: dir.to_path_buf(), manifest: String::new(), files: 0 };
    g.manifest.push_str("# file\treference\tformat\texpect\tsource frames (gapless check)\n");
    wav_family(&mut g)?;
    mp3(&mut g)?;
    flac(&mut g)?;
    std::fs::write(dir.join("manifest.tsv"), &g.manifest)?;
    Ok(g.files)
}

fn wav_family(g: &mut Gen) -> Result<()> {
    use Ref::{S16, S32};
    // 16-bit PCM at every native rate, mono and stereo.
    for &rate in &NATIVE {
        for ch in [1, 2] {
            let src = g.source(Sig::Mix, rate, ch, 1.5, 16)?;
            g.ff(&src, &format!("wav/pcm16_{rate}_{ch}.wav"), &["-c:a", "pcm_s16le"], S16, false)?;
        }
    }
    let mix = g.source(Sig::Mix, 44100, 2, 1.5, 16)?;
    let mix24 = g.source(Sig::Mix, 48000, 2, 1.5, 24)?;
    let music = g.source(Sig::Music, 44100, 2, 2.0, 16)?;
    let mono8k = g.source(Sig::Mix, 8000, 1, 2.0, 16)?;
    let mono22 = g.source(Sig::Sweep, 22050, 1, 2.0, 16)?;
    // High rates: decoded as is; the rate plan decimates them.
    for (rate, bits) in [(88200, 24), (96000, 24), (176_400, 16), (192_000, 24)] {
        let src = g.source(Sig::Sweep, rate, 2, 1.0, bits)?;
        let (c, r) = if bits == 24 { ("pcm_s24le", S32) } else { ("pcm_s16le", S16) };
        g.ff(&src, &format!("wav/hi_{rate}_{bits}.wav"), &["-c:a", c], r, false)?;
    }
    let wav: &[(&str, &Path, &[&str], Ref)] = &[
        ("wav/u8_22050_1.wav", &mono22, &["-c:a", "pcm_u8"], S16),
        ("wav/s24_48000_2.wav", &mix24, &["-c:a", "pcm_s24le"], S32),
        ("wav/s32_48000_2.wav", &mix24, &["-c:a", "pcm_s32le"], S32),
        ("wav/f32_44100_2.wav", &music, &["-c:a", "pcm_f32le"], S32),
        ("wav/f64_44100_2.wav", &music, &["-c:a", "pcm_f64le"], S32),
        ("wav/alaw_8000_1.wav", &mono8k, &["-c:a", "pcm_alaw"], S16),
        ("wav/mulaw_8000_1.wav", &mono8k, &["-c:a", "pcm_mulaw"], S16),
        ("wav/ima_44100_2.wav", &music, &["-c:a", "adpcm_ima_wav"], S16),
        ("wav/ima_22050_1.wav", &mono22, &["-c:a", "adpcm_ima_wav"], S16),
        ("wav/ms_44100_2.wav", &music, &["-c:a", "adpcm_ms"], S16),
        ("wav/ms_8000_1.wav", &mono8k, &["-c:a", "adpcm_ms"], S16),
        ("wav/ms_44100_2_bs512.wav", &music, &["-c:a", "adpcm_ms", "-block_size", "512"], S16),
        (
            "wav/tagged_44100_2.wav",
            &mix,
            &["-c:a", "pcm_s16le", "-metadata", "title=Tagged", "-metadata", "artist=Yoton"],
            S16,
        ),
        (
            "wav/bext_44100_2.wav",
            &mix,
            &["-c:a", "pcm_s16le", "-write_bext", "1", "-metadata", "description=bext chunk before the data"],
            S16,
        ),
        ("aiff/s16_44100_2.aiff", &mix, &["-c:a", "pcm_s16be"], S16),
        ("aiff/s24_48000_2.aiff", &mix24, &["-c:a", "pcm_s24be"], S32),
        ("aiff/s32_48000_2.aiff", &mix24, &["-c:a", "pcm_s32be"], S32),
        ("aiff/s8_22050_1.aiff", &mono22, &["-c:a", "pcm_s8"], S16),
        ("aiff/sowt_44100_2.aifc", &mix, &["-c:a", "pcm_s16le", "-f", "aiff"], S16),
        ("aiff/fl32_44100_2.aifc", &music, &["-c:a", "pcm_f32be", "-f", "aiff"], S32),
        ("aiff/fl64_44100_2.aifc", &music, &["-c:a", "pcm_f64be", "-f", "aiff"], S32),
        ("aiff/alaw_8000_1.aifc", &mono8k, &["-c:a", "pcm_alaw", "-f", "aiff"], S16),
        ("aiff/ulaw_8000_1.aifc", &mono8k, &["-c:a", "pcm_mulaw", "-f", "aiff"], S16),
        ("aiff/ima4_44100_2.aifc", &music, &["-c:a", "adpcm_ima_qt", "-f", "aiff"], S16),
        ("aiff/ima4_22050_1.aifc", &mono22, &["-c:a", "adpcm_ima_qt", "-f", "aiff"], S16),
        ("au/s16_44100_2.au", &mix, &["-c:a", "pcm_s16be"], S16),
        ("au/s24_48000_2.au", &mix24, &["-c:a", "pcm_s24be"], S32),
        ("au/s32_48000_2.au", &mix24, &["-c:a", "pcm_s32be"], S32),
        ("au/s8_22050_1.au", &mono22, &["-c:a", "pcm_s8"], S16),
        ("au/f32_44100_2.au", &music, &["-c:a", "pcm_f32be"], S32),
        ("au/f64_44100_2.au", &music, &["-c:a", "pcm_f64be"], S32),
        ("au/mulaw_8000_1.au", &mono8k, &["-c:a", "pcm_mulaw"], S16),
        ("au/alaw_8000_1.au", &mono8k, &["-c:a", "pcm_alaw"], S16),
    ];
    for &(rel, src, args, r) in wav {
        g.ff(src, rel, args, r, false)?;
    }
    // Six channels, other signals.
    let six = g.path("src/six_48000.wav")?;
    ffmpeg(
        &[
            s(&["-f", "lavfi", "-i", "sine=frequency=300:sample_rate=48000:duration=1"]),
            s(&["-filter_complex", "[0:a]asplit=6[a][b][c][d][e][f];[a][b][c][d][e][f]amerge=inputs=6"]),
            s(&["-c:a", "pcm_s16le"]),
            vec![p(&six)],
        ]
        .concat(),
    )?;
    g.ff(&six, "wav/pcm16_48000_6.wav", &["-c:a", "pcm_s16le"], S16, false)?;
    for sig in [Sig::Impulse, Sig::Silence, Sig::Noise, Sig::Square] {
        let src = g.source(sig, 44100, 2, 1.0, 16)?;
        g.ff(&src, &format!("wav/{}_44100_2.wav", sig.name()), &["-c:a", "pcm_s16le"], S16, false)?;
    }
    Ok(())
}

fn mp3(g: &mut Gen) -> Result<()> {
    use Ref::S16;
    let music = g.source(Sig::Music, 44100, 2, 4.0, 16)?;
    let mix = g.source(Sig::Mix, 44100, 2, 4.0, 16)?;
    let noise = g.source(Sig::Noise, 44100, 2, 3.0, 16)?;
    let lame = ["-c:a", "libmp3lame"];
    let with = |extra: &[&'static str]| -> Vec<&'static str> { [&lame[..], extra].concat() };
    for br in ["32k", "64k", "128k", "192k", "320k"] {
        g.ff(&music, &format!("mp3/cbr{br}_44100_2.mp3"), &with(&["-b:a", br]), S16, true)?;
    }
    for q in ["0", "2", "4", "6", "9"] {
        g.ff(&mix, &format!("mp3/vbr_q{q}_44100_2.mp3"), &with(&["-q:a", q]), S16, true)?;
    }
    g.ff(&music, "mp3/abr128_44100_2.mp3", &with(&["-abr", "1", "-b:a", "128k"]), S16, true)?;
    g.ff(&music, "mp3/lr_stereo_128k.mp3", &with(&["-b:a", "128k", "-joint_stereo", "0"]), S16, true)?;
    g.ff(&noise, "mp3/noise_128k.mp3", &with(&["-b:a", "128k"]), S16, true)?;
    g.ff(&music, "mp3/noxing_128k.mp3", &with(&["-b:a", "128k", "-write_xing", "0"]), S16, false)?;
    g.ff(
        &music,
        "mp3/id3v1_128k.mp3",
        &with(&["-b:a", "128k", "-write_id3v1", "1", "-metadata", "title=T1"]),
        S16,
        true,
    )?;
    // Other rates: MPEG-1 32/48 kHz, MPEG-2 16/22.05/24 kHz, MPEG-2.5 8/11.025/12 kHz.
    for (rate, ch, br) in [
        (48000, 2, "320k"),
        (32000, 2, "128k"),
        (24000, 2, "96k"),
        (22050, 2, "64k"),
        (16000, 1, "32k"),
        (12000, 2, "32k"),
        (11025, 1, "16k"),
        (8000, 1, "8k"),
    ] {
        let src = g.source(Sig::Music, rate, ch, 3.0, 16)?;
        g.ff(&src, &format!("mp3/cbr{br}_{rate}_{ch}.mp3"), &with(&["-b:a", br]), S16, true)?;
    }
    let src = g.source(Sig::Sweep, 22050, 1, 3.0, 16)?;
    g.ff(&src, "mp3/vbr_q5_22050_1.mp3", &with(&["-q:a", "5"]), S16, true)?;
    for sig in [Sig::Silence, Sig::Impulse, Sig::Square, Sig::Sweep] {
        let src = g.source(sig, 44100, 2, 2.0, 16)?;
        g.ff(&src, &format!("mp3/{}_192k.mp3", sig.name()), &with(&["-b:a", "192k"]), S16, true)?;
    }
    // ID3v2 with a large picture (more than the stream ring) in front.
    let cover = g.path("src/cover.png")?;
    ffmpeg(
        &[
            s(&["-f", "lavfi", "-i", "nullsrc=s=400x400,geq=random(1)*255:random(2)*255:random(3)*255"]),
            s(&["-frames:v", "1"]),
            vec![p(&cover)],
        ]
        .concat(),
    )?;
    let out = g.path("mp3/apic_128k.mp3")?;
    ffmpeg(
        &[
            s(&["-i"]),
            vec![p(&music)],
            s(&["-i"]),
            vec![p(&cover)],
            s(&["-map", "0:a", "-map", "1:v", "-c:a", "libmp3lame", "-b:a", "128k", "-c:v", "copy"]),
            s(&[
                "-id3v2_version",
                "3",
                "-metadata",
                "title=With picture",
                "-metadata:s:v",
                "comment=Cover (front)",
            ]),
            vec![p(&out)],
        ]
        .concat(),
    )?;
    g.add("mp3/apic_128k.mp3", S16, Some(&music))?;
    // Layer II.
    let src = g.source(Sig::Music, 44100, 2, 3.0, 16)?;
    let out = g.path("mp3/layer2_192k.mp2")?;
    ffmpeg(&[s(&["-i"]), vec![p(&src)], s(&["-c:a", "mp2", "-b:a", "192k"]), vec![p(&out)]].concat())?;
    g.refuse("mp3/layer2_192k.mp2")?;
    Ok(())
}

fn flac(g: &mut Gen) -> Result<()> {
    use Ref::{S16, S32};
    let music = g.source(Sig::Music, 44100, 2, 3.0, 16)?;
    let mix = g.source(Sig::Mix, 44100, 2, 3.0, 16)?;
    for level in 0..=8 {
        g.flac(&music, &format!("flac/l{level}_44100_2.flac"), &[&format!("-{level}")], S16)?;
    }
    let mono = g.source(Sig::Mix, 22050, 1, 3.0, 16)?;
    g.flac(&mono, "flac/l5_22050_1.flac", &["-5"], S16)?;
    for bs in ["192", "576", "1152", "2304", "4096", "4608"] {
        g.flac(&mix, &format!("flac/bs{bs}_44100_2.flac"), &["-5", "-b", bs], S16)?;
    }
    g.flac(&mix, "flac/bs8192_44100_2.flac", &["-5", "--lax", "-b", "8192"], S16)?;
    let big = g.path("flac/bs16384_44100_2.flac")?;
    tool("flac", &[s(&["-s", "-f", "-5", "--lax", "-b", "16384", "-o"]), vec![p(&big), p(&mix)]].concat())?;
    g.refuse("flac/bs16384_44100_2.flac")?;
    g.flac(&music, "flac/noseek_44100_2.flac", &["-5", "--no-seektable"], S16)?;
    g.flac(&music, "flac/nopad_44100_2.flac", &["-8", "--no-padding"], S16)?;
    let cover = g.dir.join("src/cover.png");
    g.flac(
        &music,
        "flac/tagged_44100_2.flac",
        &["-5", "-T", "TITLE=Tagged", "-T", "ARTIST=Yoton", &format!("--picture={}", p(&cover))],
        S16,
    )?;
    let s24 = g.source(Sig::Mix, 96000, 2, 2.0, 24)?;
    g.flac(&s24, "flac/s24_96000_2.flac", &["-5"], S32)?;
    let s24b = g.source(Sig::Music, 48000, 2, 2.0, 24)?;
    g.flac(&s24b, "flac/s24_48000_2_l8.flac", &["-8"], S32)?;
    let s8 = g.source(Sig::Sweep, 22050, 1, 2.0, 8)?;
    g.flac(&s8, "flac/s8_22050_1.flac", &["-5"], S16)?;
    for sig in [Sig::Silence, Sig::Impulse, Sig::Noise, Sig::Square] {
        let src = g.source(sig, 44100, 2, 1.0, 16)?;
        g.flac(&src, &format!("flac/{}_44100_2.flac", sig.name()), &["-5"], S16)?;
    }
    let six = g.dir.join("src/six_48000.wav");
    // Six channels need ~100 KB of planar block: over the 38 KiB budget,
    // refused (no in-decoder down-mix yet).
    let out = g.path("flac/six_48000.flac")?;
    tool("flac", &[s(&["-s", "-f", "-5", "--channel-map=none", "-o"]), vec![p(&out), p(&six)]].concat())?;
    g.refuse("flac/six_48000.flac")?;
    // Odd bit depths (12 and 20 bits) from WAVE_FORMAT_EXTENSIBLE input
    // with fewer valid bits than the container.
    for (bits, cont) in [(12u16, 16u16), (20, 24)] {
        let src = g.path(&format!("src/valid{bits}.wav"))?;
        write_valid_bits_wav(&src, 44100, 2, cont, bits, 2 * 44100)?;
        g.flac(&src, &format!("flac/s{bits}_44100_2.flac"), &["-5"], if bits > 16 { S32 } else { S16 })?;
    }
    // ffmpeg's encoder: high LPC orders, fixed predictors, every stereo mode.
    let ffl: &[(&str, &[&str])] = &[
        ("flac/ff_l12.flac", &["-c:a", "flac", "-compression_level", "12"]),
        ("flac/ff_fixed.flac", &["-c:a", "flac", "-lpc_type", "fixed"]),
        ("flac/ff_lr.flac", &["-c:a", "flac", "-ch_mode", "indep"]),
        ("flac/ff_ls.flac", &["-c:a", "flac", "-ch_mode", "left_side"]),
        ("flac/ff_rs.flac", &["-c:a", "flac", "-ch_mode", "right_side"]),
        ("flac/ff_ms.flac", &["-c:a", "flac", "-ch_mode", "mid_side"]),
        (
            "flac/ff_exact_rice.flac",
            &["-c:a", "flac", "-exact_rice_parameters", "1", "-multi_dim_quant", "1"],
        ),
        ("flac/ff_fs512.flac", &["-c:a", "flac", "-frame_size", "512"]),
    ];
    for &(rel, args) in ffl {
        g.ff(&mix, rel, args, S16, false)?;
    }
    // Refused for now: 32-bit samples, Ogg FLAC (no Ogg layer yet).
    let s32raw = g.path("src/raw32.raw")?;
    let src = g.source(Sig::Mix, 44100, 2, 1.0, 16)?;
    sox(&[vec![p(&src)], s(&["-t", "raw", "-e", "signed", "-b", "32"]), vec![p(&s32raw)]].concat())?;
    let out = g.path("flac/s32_44100_2.flac")?;
    tool(
        "flac",
        &[
            s(&["-s", "-f", "-5", "--force-raw-format", "--endian=little", "--sign=signed", "--channels=2"]),
            s(&["--bps=32", "--sample-rate=44100", "-o"]),
            vec![p(&out), p(&s32raw)],
        ]
        .concat(),
    )?;
    g.refuse("flac/s32_44100_2.flac")?;
    let out = g.path("flac/ogg_44100_2.oga")?;
    tool("flac", &[s(&["-s", "-f", "-5", "--ogg", "-o"]), vec![p(&out), p(&music)]].concat())?;
    g.refuse("flac/ogg_44100_2.oga")?;
    Ok(())
}

/// A stereo music-like test signal quantised to `valid` bits, stored
/// left-justified in `container`-bit samples of a WAVE_FORMAT_EXTENSIBLE
/// file (no tool here writes such files).
fn write_valid_bits_wav(
    path: &Path,
    rate: u32,
    ch: u16,
    container: u16,
    valid: u16,
    frames: u32,
) -> Result<()> {
    let bytes = container / 8;
    let mut data = Vec::with_capacity(frames as usize * usize::from(ch * bytes));
    let full = f64::from(1u32 << (valid - 1)) - 1.0;
    let mut noise = 0x1234_5678u32;
    for n in 0..frames {
        let t = f64::from(n) / f64::from(rate);
        for c in 0..ch {
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            let x = 0.5 * (2.0 * std::f64::consts::PI * (220.0 + 110.0 * f64::from(c)) * t).sin()
                + 0.2 * (2.0 * std::f64::consts::PI * 3150.0 * t).sin()
                + 0.01 * (f64::from(noise) / f64::from(u32::MAX) - 0.5);
            let q = (x * full).round() as i32;
            let v = q << (container - valid);
            data.extend_from_slice(&v.to_le_bytes()[..usize::from(bytes)]);
        }
    }
    let mut h = Vec::new();
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(4 + 8 + 40 + 8 + data.len() as u32).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&40u32.to_le_bytes());
    h.extend_from_slice(&0xFFFEu16.to_le_bytes());
    h.extend_from_slice(&ch.to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&(rate * u32::from(ch * bytes)).to_le_bytes());
    h.extend_from_slice(&(ch * bytes).to_le_bytes());
    h.extend_from_slice(&container.to_le_bytes());
    h.extend_from_slice(&22u16.to_le_bytes());
    h.extend_from_slice(&valid.to_le_bytes());
    h.extend_from_slice(&3u32.to_le_bytes());
    // KSDATAFORMAT_SUBTYPE_PCM
    h.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71]);
    h.extend_from_slice(b"data");
    h.extend_from_slice(&(data.len() as u32).to_le_bytes());
    h.extend_from_slice(&data);
    std::fs::write(path, h)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Fuzzing (07 §3.4)

fn fuzz_dir() -> PathBuf {
    root().join("crates").join("codecs").join("conformance").join("fuzz")
}

/// cargo-fuzz target names (`fuzz/fuzz_targets/<name>.rs`).
fn fuzz_targets() -> Result<Vec<String>> {
    let mut v: Vec<String> = std::fs::read_dir(fuzz_dir().join("fuzz_targets"))?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".rs")).map(str::to_string))
        .collect();
    v.sort();
    Ok(v)
}

/// A nightly toolchain new enough for the workspace's `rust-version`
/// (`YOTON_FUZZ_TOOLCHAIN`, else the newest installed `nightly*`) that has
/// cargo-fuzz.
fn fuzz_toolchain() -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(t) = std::env::var("YOTON_FUZZ_TOOLCHAIN") {
        candidates.push(t);
    }
    let list = Command::new("rustup").args(["toolchain", "list"]).output().ok()?;
    let mut nightlies: Vec<String> = String::from_utf8_lossy(&list.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|n| n.starts_with("nightly"))
        .map(str::to_string)
        .collect();
    nightlies.sort();
    candidates.extend(nightlies.into_iter().rev());
    candidates.into_iter().find(|tc| {
        let minor = Command::new("rustc")
            .arg(format!("+{tc}"))
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| {
                let v = String::from_utf8_lossy(&o.stdout).into_owned();
                v.split_whitespace()
                    .nth(1)
                    .and_then(|s| s.split('.').nth(1).and_then(|m| m.parse::<u32>().ok()))
            })
            .unwrap_or(0);
        minor >= 98
            && Command::new("cargo")
                .arg(format!("+{tc}"))
                .args(["fuzz", "--version"])
                .output()
                .is_ok_and(|o| o.status.success())
    })
}

/// Seed an empty libFuzzer corpus with short copies of the conformance
/// files the target understands (with the 3 control bytes the target bodies
/// expect in front).
fn seed_fuzz_corpus(target: &str, corpus_dir: &Path, out: &Path) -> Result<()> {
    if std::fs::read_dir(out)?.next().is_some() {
        return Ok(());
    }
    let wanted: &[&str] = match target {
        "wav" => &["wav/", "aiff/", "au/"],
        "mp3" => &["mp3/"],
        "flac" => &["flac/"],
        "meta" => &["mp3/", "flac/"],
        _ => &[""],
    };
    let manifest = std::fs::read_to_string(corpus_dir.join("manifest.tsv"))?;
    for (i, rel) in manifest
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split('\t').next())
        .filter(|rel| wanted.iter().any(|w| rel.starts_with(w)))
        .enumerate()
    {
        let mut data = vec![0u8, 0, 0];
        let mut f = std::fs::read(corpus_dir.join(rel))?;
        f.truncate(16 * 1024);
        data.extend_from_slice(&f);
        std::fs::write(out.join(format!("seed{i}")), data)?;
    }
    Ok(())
}

fn fuzz_smoke(secs: u64) -> Result<String> {
    let dir = corpus_dir();
    if !dir.join("manifest.tsv").exists() {
        generate(&dir)?;
    }
    // 1. Stable smoke: every target body over mutated corpus files.
    run(conformance().arg("fuzz-smoke").arg(&dir).args(["--iters", "4000"]))?;
    // 2. libFuzzer, if a recent enough nightly and cargo-fuzz are installed.
    let Some(tc) = fuzz_toolchain() else {
        return Ok(
            "fuzz smoke (stable) passed; cargo-fuzz not run (no nightly >= 1.98 with cargo-fuzz)".into()
        );
    };
    let targets = fuzz_targets()?;
    for t in &targets {
        let regress = root().join("tests").join("regress").join(t);
        let corpus = root().join("target").join("fuzz-corpus").join(t);
        std::fs::create_dir_all(&corpus)?;
        seed_fuzz_corpus(t, &dir, &corpus)?;
        let mut c = Command::new("cargo");
        c.current_dir(fuzz_dir())
            .arg(format!("+{tc}"))
            .args(["fuzz", "run", "--fuzz-dir", "."])
            .arg(t)
            .arg(&corpus);
        if regress.is_dir() {
            c.arg(&regress);
        }
        c.arg("--").arg(format!("-max_total_time={secs}")).arg("-rss_limit_mb=2048").arg("-max_len=262144");
        run(&mut c).with_context(|| {
            format!("fuzz target {t} (crash inputs are in {}/artifacts)", fuzz_dir().display())
        })?;
    }
    Ok(format!("fuzz smoke passed (stable smoke + {} cargo-fuzz targets x {secs} s)", targets.len()))
}

// ---------------------------------------------------------------------------
// Code size on the device target

/// Codec crates measured by `codec-size`.
const SIZE_CRATES: [&str; 4] = ["yoton-audio", "yoton-codec-wav", "yoton-codec-mp3", "yoton-codec-flac"];

fn codec_size() -> Result<String> {
    let target_dir = root().join("target").join("codec-size");
    let mut report = String::new();
    for krate in SIZE_CRATES {
        if !root().join("Cargo.lock").exists() {
            bail!("no Cargo.lock");
        }
        let mut c = cargo();
        c.current_dir(root())
            .args([
                "build",
                "--quiet",
                "--release",
                "--target",
                "thumbv7em-none-eabihf",
                "-p",
                krate,
                "--target-dir",
            ])
            .arg(&target_dir)
            .env("CARGO_PROFILE_RELEASE_LTO", "false")
            .env("RUSTFLAGS", "-C target-cpu=cortex-m4");
        if run(&mut c).is_err() {
            writeln!(report, "{krate:<20} (not built)")?;
            continue;
        }
        let lib = target_dir
            .join("thumbv7em-none-eabihf")
            .join("release")
            .join(format!("lib{}.rlib", krate.replace('-', "_")));
        let (text, data) = rlib_size(&lib)?;
        writeln!(report, "{krate:<20} text+rodata {text:>7} B   data+bss {data:>5} B")?;
    }
    log(&report);
    Ok("codec sizes reported (thumbv7em-none-eabihf, opt-level s, per crate, before LTO)".into())
}

/// Sum of the Berkeley `text` (code + read-only data) and `data + bss`
/// columns over the objects of an rlib, via `arm-none-eabi-size` or
/// `llvm-size`.
fn rlib_size(lib: &Path) -> Result<(u64, u64)> {
    let tools = ["arm-none-eabi-size", "llvm-size"];
    for t in tools {
        let Ok(out) = Command::new(t).arg(lib).output() else { continue };
        if !out.status.success() {
            continue;
        }
        let (mut text, mut data) = (0u64, 0u64);
        for line in String::from_utf8_lossy(&out.stdout).lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() >= 3
                && let (Ok(t), Ok(d), Ok(b)) = (f[0].parse::<u64>(), f[1].parse::<u64>(), f[2].parse::<u64>())
            {
                text += t;
                data += d + b;
            }
        }
        return Ok((text, data));
    }
    bail!("neither arm-none-eabi-size nor llvm-size is installed")
}
