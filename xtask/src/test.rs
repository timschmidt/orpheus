//! `xtask test [host|golden|pagelink|codecs|fuzz-smoke|codec-size|all]` (07 §3).
//!
//! * `pagelink`: the paged toy end to end (WP T-3).
//!
//! * `codecs [--gen] [--filter S] [--quick]`: the codec conformance suite
//!   (07 §3.3). `--gen` (re)builds the corpus in `tests/corpus/` with ffmpeg,
//!   sox and flac; it is also built when missing. The checks run in the
//!   `orpheus-codec-conformance` binary with overflow checks on. Then the
//!   RFC 8251 Opus vectors (fetched into `tests/vectors/` when missing) run
//!   as the opus crate's tests. `ORPHEUS_CORPUS` overrides the corpus path.
//! * `fuzz-smoke [--secs N]`: the stable mutation smoke run over the codec
//!   fuzz targets, then every `cargo fuzz` target of every fuzz project
//!   (`crates/**/fuzz/`, `sim/fuzz/`) for N seconds (default 60) when a
//!   nightly toolchain and cargo-fuzz are installed (07 §3.4), starting from
//!   the committed corpus (`tests/fuzz-corpus/<target>/`, with its
//!   dictionary `tests/fuzz-corpus/<target>.dict` if there is one) and the
//!   regression inputs (`tests/regress/<target>/`); new inputs go to
//!   `target/fuzz-corpus/<target>/` (merge them with `cargo fuzz cmin`).
//! * `fuzz-seeds`: rewrite the committed seed inputs (`seed-*`) of every
//!   fuzz target (VER-4): the codec targets from the conformance corpus
//!   (generated when missing; at most 24 files per target, each cut to
//!   16 KiB), the others from their crates' `fuzz_corpus` tests
//!   (`ORPHEUS_WRITE_SEEDS=1`), which encode them with the crates' own
//!   encoders and fixtures.
//! * `codec-size`: build each codec crate for `thumbv7em-none-eabihf`
//!   (release) and report its code and data size.
//! * `asan [--fuzz-only] [--lean]`, `miri [--crate NAME]`: sanitizers (07 §3.5,
//!   VER-5; `sanitize.rs`). Not part of `all`.

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
    let fuzz_only = args.flag("fuzz-only");
    let lean = args.flag("lean");
    let krate = args.value("crate");
    let which = args.next_positional().unwrap_or_else(|| "all".into());
    match which.as_str() {
        "host" => host(),
        "golden" => golden(bless),
        "pagelink" => crate::selftest::run(),
        "codecs" => codecs(gen_corpus, filter.as_deref(), quick),
        "fuzz-smoke" => fuzz_smoke(secs),
        "fuzz-seeds" => fuzz_seeds(),
        "codec-size" => codec_size(),
        "asan" => crate::sanitize::asan(fuzz_only, lean),
        "miri" => crate::sanitize::miri(krate.as_deref()),
        "all" => {
            host()?;
            golden(false)?;
            crate::selftest::run()?;
            let c = codecs(gen_corpus, None, false)?;
            let f = fuzz_smoke(secs)?;
            Ok(format!("host, golden and pagelink tests passed; {c}; {f}"))
        }
        other => bail!(
            "unknown test suite `{other}` (host, golden, pagelink, codecs, fuzz-smoke, fuzz-seeds, codec-size, asan, miri, all)"
        ),
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
    c.current_dir(root()).args(["test", "--quiet", "--package", "orpheus-sim", "--test", "golden"]);
    if bless {
        c.env("ORPHEUS_BLESS", "1");
    }
    run(&mut c)?;
    Ok(if bless { "golden images re-blessed".into() } else { "golden tests passed".into() })
}

// ---------------------------------------------------------------------------
// Codec conformance (07 §3.3)

/// `tests/corpus/`, or `$ORPHEUS_CORPUS` (a scratch corpus while working on
/// one codec's generator).
fn corpus_dir() -> PathBuf {
    std::env::var_os("ORPHEUS_CORPUS").map_or_else(|| root().join("tests").join("corpus"), PathBuf::from)
}

