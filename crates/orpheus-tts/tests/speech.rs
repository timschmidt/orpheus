//! The synthesiser (TTS-2): durations against TTS-1's model, block-size
//! independence, vowel formants by LPC, the F0 contours, pitch kept across
//! speeds, no clipping, events, stop, RAM.

use orpheus_tts::phoneme::{ALL, Phone};
use orpheus_tts::resample::frames_for;
use orpheus_tts::speak::ram;
use orpheus_tts::targets;
use orpheus_tts::{
    BreakKind, Config, Contour, Converter, Event, Options, Pause, Ph, Seg, Speaker, Speed, Stop, Stress,
    Synth, Token, Voice, Word,
};

const SENTENCES: [&str; 6] = [
    "The quick brown fox jumps over the lazy dog.",
    "Would you like to resume where you stopped?",
    "Track three of twelve, four minutes and five seconds.",
    "Where did you put the green umbrella? Nobody knows!",
    "Battery at forty percent; about two hours left.",
    "She sells shells, he jumps; we sing... the end.",
];

fn tokens(text: &str, speed: Speed) -> Vec<(Token, Vec<u8>)> {
    let mut v = Vec::new();
    Converter::new(Options::default(), speed)
        .convert(text.as_bytes(), |c, t| v.push((t, c.word_text().to_vec())));
    v
}

fn model_ms(toks: &[(Token, Vec<u8>)]) -> u64 {
    toks.iter()
        .map(|(t, _)| match t {
            Token::Phone(s) => u64::from(s.ms),
            Token::Break(p) => u64::from(p.ms),
            Token::Word(_) => 0,
        })
        .sum()
}

/// Renders `toks` with `block` frames per call, pushing tokens as space
/// allows; returns the samples (interleaved when stereo) and the events
/// with their absolute frame.
fn render(cfg: Config, toks: &[(Token, Vec<u8>)], block: usize) -> (Vec<i16>, Vec<(usize, Event)>) {
    let mut s = Synth::new(cfg);
    let ch = usize::from(cfg.channels);
    let mut out = Vec::new();
    let mut ev = Vec::new();
    let mut buf = vec![0i16; block * ch];
    let mut it = toks.iter();
    let mut finished = false;
    for _ in 0..10_000_000 {
        while !finished && s.space() > 0 {
            match it.next() {
                Some((t, text)) => assert!(s.push_with_text(*t, text)),
                None => {
                    s.finish();
                    finished = true;
                }
            }
        }
        let base = out.len() / ch;
        let n = s.render(&mut buf, |k, e| ev.push((base + k, e)));
        out.extend_from_slice(&buf[..n * ch]);
        if finished && s.is_idle() {
            return (out, ev);
        }
    }
    panic!("render did not end");
}

fn render_f32(cfg: Config, toks: &[(Token, Vec<u8>)]) -> Vec<f32> {
    let mut s = Synth::new(Config { channels: 1, ..cfg });
    for (t, text) in toks {
        assert!(s.push_with_text(*t, text), "queue full in a short test");
    }
    s.finish();
    let mut out = vec![0.0f32; 60 * cfg.rate as usize];
    let n = s.render_f32(&mut out, |_, _| {});
    assert!(s.is_idle(), "did not end within 60 s");
    out.truncate(n);
    out
}

#[test]
fn durations_follow_the_model_exactly() {
    for text in SENTENCES {
        for q in [1u8, 4, 16, 40] {
            let speed = Speed::from_quarters(q);
            let toks = tokens(text, speed);
            let ms = model_ms(&toks);
            for rate in [16_000u32, 44_100, 48_000] {
                let cfg = Config { rate, channels: 2, speed, ..Config::default() };
                let (pcm, ev) = render(cfg, &toks, 1000);
                assert_eq!(pcm.len() as u64 / 2, frames_for(16 * ms, rate), "{text:?} {q} {rate}");
                assert_eq!(ev.last().map(|e| e.1), Some(Event::End));
                if rate == 16_000 {
                    // Each word starts exactly at the sum of what came before.
                    let mut at = 0u64;
                    let mut starts = Vec::new();
                    let mut fresh = false;
                    for (t, _) in &toks {
                        match t {
                            Token::Word(_) => fresh = true,
                            Token::Phone(s) => {
                                if fresh {
                                    starts.push(16 * at);
                                    fresh = false;
                                }
                                at += u64::from(s.ms);
                            }
                            Token::Break(p) => at += u64::from(p.ms),
                        }
                    }
                    let got: Vec<u64> = ev
                        .iter()
                        .filter(|(_, e)| matches!(e, Event::Word { .. }))
                        .map(|(k, _)| *k as u64)
                        .collect();
                    assert_eq!(got, starts, "{text:?} {q}");
                }
            }
        }
    }
}