/// The conformance binary, built with overflow checks and debug assertions
/// (tests run with checks on, 02 §9.1) in its own target directory.
fn conformance() -> Command {
    let mut c = cargo();
    c.current_dir(root())
        .args(["run", "--quiet", "--release", "-p", "orpheus-codec-conformance", "--target-dir"])
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
    let vectors = fetch_opus_vectors();
    // The RFC 8251 Opus vectors (range-exact, ≤ −80 dBFS) run as the opus
    // crate's tests; they skip themselves when the vectors are missing.
    run(cargo().current_dir(root()).args(["test", "--quiet", "--release", "-p", "orpheus-codec-opus"]))?;
    Ok(format!("codec conformance passed; {vectors}"))
}

/// The RFC 8251 Opus test vectors (opus-codec.org; code components of the
/// RFC, Simplified BSD licence) in `tests/vectors/opus_newvectors`,
/// downloaded with curl and checked by SHA-256 when missing.
fn fetch_opus_vectors() -> String {
    const URL: &str = "https://opus-codec.org/static/testvectors/opus_testvectors-rfc8251.tar.gz";
    const SHA256: &str = "6b26a22f9ba87b2b836906a9bb7afec5f8e54d49553b1200382520ee6fedfa55";
    let dir = root().join("tests").join("vectors");
    if dir.join("opus_newvectors").join("testvector01.bit").exists() {
        return "Opus RFC 8251 vectors present".into();
    }
    let tgz = dir.join("opus_testvectors-rfc8251.tar.gz");
    let fetched = std::fs::create_dir_all(&dir).is_ok()
        && Command::new("curl").args(["-sSfL", "-o"]).arg(&tgz).arg(URL).status().is_ok_and(|s| s.success())
        && Command::new("sha256sum")
            .arg(&tgz)
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).starts_with(SHA256))
        && Command::new("tar").arg("xzf").arg(&tgz).arg("-C").arg(&dir).status().is_ok_and(|s| s.success());
    let _ = std::fs::remove_file(&tgz);
    if fetched {
        "Opus RFC 8251 vectors downloaded".into()
    } else {
        log("could not fetch the Opus test vectors: their tests are skipped");
        "Opus RFC 8251 vectors NOT available (tests skipped)".into()
    }
}

// ---------------------------------------------------------------------------
// Corpus generator

/// The rates the DAC plays natively (04 §3.2).
const NATIVE: [u32; 9] = [8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000];