#[test]
fn asides_are_faster() {
    let speed = Speed::NORMAL;
    let toks = tokens("The bridge (built long ago) is very narrow.", speed);
    let ms = model_ms(&toks);
    let (pcm, _) = render(Config { rate: 16_000, channels: 1, ..Config::default() }, &toks, 512);
    let got = pcm.len() as u64 / 16;
    assert!(got < ms && got > ms * 85 / 100, "{got} ms vs {ms} ms");
}

#[test]
fn output_is_independent_of_block_sizes_and_feeding() {
    let text = "Hello there. Is it ready? (Yes, it is!) Track 3, 4:05.";
    let toks = tokens(text, Speed::NORMAL);
    let cfg = Config { rate: 44_100, channels: 2, ..Config::default() };
    let (a, ea) = render(cfg, &toks, 4096);
    for block in [1usize, 7, 256, 1023] {
        let (b, eb) = render(cfg, &toks, block);
        assert_eq!(a, b, "block {block}");
        assert_eq!(ea, eb, "block {block}");
    }
    // Text fed in pieces through the speaker gives the same samples.
    let mut sp = Speaker::new(cfg, Options::default());
    let mut out = Vec::new();
    let mut buf = vec![0i16; 2 * 300];
    let mut rest = text.as_bytes();
    loop {
        let n = sp.feed(&rest[..rest.len().min(5)]);
        rest = &rest[n..];
        if rest.is_empty() {
            sp.finish();
        }
        let got = sp.render(&mut buf, |_, _| {});
        out.extend_from_slice(&buf[..2 * got]);
        if sp.is_idle() {
            break;
        }
    }
    assert_eq!(a, out);
}

// ---------------------------------------------------------------------------
// Spectra

/// LPC coefficients (autocorrelation method, Levinson–Durbin).
fn lpc(x: &[f64], p: usize) -> Vec<f64> {
    let r: Vec<f64> = (0..=p).map(|k| x[k..].iter().zip(x).map(|(a, b)| a * b).sum()).collect();
    let mut a = vec![0.0; p + 1];
    a[0] = 1.0;
    let mut e = r[0] * (1.0 + 1e-9);
    for i in 1..=p {
        let mut acc = r[i];
        for j in 1..i {
            acc += a[j] * r[i - j];
        }
        let k = -acc / e;
        let prev = a.clone();
        for j in 1..i {
            a[j] = prev[j] + k * prev[i - j];
        }
        a[i] = k;
        e *= 1.0 - k * k;
    }
    a
}

/// Peaks (Hz) of the LPC envelope of `x` (16 kHz) below 4 kHz.
fn formant_peaks(x: &[f32]) -> Vec<f64> {
    // Pre-emphasis and a Hamming window.
    let n = x.len();
    let y: Vec<f64> = (0..n)
        .map(|i| {
            let pre = f64::from(x[i]) - if i > 0 { 0.9 * f64::from(x[i - 1]) } else { 0.0 };
            pre * (0.54 - 0.46 * (std::f64::consts::TAU * i as f64 / (n - 1) as f64).cos())
        })
        .collect();
    let a = lpc(&y, 18);
    let env: Vec<f64> = (0..=800)
        .map(|k| {
            let w = std::f64::consts::TAU * (f64::from(k) * 5.0) / 16_000.0;
            let (mut re, mut im) = (0.0, 0.0);
            for (j, c) in a.iter().enumerate() {
                re += c * (w * j as f64).cos();
                im -= c * (w * j as f64).sin();
            }
            -10.0 * (re * re + im * im).log10()
        })
        .collect();
    (1..env.len() - 1)
        .filter(|&k| env[k] > env[k - 1] && env[k] >= env[k + 1])
        .map(|k| k as f64 * 5.0)
        .collect()
}

fn one_phone(ph: Ph, stress: Stress, ms: u16, contour: Contour) -> Vec<(Token, Vec<u8>)> {
    vec![
        (Token::Word(Word { pos: 0, phrase_final: true, spelled: false }), b"x".to_vec()),
        (Token::Phone(Seg { phone: Phone::new(ph, stress), ms }), vec![]),
        (Token::Break(Pause { kind: BreakKind::Period, contour, ms: 100 }), vec![]),
    ]
}

#[test]
fn vowel_formants_are_on_target() {
    let mut report = String::new();
    for voice in [Voice::NEUTRAL, Voice::LOW] {
        for ph in ALL.iter().copied().filter(|p| p.class() == orpheus_tts::phoneme::Class::Vowel) {
            let toks = one_phone(ph, Stress::Primary, 400, Contour::Level);
            let x = render_f32(Config { rate: 16_000, voice, ..Config::default() }, &toks);
            let mid = &x[16 * 150..16 * 350];
            let peaks = formant_peaks(mid);
            let t = targets::target(ph, Stress::Primary);
            let mut line = format!("{:<8}{:<3}", voice.name, ph.name());
            for k in 0..3 {
                let want = f64::from(t.f[k]) * f64::from(voice.formants);
                let got = peaks
                    .iter()
                    .copied()
                    .min_by(|a, b| (a - want).abs().total_cmp(&(b - want).abs()))
                    .unwrap();
                let err = (got - want).abs() / want;
                let tol = [0.15, 0.08, 0.08][k];
                line +=
                    &format!("  F{}={got:5.0} ({want:4.0}, {:+5.1}%)", k + 1, 100.0 * (got - want) / want);
                assert!(
                    err <= tol,
                    "{} {}: F{} at {got} Hz, target {want} Hz ({peaks:?})",
                    voice.name,
                    ph.name(),
                    k + 1
                );
            }
            report += &line;
            report.push('\n');
        }
    }
    eprintln!("{report}");
}