struct Gen {
    dir: PathBuf,
    manifest: String,
    /// `gapless.tsv`: album-style splits decoded back to back.
    joins: String,
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
        self.add_dec(rel, r, gapless_src, None)
    }

    /// As [`Self::add`], decoding the reference with the named ffmpeg
    /// decoder (`libopus`, `libspeex`) instead of ffmpeg's default one.
    fn add_dec(
        &mut self,
        rel: &str,
        r: Ref,
        gapless_src: Option<&Path>,
        decoder: Option<&str>,
    ) -> Result<()> {
        let file = self.dir.join(rel);
        let ref_rel = format!("ref/{}.{}", rel.replace('/', "_"), if r == Ref::S32 { "s32" } else { "s16" });
        let rf = self.path(&ref_rel)?;
        let (f, c) = if r == Ref::S32 { ("s32le", "pcm_s32le") } else { ("s16le", "pcm_s16le") };
        let dec: Vec<String> = decoder.map_or_else(Vec::new, |d| s(&["-c:a", d]));
        ffmpeg(
            &[dec, s(&["-i"]), vec![p(&file)], s(&["-map", "0:a:0", "-f", f, "-c:a", c]), vec![p(&rf)]]
                .concat(),
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
    let mut g = Gen { dir: dir.to_path_buf(), manifest: String::new(), joins: String::new(), files: 0 };
    g.manifest.push_str("# file\treference\tformat\texpect\tsource frames (gapless check)\n");
    g.joins.push_str("# name\tsource wav\tparts\texact|lossy\tper-part references\n");
    wav_family(&mut g)?;
    mp3(&mut g)?;
    flac(&mut g)?;
    vorbis(&mut g)?;
    opus(&mut g)?;
    speex(&mut g)?;
    gapless(&mut g)?;
    std::fs::write(dir.join("manifest.tsv"), &g.manifest)?;
    std::fs::write(dir.join("gapless.tsv"), &g.joins)?;
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

/// Ogg Opus (WP AU-8): libopus through ffmpeg in its SILK, hybrid and CELT
/// ranges, every frame duration, CBR/VBR, mono/stereo, and opus-tools'
/// `opusenc` (its own muxer, a picture in OpusTags). References come from
/// libopus (`-c:a libopus`), which applies pre-skip, end trimming and the
/// header gain as RFC 7845 asks.
fn opus(g: &mut Gen) -> Result<()> {
    use Ref::S16;
    let music = g.source(Sig::Music, 48000, 2, 4.0, 16)?;
    let mix = g.source(Sig::Mix, 48000, 2, 3.0, 16)?;
    let mono = g.source(Sig::Music, 48000, 1, 3.0, 16)?;
    let enc = |extra: &[&'static str]| -> Vec<&'static str> { [&["-c:a", "libopus"][..], extra].concat() };
    let add = |g: &mut Gen, src: &Path, rel: &str, args: &[&str], gapless: bool| -> Result<()> {
        let out = g.path(rel)?;
        ffmpeg(&[s(&["-i"]), vec![p(src)], s(args), vec![p(&out)]].concat())?;
        g.add_dec(rel, S16, gapless.then_some(src), Some("libopus"))
    };
    // Bit-rate ladder, default application (the encoder picks SILK, hybrid
    // or CELT by rate).
    for br in ["16k", "24k", "32k", "48k", "64k", "96k", "128k", "192k", "256k"] {
        add(g, &music, &format!("opus/music_{br}.opus"), &enc(&["-b:a", br]), true)?;
    }
    // SILK (voip at low rates), hybrid (voip/audio mid rates), CELT only.
    for (rel, src, args) in [
        ("opus/silk_nb_8k_mono.opus", &mono, &["-application", "voip", "-b:a", "8k", "-cutoff", "4000"][..]),
        (
            "opus/silk_mb_12k_mono.opus",
            &mono,
            &["-application", "voip", "-b:a", "12k", "-cutoff", "6000"][..],
        ),
        (
            "opus/silk_wb_16k_mono.opus",
            &mono,
            &["-application", "voip", "-b:a", "16k", "-cutoff", "8000"][..],
        ),
        (
            "opus/silk_wb_20k_stereo.opus",
            &music,
            &["-application", "voip", "-b:a", "20k", "-cutoff", "8000"][..],
        ),
        (
            "opus/hybrid_swb_24k.opus",
            &music,
            &["-application", "voip", "-b:a", "24k", "-cutoff", "12000"][..],
        ),
        ("opus/hybrid_fb_32k.opus", &music, &["-application", "voip", "-b:a", "32k"][..]),
        ("opus/celt_lowdelay_64k.opus", &mix, &["-application", "lowdelay", "-b:a", "64k"][..]),
        ("opus/celt_cbr_128k.opus", &mix, &["-vbr", "off", "-b:a", "128k"][..]),
        ("opus/celt_cvbr_96k.opus", &mix, &["-vbr", "constrained", "-b:a", "96k"][..]),
        ("opus/mono_48k.opus", &mono, &["-b:a", "48k"][..]),
    ] {
        add(g, src, rel, &enc(args), true)?;
    }
    // Frame durations: 2.5–20 ms single frames, 40/60/120 ms multi-frame
    // packets (SILK 40/60 ms frames at voip rates).
    for fd in ["2.5", "5", "10", "20", "40", "60", "120"] {
        add(
            g,
            &music,
            &format!("opus/celt_fd{fd}_96k.opus"),
            &enc(&["-frame_duration", fd, "-b:a", "96k"]),
            true,
        )?;
    }
    for fd in ["40", "60", "120"] {
        add(
            g,
            &mono,
            &format!("opus/silk_fd{fd}_16k.opus"),
            &enc(&["-application", "voip", "-frame_duration", fd, "-b:a", "16k"]),
            true,
        )?;
    }
    // Other input rates (decoded at 48 kHz: no gapless length check).
    for rate in [8000, 16000, 24000] {
        let src = g.source(Sig::Music, rate, 1, 3.0, 16)?;
        add(g, &src, &format!("opus/in{rate}_mono.opus"), &enc(&["-b:a", "24k"]), false)?;
    }
    for sig in [Sig::Silence, Sig::Impulse, Sig::Square, Sig::Noise, Sig::Sweep] {
        let src = g.source(sig, 48000, 2, 2.0, 16)?;
        add(g, &src, &format!("opus/{}_96k.opus", sig.name()), &enc(&["-b:a", "96k"]), true)?;
    }
    // opus-tools' encoder and muxer, if installed: speech framing and a
    // picture (OpusTags over several pages).
    if Command::new("opusenc").arg("--version").output().is_ok() {
        let cover = g.dir.join("src/cover.png");
        for (rel, args) in [
            ("opus/opusenc_96k.opus", vec!["--quiet", "--bitrate", "96"]),
            (
                "opus/opusenc_speech_24k_60ms.opus",
                vec!["--quiet", "--speech", "--bitrate", "24", "--framesize", "60"],
            ),
            ("opus/opusenc_picture.opus", vec!["--quiet", "--bitrate", "64", "--picture"]),
        ] {
            let out = g.path(rel)?;
            let mut a = s(&args);
            if rel.contains("picture") {
                a.push(p(&cover));
            }
            tool("opusenc", &[a, vec![p(&music), p(&out)]].concat())?;
            g.add_dec(rel, S16, Some(&music), Some("libopus"))?;
        }
    }
    // Header output gain (+6 dB and -6 dB), patched into an ffmpeg file.
    for (rel, gain) in [("opus/gain_plus6db.opus", 1536i16), ("opus/gain_minus6db.opus", -1536)] {
        let base = g.dir.join("opus/music_64k.opus");
        let mut f = std::fs::read(&base)?;
        set_opus_gain(&mut f, gain)?;
        std::fs::write(g.path(rel)?, f)?;
        g.add_dec(rel, S16, Some(&music), Some("libopus"))?;
    }
    // Surround (multistream, mapping family 1): refused.
    let six = g.dir.join("src/six_48000.wav");
    let out = g.path("opus/six_48000.opus")?;
    ffmpeg(
        &[
            s(&["-i"]),
            vec![p(&six)],
            s(&["-c:a", "libopus", "-b:a", "192k", "-mapping_family", "1"]),
            vec![p(&out)],
        ]
        .concat(),
    )?;
    g.refuse("opus/six_48000.opus")?;
    Ok(())
}

/// Set the output gain in the `OpusHead` of an Ogg Opus file (first page)
/// and fix that page's CRC.
fn set_opus_gain(f: &mut [u8], gain: i16) -> Result<()> {
    if f.len() < 28 || &f[..4] != b"OggS" {
        bail!("not an Ogg file");
    }
    let nseg = usize::from(f[26]);
    let hl = 27 + nseg;
    let body: usize = f[27..hl].iter().map(|&b| usize::from(b)).sum();
    if f.len() < hl + body || &f[hl..hl + 8] != b"OpusHead" {
        bail!("no OpusHead");
    }
    f[hl + 16..hl + 18].copy_from_slice(&gain.to_le_bytes());
    f[22..26].fill(0);
    let mut crc = 0u32;
    for &b in &f[..hl + body] {
        crc ^= u32::from(b) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04C1_1DB7 } else { crc << 1 };
        }
    }
    f[22..26].copy_from_slice(&crc.to_le_bytes());
    Ok(())
}

/// Ogg Speex from ffmpeg's `libspeex` encoder: narrowband (8 kHz),
/// wideband (16 kHz) and ultra-wideband (32 kHz), CBR qualities that select
/// every sub-mode, VBR, ABR, VAD/DTX, several frames per packet, mono and
/// intensity stereo. References come from libspeex (`-c:a libspeex`),
/// which outputs every decoded frame untrimmed; the sources are whole
/// numbers of frames long (3 s), so the decoded length equals the source
/// length.
fn speex(g: &mut Gen) -> Result<()> {
    use Ref::S16;
    let add = |g: &mut Gen, src: &Path, rel: &str, args: &[&str]| -> Result<()> {
        let out = g.path(rel)?;
        ffmpeg(&[s(&["-i"]), vec![p(src)], s(&["-c:a", "libspeex"]), s(args), vec![p(&out)]].concat())?;
        g.add_dec(rel, S16, Some(src), Some("libspeex"))
    };
    let nb = g.source(Sig::Mix, 8000, 1, 3.0, 16)?;
    let wb = g.source(Sig::Mix, 16000, 1, 3.0, 16)?;
    let uwb = g.source(Sig::Mix, 32000, 1, 3.0, 16)?;
    let nb2 = g.source(Sig::Music, 8000, 2, 3.0, 16)?;
    let wb2 = g.source(Sig::Music, 16000, 2, 3.0, 16)?;
    let uwb2 = g.source(Sig::Music, 32000, 2, 3.0, 16)?;
    // CBR: quality 0–10 maps to narrowband sub-modes 1, 8, 2, 3, 4, 5, 6, 7.
    for q in ["0", "1", "2", "4", "6", "8", "9", "10"] {
        add(g, &nb, &format!("speex/nb_q{q}.spx"), &["-cbr_quality", q])?;
    }
    for q in ["0", "3", "6", "8", "10"] {
        add(g, &wb, &format!("speex/wb_q{q}.spx"), &["-cbr_quality", q])?;
    }
    for q in ["2", "8", "10"] {
        add(g, &uwb, &format!("speex/uwb_q{q}.spx"), &["-cbr_quality", q])?;
    }
    let other: &[(&str, &Path, &[&str])] = &[
        ("speex/nb_vbr_q4.spx", &nb, &["-q:a", "4"]),
        ("speex/nb_abr_11k.spx", &nb, &["-abr", "1", "-b:a", "11k"]),
        ("speex/nb_vad_dtx.spx", &nb, &["-q:a", "3", "-vad", "1", "-dtx", "1"]),
        ("speex/nb_fpp4_c10.spx", &nb, &["-frames_per_packet", "4", "-compression_level", "10"]),
        ("speex/wb_vbr_q6.spx", &wb, &["-q:a", "6"]),
        ("speex/wb_abr_20k.spx", &wb, &["-abr", "1", "-b:a", "20k"]),
        ("speex/wb_fpp3_c1.spx", &wb, &["-frames_per_packet", "3", "-compression_level", "1"]),
        ("speex/wb_dtx.spx", &wb, &["-q:a", "2", "-vad", "1", "-dtx", "1"]),
        ("speex/uwb_vbr_q7.spx", &uwb, &["-q:a", "7"]),
        ("speex/uwb_fpp8.spx", &uwb, &["-frames_per_packet", "8"]),
        ("speex/nb_stereo_q8.spx", &nb2, &["-cbr_quality", "8"]),
        ("speex/wb_stereo_vbr.spx", &wb2, &["-q:a", "5"]),
        ("speex/uwb_stereo_abr_fpp2.spx", &uwb2, &["-abr", "1", "-b:a", "32k", "-frames_per_packet", "2"]),
    ];
    for &(rel, src, args) in other {
        add(g, src, rel, args)?;
    }
    let silence = g.source(Sig::Silence, 8000, 1, 2.0, 16)?;
    add(g, &silence, "speex/nb_silence.spx", &[])?;
    let impulse = g.source(Sig::Impulse, 16000, 1, 2.0, 16)?;
    add(g, &impulse, "speex/wb_impulse.spx", &[])?;
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

fn vorbis(g: &mut Gen) -> Result<()> {
    use Ref::S16;
    let music = g.source(Sig::Music, 44100, 2, 4.0, 16)?;
    let mix = g.source(Sig::Mix, 44100, 2, 4.0, 16)?;
    let lib = ["-c:a", "libvorbis"];
    let with = |extra: &[&'static str]| -> Vec<&'static str> { [&lib[..], extra].concat() };
    for q in ["0", "2", "5", "8", "10"] {
        g.ff(&music, &format!("vorbis/q{q}_44100_2.ogg"), &with(&["-q:a", q]), S16, true)?;
    }
    g.ff(&mix, "vorbis/q4_mix_44100_2.ogg", &with(&["-q:a", "4"]), S16, true)?;
    g.ff(&mix, "vorbis/b64k_44100_2.ogg", &with(&["-b:a", "64k"]), S16, true)?;
    for (rate, ch, q) in [
        (48000, 2, "6"),
        (32000, 2, "3"),
        (22050, 1, "3"),
        (16000, 1, "2"),
        (11025, 1, "1"),
        (8000, 1, "0"),
        (96000, 2, "5"),
    ] {
        let src = g.source(Sig::Music, rate, ch, 3.0, 16)?;
        g.ff(&src, &format!("vorbis/q{q}_{rate}_{ch}.ogg"), &with(&["-q:a", q]), S16, true)?;
    }
    for sig in [Sig::Silence, Sig::Impulse, Sig::Square, Sig::Noise, Sig::Sweep] {
        let src = g.source(sig, 44100, 2, 2.0, 16)?;
        g.ff(&src, &format!("vorbis/{}_q5.ogg", sig.name()), &with(&["-q:a", "5"]), S16, true)?;
    }
    // A comment packet larger than the stream ring before the set-up.
    let big = format!("comment={}", "lyrics ".repeat(6000));
    g.ff(
        &music,
        "vorbis/bigcomment_q3.ogg",
        &["-c:a", "libvorbis", "-q:a", "3", "-metadata", &big],
        S16,
        true,
    )?;
    let six = g.dir.join("src/six_48000.wav");
    // Six channels need ~104 KB: over the 72 KiB arena, refused.
    let out = g.path("vorbis/six_q3_48000.ogg")?;
    ffmpeg(&[s(&["-i"]), vec![p(&six)], s(&["-c:a", "libvorbis", "-q:a", "3"]), vec![p(&out)]].concat())?;
    g.refuse("vorbis/six_q3_48000.ogg")?;
    Ok(())
}

/// Album-style gapless sets (04 §2.2): one source cut at odd sample
/// positions into three tracks, encoded per codec. The conformance run
/// decodes the tracks back to back in one arena (decoder re-opened at each
/// boundary) and requires the join to have exactly the source's length —
/// bit-exact for lossless codecs, within −80 dBFS of ffmpeg's per-track
/// decodes for lossy ones.
fn gapless(g: &mut Gen) -> Result<()> {
    let src = g.source(Sig::Music, 44100, 2, 5.0, 16)?;
    let total = frames_of(&src)?;
    let cuts = [0u64, 60_417, 131_333, total];
    let mut wavs = Vec::new();
    for k in 0..3 {
        let part = g.path(&format!("gapless/src_part{k}.wav"))?;
        sox(&[
            vec![p(&src), p(&part)],
            s(&["trim", &format!("{}s", cuts[k]), &format!("{}s", cuts[k + 1] - cuts[k])]),
        ]
        .concat())?;
        wavs.push(part);
    }
    let codecs: &[(&str, &str, &[&str], bool)] = &[
        ("flac", "flac", &["-c:a", "flac"], true),
        ("wav", "wav", &["-c:a", "pcm_s16le"], true),
        ("mp3", "mp3", &["-c:a", "libmp3lame", "-b:a", "192k"], false),
        ("vorbis", "ogg", &["-c:a", "libvorbis", "-q:a", "5"], false),
    ];
    for &(name, ext, args, exact) in codecs {
        let mut parts = Vec::new();
        let mut refs = Vec::new();
        for (k, w) in wavs.iter().enumerate() {
            let rel = format!("gapless/{name}_part{k}.{ext}");
            let out = g.path(&rel)?;
            ffmpeg(&[s(&["-i"]), vec![p(w)], s(args), vec![p(&out)]].concat())?;
            let rref = format!("gapless/{name}_part{k}.s16");
            ffmpeg(
                &[
                    s(&["-i"]),
                    vec![p(&out)],
                    s(&["-f", "s16le", "-c:a", "pcm_s16le"]),
                    vec![p(&g.dir.join(&rref))],
                ]
                .concat(),
            )?;
            parts.push(rel);
            refs.push(rref);
        }
        let srcrel = src.strip_prefix(&g.dir).unwrap_or(&src).to_string_lossy().into_owned();
        writeln!(
            g.joins,
            "{name}\t{srcrel}\t{}\t{}\t{}",
            parts.join(","),
            if exact { "exact" } else { "lossy" },
            refs.join(",")
        )?;
    }
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
/// (`ORPHEUS_FUZZ_TOOLCHAIN`, else the newest installed `nightly*`) that has
/// cargo-fuzz.
fn fuzz_toolchain() -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(t) = std::env::var("ORPHEUS_FUZZ_TOOLCHAIN") {
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
        "vorbis" | "ogg" => &["vorbis/"],
        "opus" => &["opus/"],
        "speex" => &["speex/"],
        "meta" => &["mp3/", "flac/", "wav/"],
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
    // 1. Stable smoke: every codec target body over mutated corpus files.
    run(conformance().arg("fuzz-smoke").arg(&dir).args(["--iters", "4000"]))?;
    // 2. libFuzzer, if a recent enough nightly and cargo-fuzz are installed.
    let Some(tc) = fuzz_toolchain() else {
        return Ok(
            "fuzz smoke (stable) passed; cargo-fuzz not run (no nightly >= 1.98 with cargo-fuzz)".into()
        );
    };
    let codec = fuzz_targets()?;
    let mut n = 0;
    for fd in crate::cover::fuzz_dirs() {
        for t in &fd.targets {
            let scratch = root().join("target").join("fuzz-corpus").join(t);
            std::fs::create_dir_all(&scratch)?;
            let committed = root().join("tests").join("fuzz-corpus").join(t);
            if codec.contains(t) && !committed.is_dir() {
                seed_fuzz_corpus(t, &dir, &scratch)?;
            }
            let mut c = Command::new("cargo");
            c.current_dir(&fd.dir)
                .arg(format!("+{tc}"))
                .args(["fuzz", "run", "--fuzz-dir", "."])
                .arg(t)
                .arg(&scratch)
                .args(crate::cover::corpus_dirs(&fd, t));
            c.arg("--")
                .arg(format!("-max_total_time={secs}"))
                .arg("-rss_limit_mb=2048")
                .arg("-max_len=262144");
            let dict = root().join("tests").join("fuzz-corpus").join(format!("{t}.dict"));
            if dict.exists() {
                c.arg(format!("-dict={}", dict.display()));
            }
            if let Some(s) = crate::cover::fs_seeds(&root().join("target")) {
                c.env("ORPHEUS_FS_SEEDS", s);
            }
            run(&mut c).with_context(|| {
                format!("fuzz target {t} (crash inputs are in {}/artifacts)", fd.dir.display())
            })?;
            n += 1;
        }
    }
    Ok(format!("fuzz smoke passed (stable smoke + {n} cargo-fuzz targets x {secs} s)"))
}

/// Codec seeds of one target: at most 24 conformance files spread over the
/// manifest, each cut to 16 KiB, with the control bytes the target body
/// reads first (`orpheus_codec_conformance::fuzz`).
fn codec_seeds(target: &str, corpus: &Path, out: &Path) -> Result<usize> {
    let (wanted, prefix): (&[&str], &[u8]) = match target {
        "wav" => (&["wav/", "aiff/", "au/"], &[0, 0, 0]),
        "mp3" => (&["mp3/"], &[0, 0, 0]),
        "flac" => (&["flac/"], &[0, 0, 0]),
        "vorbis" => (&["vorbis/"], &[0, 0, 0]),
        "opus" => (&["opus/"], &[0, 0, 0]),
        "speex" => (&["speex/"], &[0, 0, 0]),
        "decode_any" => (&[""], &[0, 0, 0]),
        "ogg" => (&["vorbis/", "opus/", "speex/"], &[0]),
        "meta" => (&["mp3/", "flac/", "wav/", "aiff/"], &[0]),
        "probe" => (&[""], &[]),
        _ => return Ok(0),
    };
    let manifest = std::fs::read_to_string(corpus.join("manifest.tsv"))?;
    let files: Vec<&str> = manifest
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split('\t').next())
        .filter(|rel| wanted.iter().any(|w| rel.starts_with(w)))
        .collect();
    let step = files.len().div_ceil(24).max(1);
    let mut n = 0;
    for (i, rel) in files.iter().enumerate().filter(|(i, _)| i % step == 0) {
        let mut data = prefix.to_vec();
        // Vary the refill pattern and the seek of the decode targets.
        if prefix.len() == 3 {
            data[0] = (i % 5) as u8;
            data[1] = if i % 3 == 0 { 0x2B } else { 0 };
        }
        let mut f = std::fs::read(corpus.join(rel))?;
        f.truncate(16 * 1024);
        data.extend_from_slice(&f);
        let name = rel.replace('/', "_");
        std::fs::write(out.join(format!("seed-{name}")), data)?;
        n += 1;
    }
    Ok(n)
}

/// LRC and SYLT-like seeds of the `lyrics` target (no corpus files carry
/// lyrics).
const LYRICS_SEEDS: [(&str, &str); 3] = [
    (
        "lrc",
        "[ar:Artist]\n[ti:Title]\n[offset:+250]\n[00:01.00]First line\n[00:05.50][01:05.50]Repeated\n[00:09.999]\u{442}\u{435}\u{43a}\u{441}\u{442}\n",
    ),
    (
        "lrc-enhanced",
        "[00:12.00]<00:12.00>Word <00:12.50>by <00:13.00>word\r\n[1:02.3]short\r\n[xx:yy]bad tag\n",
    ),
    ("lrc-bom", "\u{feff}[length: 03:15]\n[00:00.00]\n[99:59.99]end\n"),
];

fn fuzz_seeds() -> Result<String> {
    let corpus = corpus_dir();
    if !corpus.join("manifest.tsv").exists() {
        generate(&corpus)?;
    }
    let base = root().join("tests").join("fuzz-corpus");
    let mut codec = 0;
    for t in fuzz_targets()? {
        let out = base.join(&t);
        std::fs::create_dir_all(&out)?;
        codec += codec_seeds(&t, &corpus, &out)?;
        if t == "lyrics" {
            for (name, text) in LYRICS_SEEDS {
                std::fs::write(out.join(format!("seed-{name}")), text)?;
                codec += 1;
            }
        }
    }
    // The other targets' seeds come from their crates' tests.
    let tests: [(&str, &str); 14] = [
        ("orpheus-otp", "fuzz_corpus"),
        ("orpheus-bt", "fuzz_corpus"),
        ("orpheus-usb", "fuzz_corpus"),
        ("orpheus-proto", "fuzz_corpus"),
        ("orpheus-settings", "fuzz_corpus"),
        ("orpheus-index", "fuzz_corpus"),
        ("orpheus-library", "fuzz_corpus"),
        ("orpheus-player", "fuzz_corpus"),
        ("orpheus-sdcard", "fuzz_corpus"),
        ("orpheus-input", "fuzz_corpus"),
        ("orpheus-power", "fuzz_corpus"),
        ("orpheus-img", "fuzz_corpus"),
        ("orpheus-fs", "fuzz_smoke"),
        ("orpheus-sim", "fuzz_corpus"),
    ];
    for (pkg, test) in tests {
        run(cargo()
            .current_dir(root())
            .args(["test", "-p", pkg, "--test", test])
            .env("ORPHEUS_WRITE_SEEDS", "1"))?;
    }
    Ok(format!("fuzz seeds written: {codec} codec seeds, {} crates' seed tests", tests.len()))
}

// ---------------------------------------------------------------------------
// Code size on the device target

/// Codec crates measured by `codec-size`.
const SIZE_CRATES: [&str; 10] = [
    "orpheus-audio",
    "orpheus-codec-wav",
    "orpheus-codec-mp3",
    "orpheus-codec-flac",
    "orpheus-ogg",
    "orpheus-codec-vorbis",
    "orpheus-codec-opus",
    "orpheus-codec-speex",
    "orpheus-meta",
    "orpheus-player",
];

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