#[test]
fn fricatives_have_their_spectra() {
    // /s/ peaks above 4 kHz, /ʃ/ between 2 and 4 kHz; /f/ is weak.
    let band = |x: &[f32], lo: f64, hi: f64| {
        let n = x.len();
        let (mut e, mut tot) = (0.0, 0.0);
        for k in 1..n / 2 {
            let f = k as f64 * 16_000.0 / n as f64;
            let (mut re, mut im) = (0.0, 0.0);
            if k % 4 != 0 {
                continue;
            }
            for (i, v) in x.iter().enumerate() {
                let w = std::f64::consts::TAU * k as f64 * i as f64 / n as f64;
                re += f64::from(*v) * w.cos();
                im += f64::from(*v) * w.sin();
            }
            let p = re * re + im * im;
            tot += p;
            if f >= lo && f < hi {
                e += p;
            }
        }
        e / tot
    };
    let rms = |x: &[f32]| (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt();
    let cfg = Config { rate: 16_000, ..Config::default() };
    let s = render_f32(cfg, &one_phone(Ph::S, Stress::None, 200, Contour::Level));
    let sh = render_f32(cfg, &one_phone(Ph::SH, Stress::None, 200, Contour::Level));
    let f = render_f32(cfg, &one_phone(Ph::F, Stress::None, 200, Contour::Level));
    let mid = |x: &Vec<f32>| x[16 * 50..16 * 150].to_vec();
    assert!(band(&mid(&s), 4000.0, 8000.0) > 0.6, "/s/ {}", band(&mid(&s), 4000.0, 8000.0));
    assert!(band(&mid(&sh), 1800.0, 4000.0) > 0.5, "/sh/ {}", band(&mid(&sh), 1800.0, 4000.0));
    assert!(rms(&mid(&f)) < 0.3 * rms(&mid(&s)), "/f/ not weaker than /s/");
}

// ---------------------------------------------------------------------------
// Pitch

/// F0 track (Hz, 0 = unvoiced) in 5 ms hops over `win`-sample windows:
/// the shortest lag whose normalised autocorrelation is within 7 % of the
/// best (no subharmonic octave errors), voiced above 0.6; lags 40–255
/// (63–400 Hz).
fn f0_track(x: &[f32], win: usize) -> Vec<f64> {
    let mut v = Vec::new();
    let mut at = 0;
    let energy = |a: usize| -> f64 { x[a..a + win].iter().map(|s| f64::from(*s).powi(2)).sum() };
    while at + win + 256 <= x.len() {
        let e0 = energy(at);
        if e0 < 1e-5 * win as f64 {
            v.push(0.0);
            at += 80;
            continue;
        }
        let nc: Vec<f64> = (0..=256)
            .map(|l| {
                if l < 40 {
                    return 0.0;
                }
                let mut s = 0.0;
                for i in 0..win {
                    s += f64::from(x[at + i]) * f64::from(x[at + i + l]);
                }
                s / (e0 * energy(at + l)).sqrt().max(1e-12)
            })
            .collect();
        let best = nc.iter().copied().fold(0.0, f64::max);
        let lag = (41..256).find(|&l| nc[l] >= 0.93 * best && nc[l] >= nc[l - 1] && nc[l] >= nc[l + 1]);
        v.push(match lag {
            Some(l) if best > 0.6 => 16_000.0 / l as f64,
            _ => 0.0,
        });
        at += 80;
    }
    v
}

/// Voiced frames inside runs of at least four (isolated detections at
/// bursts are dropped).
fn voiced(t: &[f64]) -> Vec<f64> {
    let mut out = Vec::new();
    let mut run = Vec::new();
    for &f in t.iter().chain([0.0].iter()) {
        if f > 0.0 {
            run.push(f);
        } else {
            if run.len() >= 4 {
                out.extend_from_slice(&run);
            }
            run.clear();
        }
    }
    out
}

fn speak_f32(text: &str, speed: Speed, voice: Voice) -> Vec<f32> {
    let toks = tokens(text, speed);
    render_f32(Config { rate: 16_000, speed, voice, ..Config::default() }, &toks)
}

/// The synthesiser's own F0 per voiced 1 ms tick ([`Synth::pitch`]).
fn pitch_trace(text: &str, speed: Speed, voice: Voice) -> Vec<f64> {
    let toks = tokens(text, speed);
    let mut s = Synth::new(Config { rate: 16_000, channels: 1, speed, voice, ..Config::default() });
    for (t, w) in &toks {
        assert!(s.push_with_text(*t, w));
    }
    s.finish();
    let mut buf = [0i16; 16];
    let mut v = Vec::new();
    while !s.is_idle() {
        s.render(&mut buf, |_, _| {});
        if let Some(f) = s.pitch() {
            v.push(f64::from(f));
        }
    }
    v
}

fn end3(v: &[f64]) -> f64 {
    let mut t = v[v.len() - 3..].to_vec();
    t.sort_by(f64::total_cmp);
    t[1]
}

fn peak(v: &[f64]) -> f64 {
    v.iter().copied().fold(0.0, f64::max)
}

#[test]
fn questions_rise_and_statements_fall() {
    // Measured on the audio (pitch tracker) for the Neutral voice, on the
    // synthesiser's own F0 for all voices.
    let audio = |t: &str| voiced(&f0_track(&speak_f32(t, Speed::NORMAL, Voice::NEUTRAL), 480));
    let traces: Vec<(String, [Vec<f64>; 3])> = std::iter::once((
        "Neutral (audio)".to_string(),
        [audio("Is it ready?"), audio("It is ready."), audio("Where is it?")],
    ))
    .chain(Voice::ALL.iter().map(|v| {
        let tr = |t: &str| pitch_trace(t, Speed::NORMAL, *v);
        (v.name.to_string(), [tr("Is it ready?"), tr("It is ready."), tr("Where is it?")])
    }))
    .collect();
    for (name, [q, s, wh]) in &traces {
        eprintln!(
            "{name}: question end {:.0} Hz, statement end {:.0} Hz (peak {:.0}), wh-question end {:.0} Hz (peak {:.0})",
            end3(q),
            end3(s),
            peak(s),
            end3(wh),
            peak(wh)
        );
        assert!(end3(q) > 1.25 * end3(s), "{name}: question end {} vs statement end {}", end3(q), end3(s));
        assert!(end3(s) < 0.85 * peak(s), "{name}: statement does not fall");
        assert!(end3(q) > 1.15 * q[q.len() / 3], "{name}: question does not rise");
        assert!(end3(wh) < 0.85 * peak(wh), "{name}: wh-question does not fall");
    }
}

#[test]
fn pitch_stays_put_across_speeds() {
    // The mean F0 of a sentence does not move with the speed (no
    // resampling; only the declination's time scale and the depth of the
    // movements change): on the audio up to 4x, on the synthesiser's F0 at
    // every speed (at 10x the voiced stretches are 15–25 ms, too short for
    // the tracker).
    let text = "We will meet at the station after lunch, and then we go home.";
    let audio = |q: u8| {
        let v = voiced(&f0_track(&speak_f32(text, Speed::from_quarters(q), Voice::NEUTRAL), 480));
        v.iter().sum::<f64>() / v.len() as f64
    };
    let own = |q: u8, v: Voice| {
        let t = pitch_trace(text, Speed::from_quarters(q), v);
        t.iter().sum::<f64>() / t.len() as f64
    };
    let base = audio(4);
    for q in [1u8, 8, 16] {
        let m = audio(q);
        eprintln!("{:.2}x: mean F0 {m:.1} Hz on the audio (1x {base:.1})", f32::from(q) / 4.0);
        assert!((m / base - 1.0).abs() < 0.08, "{q}: {m} vs {base}");
    }
    for v in Voice::ALL {
        let base = own(4, v);
        for q in [1u8, 8, 16, 24, 40] {
            let m = own(q, v);
            eprintln!("{} {:.2}x: mean F0 {m:.1} Hz (1x {base:.1})", v.name, f32::from(q) / 4.0);
            assert!((m / base - 1.0).abs() < 0.1, "{} {q}: {m} vs {base}", v.name);
        }
    }
}

// ---------------------------------------------------------------------------
// Levels, events, stop, RAM

#[test]
fn no_clipping_and_no_nan() {
    let mut worst = 0.0f32;
    for voice in Voice::ALL {
        for q in [1u8, 4, 16, 40] {
            for text in SENTENCES {
                let x = speak_f32(text, Speed::from_quarters(q), voice);
                assert!(x.iter().all(|v| v.is_finite()));
                let pk = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                assert!(pk > 0.05, "{} {q} {text:?}: silent", voice.name);
                worst = worst.max(pk);
            }
        }
    }
    eprintln!("peak {worst:.3} ({:.1} dBFS)", 20.0 * worst.log10());
    // Below the soft clipper's knee: nothing is clipped or bent.
    assert!(worst < 0.8, "peak {worst}");
}

#[test]
fn events_mark_sentences_and_words() {
    let text = "One two. Three four five? Six!";
    let toks = tokens(text, Speed::NORMAL);
    let (_, ev) = render(Config::default(), &toks, 333);
    let words: Vec<u32> = ev
        .iter()
        .filter_map(|(_, e)| if let Event::Word { pos, .. } = e { Some(*pos) } else { None })
        .collect();
    let sents: Vec<u32> = ev
        .iter()
        .filter_map(|(_, e)| if let Event::Sentence { pos } = e { Some(*pos) } else { None })
        .collect();
    assert_eq!(words.len(), 6);
    assert_eq!(sents, vec![0, 9, 26]);
    assert!(words.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(ev.iter().filter(|(_, e)| *e == Event::End).count(), 1);
    // Frames are increasing.
    assert!(ev.windows(2).all(|w| w[0].0 <= w[1].0));
}

#[test]
fn stop_now_fades_quickly() {
    let cfg = Config { rate: 48_000, channels: 1, ..Config::default() };
    let mut sp = Speaker::new(cfg, Options::default());
    sp.feed(b"This is a long sentence that will be interrupted in the middle of a word.");
    sp.finish();
    let mut buf = vec![0i16; 48_000];
    let n = sp.render(&mut buf[..24_000], |_, _| {});
    assert_eq!(n, 24_000);
    sp.stop(Stop::Now);
    let mut ended = None;
    let n = sp.render(&mut buf, |k, e| {
        if e == Event::End {
            ended = Some(k);
        }
    });
    assert!(n <= 3 * 48 + 48, "{n} frames after stop");
    assert_eq!(ended, Some(n));
    assert!(sp.is_idle());
    // It speaks again afterwards.
    sp.feed(b"Again.");
    sp.finish();
    assert!(sp.render(&mut buf, |_, _| {}) > 4000);
}

#[test]
fn stop_at_word_finishes_the_word() {
    let text = "alpha bravo charlie delta echo foxtrot";
    let toks = tokens(text, Speed::NORMAL);
    let cfg = Config { rate: 16_000, channels: 1, ..Config::default() };
    let (_, full) = render(cfg, &toks, 512);
    let starts: Vec<usize> =
        full.iter().filter(|(_, e)| matches!(e, Event::Word { .. })).map(|(k, _)| *k).collect();
    // Stop in the middle of the third word.
    let mid = (starts[2] + starts[3]) / 2;
    let mut s = Synth::new(cfg);
    for (t, w) in &toks {
        s.push_with_text(*t, w);
    }
    let mut buf = vec![0i16; mid];
    let mut words = 0;
    assert_eq!(s.render(&mut buf, |_, e| words += usize::from(matches!(e, Event::Word { .. }))), mid);
    s.stop(Stop::AtWord);
    let mut more = vec![0i16; 100_000];
    let mut end = None;
    let n = s.render(&mut more, |k, e| {
        if matches!(e, Event::Word { .. }) {
            words += 1;
        }
        if e == Event::End {
            end = Some(mid + k);
        }
    });
    assert_eq!(words, 3, "no word starts after the stop");
    let end = end.expect("ended");
    assert!(end >= starts[3] && end <= starts[3] + 100, "ended at {end}, next word at {}", starts[3]);
    assert!(n < 100_000 && s.is_idle());
}

#[test]
fn ram_within_budget() {
    let (synth, dsp, rs, speaker) = (ram::synth(), ram::dsp(), ram::resampler(), ram::speaker());
    eprintln!(
        "RAM: synth {synth} B (dsp {dsp} B, resampler {rs} B), speaker (converter + synth) {speaker} B"
    );
    // 04 §8: ≤ 8 KiB for the speech engine.
    assert!(speaker <= 8 * 1024, "speaker {speaker} B");
    assert!(synth <= 6 * 1024, "synth {synth} B");
}

#[test]
fn every_phoneme_renders_in_every_position() {
    // Each phoneme between vowels, after silence and before silence, at
    // the slowest and fastest speeds: finite, bounded, audible.
    for ph in ALL {
        for (q, ms) in [(1u8, 300u16), (40, 12)] {
            let mut toks =
                vec![(Token::Word(Word { pos: 0, phrase_final: true, spelled: false }), b"x".to_vec())];
            for p in [
                Phone::new(ph, Stress::Primary),
                Phone::new(Ph::AA, Stress::Primary),
                Phone::new(ph, Stress::Unstressed),
            ] {
                toks.push((Token::Phone(Seg { phone: p, ms }), vec![]));
            }
            toks.push((
                Token::Break(Pause { kind: BreakKind::Question, contour: Contour::Rise, ms: 50 }),
                vec![],
            ));
            let x = render_f32(
                Config { rate: 16_000, speed: Speed::from_quarters(q), ..Config::default() },
                &toks,
            );
            assert!(x.iter().all(|v| v.is_finite() && v.abs() <= 1.0), "{ph:?}");
            assert_eq!(x.len(), 16 * (3 * usize::from(ms) + 50));
        }
    }
}
