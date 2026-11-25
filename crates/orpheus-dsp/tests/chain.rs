//! The whole chain: bit-exact identity when flat, no overflow at full scale
//! with +12 dB boosts, click-free parameter changes, gains, crossfeed,
//! channel modes, meters, decimation through the chain, cost model.

use std::f64::consts::PI;

use orpheus_audio::{PcmBlock, SampleKind};
use orpheus_dsp::chain::HEADROOM_MB;
use orpheus_dsp::cost::StageId;
use orpheus_dsp::eq::EQ5_FREQS;
use orpheus_dsp::fixed::UNITY;
use orpheus_dsp::meter::db10;
use orpheus_dsp::{
    Band, Chain, ChannelMode, CrossfeedParams, DitherMode, EqPreamp, Format, MeterMode, OutWidth, Preset,
    RgMode, RgSettings, RgTags,
};

const FMT16: Format = Format { rate: 44100, channels: 2, kind: SampleKind::I16, bits: 16 };
const FMT24: Format = Format { rate: 48000, channels: 2, kind: SampleKind::I32, bits: 24 };

/// A deterministic noisy stereo test signal (sum of sines plus LCG noise),
/// peaking near −6 dBFS.
fn music16(frames: usize, seed: u32) -> Vec<i16> {
    let mut s = seed;
    let mut v = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        for ch in 0..2 {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = f64::from((s >> 16) as i16) / 32768.0 * 0.05;
            let t = i as f64 / 44100.0;
            let x = 0.2 * (2.0 * PI * 110.0 * t + f64::from(ch)).sin()
                + 0.15 * (2.0 * PI * 1234.0 * t).sin()
                + 0.05 * (2.0 * PI * 9876.0 * t).sin()
                + noise;
            v.push((x * 32767.0) as i16);
        }
    }
    v
}

fn sine16(frames: usize, f: f64, amp: f64, rate: f64) -> Vec<i16> {
    (0..frames * 2)
        .map(|i| (amp * 32767.0 * (2.0 * PI * f * (i / 2) as f64 / rate).sin()).round() as i16)
        .collect()
}

/// Process `x` in decoder-sized blocks of `block` frames; returns the output.
fn run16(c: &mut Chain, x: &[i16], block: usize) -> Vec<i16> {
    let mut out = Vec::new();
    for b in x.chunks(block * 2) {
        let mut v = b.to_vec();
        let n = c.process(&mut PcmBlock::I16(&mut v));
        out.extend_from_slice(&v[..n]);
    }
    out
}

fn run32(c: &mut Chain, x: &[i32], block: usize) -> Vec<i32> {
    let mut out = Vec::new();
    for b in x.chunks(block * 2) {
        let mut v = b.to_vec();
        let n = c.process(&mut PcmBlock::I32(&mut v));
        out.extend_from_slice(&v[..n]);
    }
    out
}

#[test]
fn flat_chain_is_bit_exact_identity() {
    let x = music16(20_000, 1);
    let mut c = Chain::new(OutWidth::Bits16, false);
    assert_eq!(c.set_format(FMT16), Ok(44100));
    assert_eq!(c.active(), 0);
    assert_eq!(run16(&mut c, &x, 1152), x);
    // Every setting at its flat value.
    c.set_eq_preset(Preset::Flat);
    c.set_eq(&[Band::peaking(1000, 70, 0); 10]);
    c.set_bass(0);
    c.set_treble(0);
    c.set_crossfeed(None);
    c.set_balance(0);
    c.set_channel_mode(ChannelMode::Stereo);
    c.set_replaygain(RgSettings { mode: RgMode::Off, ..RgSettings::default() });
    c.set_meter(MeterMode::PeakRms); // meters read, never write
    assert_eq!(c.active() & !(1 << StageId::Meter as u16), 0);
    assert_eq!(run16(&mut c, &x, 1000), x);
    assert_eq!(c.cost().total, c.cost().per_stage[StageId::Meter as usize]);

    // 24-bit source to a 24-bit FIFO: also untouched.
    let x32: Vec<i32> =
        x.iter().enumerate().map(|(i, &v)| (i32::from(v) << 16) | ((i as i32 & 0xFF) << 8)).collect();
    let mut c = Chain::new(OutWidth::Bits24, false);
    c.set_format(FMT24).unwrap();
    assert_eq!(c.active(), 0);
    assert_eq!(run32(&mut c, &x32, 4608), x32);
}

#[test]
fn stages_return_to_bit_exact_after_ramping_back() {
    let x = music16(40_000, 2);
    let mut c = Chain::new(OutWidth::Bits16, false);
    c.set_format(FMT16).unwrap();
    c.set_dither(DitherMode::Off);
    let _ = run16(&mut c, &x[..20_000], 1152);
    c.set_eq_preset(Preset::Rock);
    c.set_bass(600);
    c.set_crossfeed(Some(CrossfeedParams::DEFAULT));
    c.set_balance(-30);
    c.set_channel_mode(ChannelMode::Mono);
    c.set_replaygain(RgSettings { mode: RgMode::Track, ..RgSettings::default() });
    c.set_track_gains(RgTags { track_gain_mb: Some(-450), ..RgTags::default() });
    c.fade_to(UNITY / 2, 20);
    let _ = run16(&mut c, &x, 1152);
    assert_ne!(c.active(), 0);
    c.set_eq_preset(Preset::Flat);
    c.set_bass(0);
    c.set_crossfeed(None);
    c.set_balance(0);
    c.set_channel_mode(ChannelMode::Stereo);
    c.set_replaygain(RgSettings::default());
    c.fade_in(20);
    // Ramps last at most 40 ms; the limiter envelope decays below its knee.
    let _ = run16(&mut c, &x, 1152);
    assert_eq!(c.active(), 0, "active {:#x}", c.active());
    assert_eq!(run16(&mut c, &x, 1152), x);
}

#[test]
fn no_overflow_at_full_scale_with_12_db_boosts() {
    let mut c = Chain::new(OutWidth::Bits16, false);
    c.set_format(FMT16).unwrap();
    c.set_dither(DitherMode::Off);
    // +12 dB on every band, pre-amp forced to 0 dB, +12 dB bass and treble,
    // ReplayGain +10 dB without clip prevention: the headroom guard and the
    // limiter must keep everything in range.
    let bands: Vec<Band> = EQ5_FREQS.iter().map(|&f| Band::peaking(f, 70, 1200)).collect();
    c.set_eq(&bands);
    c.set_eq_preamp(EqPreamp::Manual(0));
    c.set_bass(1200);
    c.set_treble(1200);
    c.set_replaygain(RgSettings { mode: RgMode::Track, prevent_clip: false, ..RgSettings::default() });
    c.set_track_gains(RgTags { track_gain_mb: Some(1000), ..RgTags::default() });
    assert!(c.gain_mb() <= HEADROOM_MB);
    let ceiling = (32768.0 * 10f64.powf(-0.1 / 20.0)).ceil() as i32 + 1;
    let mut inputs: Vec<(String, Vec<i16>)> = Vec::new();
    for &f in EQ5_FREQS.iter().chain(&[1000, 20]) {
        inputs.push((format!("sine {f} Hz"), sine16(44100, f64::from(f), 1.0, 44100.0)));
    }
    // Full-scale square wave and full-scale random noise.
    inputs.push((
        "square".into(),
        (0..88200).map(|i| if (i / 2 / 50) % 2 == 0 { 32767 } else { -32768 }).collect(),
    ));
    let mut s = 1u32;
    inputs.push((
        "noise".into(),
        (0..88200)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                if s & 1 == 0 { 32767 } else { -32768 }
            })
            .collect(),
    ));
    for (name, x) in &inputs {
        c.reset();
        let y = run16(&mut c, x, 1152);
        let peak = y.iter().map(|v| i32::from(*v).abs()).max().unwrap_or(0);
        assert!(peak <= ceiling, "{name}: peak {peak} above ceiling {ceiling}");
        // A wrap inside the chain would show as garbage; the limited sine
        // must stay a sine (fit after 0.1 s; the limiter's gain moves only
        // slowly, so the residual is small).
        if let Some(f) = name.strip_prefix("sine ").and_then(|v| v.strip_suffix(" Hz")) {
            let f: f64 = f.parse().unwrap();
            let left: Vec<f64> = y.iter().step_by(2).skip(4410).map(|v| f64::from(*v)).collect();
            let (amp, snr) = fit(&left, f, 44100.0);
            println!(
                "{name}: limited to {:.2} dBFS, residual {:.1} dB",
                20.0 * (amp / 32768.0).log10(),
                -snr
            );
            assert!(snr > 20.0, "{name}: not a sine any more ({snr:.1} dB)");
        }
        assert!(c.take_limited() > 0, "{name}: limiter engaged");
    }
}

/// Fit a sine at `f` Hz: (amplitude, signal-to-residual in dB).
fn fit(y: &[f64], f: f64, fs: f64) -> (f64, f64) {
    let (mut ss, mut sc, mut cc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &v) in y.iter().enumerate() {
        let t = 2.0 * PI * f * i as f64 / fs;
        let (s, c) = (t.sin(), t.cos());
        ss += s * s;
        sc += s * c;
        cc += c * c;
        ys += v * s;
        yc += v * c;
    }
    let det = ss * cc - sc * sc;
    let (a, b) = ((ys * cc - yc * sc) / det, (yc * ss - ys * sc) / det);
    let (mut sig, mut err) = (0.0, 0.0);
    for (i, &v) in y.iter().enumerate() {
        let t = 2.0 * PI * f * i as f64 / fs;
        let m = a * t.sin() + b * t.cos();
        sig += m * m;
        err += (v - m) * (v - m);
    }
    ((a * a + b * b).sqrt(), 10.0 * (sig / err.max(1e-9)).log10())
}

/// Largest |second difference| of the left channel over `y[a..b]` frames.
fn d2(y: &[i16], a: usize, b: usize) -> i32 {
    (a.max(2)..b)
        .map(|i| {
            let v = |k: usize| i32::from(y[2 * k]);
            (v(i) - 2 * v(i - 1) + v(i - 2)).abs()
        })
        .max()
        .unwrap_or(0)
}

/// Click threshold: the second difference may exceed the larger steady
/// state by at most this many LSB (−66 dBFS). A step of the signal shows in
/// the second difference at its full size; the linear 30 ms ramps produce
/// only a change of slope (≈ 5–10 LSB at −6 dBFS). The control in
/// `parameter_changes_are_click_free` shows that a 1 dB step is caught.
const CLICK_LSB: i32 = 16;

/// Run a 200 Hz sine at −6 dBFS on the left and a 150 Hz sine at −10 dBFS
/// on the right, apply `change` at a peak of the left; returns the largest
/// second difference of the left output before, during (100 ms) and after
/// the change, and the limit: the larger steady state plus [`CLICK_LSB`].
fn click_metric(setup: &impl Fn(&mut Chain), change: &impl Fn(&mut Chain)) -> (i32, i32, i32, i32) {
    let mut c = Chain::new(OutWidth::Bits16, false);
    c.set_format(FMT16).unwrap();
    c.set_dither(DitherMode::Off);
    setup(&mut c);
    let l = sine16(44100 * 2, 200.0, 0.5, 44100.0);
    let r = sine16(44100 * 2, 150.0, 0.316, 44100.0);
    let x: Vec<i16> = l.chunks(2).zip(r.chunks(2)).flat_map(|(a, b)| [a[0], b[1]]).collect();
    // 0.5 s plus a quarter period: the change lands on a peak of the sine,
    // where a gain step is largest.
    let half = 22050 + 55;
    let mut y = run16(&mut c, &x[..2 * half], 512);
    change(&mut c);
    y.extend(run16(&mut c, &x[2 * half..], 512));
    let before = d2(&y, half - 4410, half);
    let during = d2(&y, half, half + 4410);
    let after = d2(&y, y.len() / 2 - 4410, y.len() / 2);
    (before, during, after, before.max(after) + CLICK_LSB)
}

/// [`click_metric`], asserted.
fn click_test(name: &str, setup: impl Fn(&mut Chain), change: impl Fn(&mut Chain)) {
    let (before, during, after, limit) = click_metric(&setup, &change);
    println!("{name}: Δ² before {before}, during {during}, after {after}");
    assert!(during <= limit, "{name}: click, Δ² {during} > {limit}");
}

#[test]
fn parameter_changes_are_click_free() {
    let none = |_: &mut Chain| {};
    // Control: an unramped 1 dB gain step must count as a click.
    let (_, during, _, limit) = click_metric(&none, &|c: &mut Chain| c.fade_to(UNITY / 100 * 89, 0));
    println!("control (1 dB step): Δ² {during}, limit {limit}");
    assert!(during > 4 * limit, "the click metric is not sensitive enough");
    click_test("EQ flat → rock", none, |c| c.set_eq_preset(Preset::Rock));
    click_test("EQ rock → vocal", |c| c.set_eq_preset(Preset::Rock), |c| c.set_eq_preset(Preset::Vocal));
    click_test("EQ rock → flat", |c| c.set_eq_preset(Preset::Rock), |c| c.set_eq_preset(Preset::Flat));
    click_test("EQ band 230 Hz +12", none, |c| c.set_eq_band(1, Band::peaking(230, 70, 1200)));
    click_test(
        "EQ band 230 Hz −12 → +12",
        |c| c.set_eq_band(1, Band::peaking(230, 70, -1200)),
        |c| c.set_eq_band(1, Band::peaking(230, 70, 1200)),
    );
    click_test(
        "EQ band retune 100 → 400 Hz",
        |c| c.set_eq_band(0, Band::peaking(100, 100, 900)),
        |c| c.set_eq_band(0, Band::peaking(400, 300, 900)),
    );
    click_test("bass +10", none, |c| c.set_bass(1000));
    click_test("treble −8", none, |c| c.set_treble(-800));
    click_test("crossfeed on", none, |c| c.set_crossfeed(Some(CrossfeedParams::MEIER)));
    click_test("crossfeed off", |c| c.set_crossfeed(Some(CrossfeedParams::MEIER)), |c| c.set_crossfeed(None));
    click_test(
        "crossfeed retune",
        |c| c.set_crossfeed(Some(CrossfeedParams::DEFAULT)),
        |c| c.set_crossfeed(Some(CrossfeedParams::MEIER)),
    );
    click_test("balance right", none, |c| c.set_balance(80));
    click_test("mono", none, |c| c.set_channel_mode(ChannelMode::Mono));
    click_test("swap", none, |c| c.set_channel_mode(ChannelMode::Swap));
    click_test("right only", none, |c| c.set_channel_mode(ChannelMode::RightOnly));
    click_test(
        "ReplayGain −8 dB",
        |c| c.set_replaygain(RgSettings { mode: RgMode::Track, ..RgSettings::default() }),
        |c| c.set_track_gains(RgTags { track_gain_mb: Some(-800), ..RgTags::default() }),
    );
    click_test("fade out 30 ms", none, |c| c.fade_out(30));
}

#[test]
fn replaygain_level_is_accurate() {
    let mut c = Chain::new(OutWidth::Bits16, false);
    c.set_format(FMT16).unwrap();
    c.set_dither(DitherMode::Off);
    c.set_replaygain(RgSettings { mode: RgMode::Album, preamp_mb: 0, prevent_clip: true, fallback_mb: 0 });
    c.set_track_gains(RgTags {
        album_gain_mb: Some(-602),
        album_peak_ppm: Some(900_000),
        ..RgTags::default()
    });
    let x = sine16(44100, 1000.0, 0.8, 44100.0);
    let y = run16(&mut c, &x, 1152);
    let pk = |v: &[i16]| v.iter().map(|s| i32::from(*s).abs()).max().unwrap_or(0);
    let ratio = f64::from(pk(&y[44100..])) / f64::from(pk(&x[44100..]));
    let db = 20.0 * ratio.log10();
    println!("ReplayGain −6.02 dB measured {db:.4} dB");
    assert!((db + 6.02).abs() < 0.01);
}

#[test]
fn crossfeed_passes_mono_and_feeds_bass() {
    let mut c = Chain::new(OutWidth::Bits16, false);
    c.set_format(FMT16).unwrap();
    c.set_dither(DitherMode::Off);
    c.set_crossfeed(Some(CrossfeedParams::DEFAULT));
    c.set_limiter(false);
    // Mono (L = R) at several frequencies: unchanged within 1 LSB.
    for f in [50.0, 700.0, 5000.0] {
        let x = sine16(8820, f, 0.7, 44100.0);
        let y = run16(&mut c, &x, 1152);
        let e =
            x.iter().zip(&y).skip(4410).map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs()).max().unwrap();
        assert!(e <= 1, "{f} Hz mono changed by {e}");
    }
    // Left only, 50 Hz: the right ear gets k/(1+k) of it (−4.5 dB vs direct).
    let x: Vec<i16> = sine16(44100, 50.0, 0.7, 44100.0).chunks(2).flat_map(|f| [f[0], 0]).collect();
    let y = run16(&mut c, &x, 1152);
    let pk = |ch: usize| y.iter().skip(22050 * 2 + ch).step_by(2).map(|s| i32::from(*s).abs()).max().unwrap();
    let rel = 20.0 * (f64::from(pk(1)) / f64::from(pk(0))).log10();
    println!("crossfeed at 50 Hz: cross/direct {rel:.2} dB");
    assert!((rel + 4.5).abs() < 0.2);
}

#[test]
fn channel_modes_are_exact() {
    let x = music16(4410, 3);
    let expect = |m: ChannelMode, l: i16, r: i16| -> (i16, i16) {
        match m {
            ChannelMode::Stereo => (l, r),
            ChannelMode::Swap => (r, l),
            ChannelMode::LeftOnly => (l, l),
            ChannelMode::RightOnly => (r, r),
            ChannelMode::Mono => {
                let m = (f64::from(i32::from(l) + i32::from(r)) / 2.0).round() as i16;
                (m, m)
            }
        }
    };
    for m in [ChannelMode::Swap, ChannelMode::LeftOnly, ChannelMode::RightOnly, ChannelMode::Mono] {
        let mut c = Chain::new(OutWidth::Bits16, false);
        c.set_format(FMT16).unwrap();
        c.set_dither(DitherMode::Off);
        c.set_channel_mode(m);
        c.reset(); // finish the ramp
        let y = run16(&mut c, &x, 1152);
        for (i, f) in x.chunks(2).enumerate() {
            let (l, r) = expect(m, f[0], f[1]);
            let tol = i16::from(m == ChannelMode::Mono);
            assert!((y[2 * i] - l).abs() <= tol && (y[2 * i + 1] - r).abs() <= tol, "{m:?} frame {i}");
        }
    }
}

#[test]
fn meters_read_levels() {
    let mut c = Chain::new(OutWidth::Bits16, false);
    c.set_format(FMT16).unwrap();
    c.set_meter(MeterMode::PeakRms);
    let x = sine16(44100, 1000.0, 0.5, 44100.0);
    let _ = run16(&mut c, &x, 1152);
    let l = c.take_levels();
    println!("{l:?}");
    assert_eq!(l.frames, 44100);
    assert!((db10(l.peak[0]) + 60).abs() <= 1);
    // RMS of a sine is 3.01 dB under its peak.
    assert!((db10(l.rms[0]) + 90).abs() <= 1);
    assert_eq!(l.clipped, 0);
}

#[test]
fn hi_res_through_the_chain() {
    // 24-bit 96 kHz → 48 kHz, 16-bit FIFO with plain TPDF dither.
    let mut c = Chain::new(OutWidth::Bits16, false);
    assert_eq!(c.set_format(Format { rate: 96000, channels: 2, kind: SampleKind::I32, bits: 24 }), Ok(48000));
    assert_ne!(c.active() & (1 << StageId::Decimator as u16), 0);
    c.set_dither(DitherMode::Tpdf);
    let x: Vec<i32> = (0..96000 * 2)
        .map(|i| {
            ((0.5 * (2.0 * PI * 1000.0 * f64::from(i / 2) / 96000.0).sin() * 8_388_607.0).round() as i32) << 8
        })
        .collect();
    let y = run32(&mut c, &x, 4608);
    assert_eq!(y.len(), x.len() / 2);
    assert!(y.iter().all(|v| v & 0xFFFF == 0), "16-bit output, left-justified");
    // SNR against a fitted sine: TPDF at 16 bits (error variance LSB²/4)
    // allows 87.3 dB for a −6 dBFS sine; anything the decimator adds shows.
    let left: Vec<f64> = y.iter().step_by(2).skip(1000).map(|v| f64::from(v >> 16)).collect();
    let (mut ss, mut sc, mut cc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &v) in left.iter().enumerate() {
        let t = 2.0 * PI * 1000.0 * i as f64 / 48000.0;
        let (s, co) = (t.sin(), t.cos());
        ss += s * s;
        sc += s * co;
        cc += co * co;
        ys += v * s;
        yc += v * co;
    }
    let det = ss * cc - sc * sc;
    let (a, b) = ((ys * cc - yc * sc) / det, (yc * ss - ys * sc) / det);
    let (mut sig, mut err) = (0.0, 0.0);
    for (i, &v) in left.iter().enumerate() {
        let t = 2.0 * PI * 1000.0 * i as f64 / 48000.0;
        let m = a * t.sin() + b * t.cos();
        sig += m * m;
        err += (v - m) * (v - m);
    }
    let snr = 10.0 * (sig / err).log10();
    let level = 20.0 * ((a * a + b * b).sqrt() / 16383.5).log10();
    println!("96k→48k 24→16 bit: level {level:+.4} dB, SNR {snr:.1} dB");
    assert!(level.abs() < 0.01);
    assert!(snr > 87.0);

    // 192 kHz → 48 kHz with odd block sizes.
    assert_eq!(
        c.set_format(Format { rate: 192_000, channels: 2, kind: SampleKind::I32, bits: 24 }),
        Ok(48000)
    );
    let y = run32(&mut c, &x, 1001);
    assert_eq!(y.len(), x.len() / 4);
    assert_eq!(
        c.set_format(Format { rate: 64000, channels: 2, kind: SampleKind::I32, bits: 24 }),
        Err(orpheus_audio::Unsupported::Rate(64000))
    );
}

#[test]
fn twenty_four_bit_source_is_dithered_to_sixteen() {
    let mut c = Chain::new(OutWidth::Bits16, false);
    c.set_format(FMT24).unwrap();
    assert_eq!(c.active(), 1 << StageId::Output as u16);
    let x: Vec<i32> = (0..9600).map(|i| (i * 977) << 8).collect();
    let y = run32(&mut c, &x, 4608);
    assert!(y.iter().all(|v| v & 0xFFFF == 0));
    // Same 16-bit content in an I32 block needs no dither.
    let mut c = Chain::new(OutWidth::Bits16, false);
    c.set_format(Format { bits: 16, ..FMT24 }).unwrap();
    assert_eq!(c.active(), 0);
}

#[test]
fn mono_stream() {
    let mut c = Chain::new(OutWidth::Bits16, false);
    c.set_format(Format { channels: 1, ..FMT16 }).unwrap();
    c.set_crossfeed(Some(CrossfeedParams::DEFAULT));
    c.set_channel_mode(ChannelMode::Swap);
    // Stereo-only stages do nothing on a mono stream.
    assert_eq!(c.active(), 0);
    c.set_eq_preset(Preset::BassBoost);
    let x: Vec<i16> = sine16(4410, 100.0, 0.3, 44100.0).into_iter().step_by(2).collect();
    let mut v = x.clone();
    let n = c.process(&mut PcmBlock::I16(&mut v));
    assert_eq!(n, x.len());
}

#[test]
fn cost_model_table() {
    let mut c = Chain::new(OutWidth::Bits16, false);
    c.set_format(FMT16).unwrap();
    let show = |name: &str, c: &Chain| {
        let k = c.cost();
        println!(
            "{name:<40} {:>4} cycles/frame  {:>5.2} % of 48 MHz  per stage {:?}",
            k.total,
            f64::from(k.load_bp(c.output_rate(), 48_000_000)) / 100.0,
            k.per_stage
        );
        k.total
    };
    assert_eq!(show("flat", &c), 0);
    c.set_replaygain(RgSettings { mode: RgMode::Track, ..RgSettings::default() });
    c.set_track_gains(RgTags { track_gain_mb: Some(-700), ..RgTags::default() });
    c.reset();
    show("ReplayGain −7 dB", &c);
    c.set_replaygain(RgSettings::default());
    c.set_eq_preset(Preset::Rock);
    c.reset();
    show("EQ 5 bands (Rock)", &c);
    c.set_eq_preset(Preset::Flat);
    c.reset();
    c.set_crossfeed(Some(CrossfeedParams::DEFAULT));
    c.reset();
    show("crossfeed", &c);
    c.set_crossfeed(None);
    c.reset();
    c.set_format(FMT24).unwrap();
    show("24-bit, flat (TPDF dither)", &c);
    c.set_format(Format { rate: 96000, ..FMT24 }).unwrap();
    show("96 kHz 24-bit → 48 kHz", &c);
    c.set_format(Format { rate: 192_000, ..FMT24 }).unwrap();
    show("192 kHz 24-bit → 48 kHz", &c);
    c.set_format(FMT16).unwrap();
    c.set_eq(&[Band::peaking(1000, 70, 300); 10]);
    c.set_bass(300);
    c.set_treble(300);
    c.set_crossfeed(Some(CrossfeedParams::DEFAULT));
    c.set_balance(10);
    c.set_meter(MeterMode::PeakRms);
    c.reset();
    show("everything (10 bands)", &c);
}

#[test]
fn ram_per_stage() {
    use core::mem::size_of;
    use orpheus_dsp::{crossfeed, decimate, dither, eq, fade, gain, limiter, matrix, meter};
    let stages = [
        ("work buffer (64 frames)", size_of::<[i32; orpheus_dsp::CHUNK * 2]>()),
        ("decimator (HB27 + HB81 history)", size_of::<decimate::Decimator>()),
        ("gain", size_of::<gain::Gain>()),
        ("equaliser, 10 bands", size_of::<eq::Equalizer<10>>()),
        ("bass/treble", size_of::<eq::Tone>()),
        ("crossfeed", size_of::<crossfeed::Crossfeed>()),
        ("channel matrix", size_of::<matrix::Matrix>()),
        ("limiter", size_of::<limiter::Limiter>()),
        ("fade", size_of::<fade::Fade>()),
        ("dither", size_of::<dither::Ditherer>()),
        ("meter", size_of::<meter::Meter>()),
    ];
    let mut sum = 0;
    for (name, n) in stages {
        println!("{name:<34} {n:>5} bytes");
        sum += n;
    }
    let total = orpheus_dsp::chain::ram_bytes();
    println!("{:<34} {total:>5} bytes (stages {sum}, settings and padding {})", "Chain", total - sum);
    // Budget for the whole chain: 4.5 KiB.
    assert!(total <= 4608, "{total}");
}

/// The stage mask kept between blocks (AU-PERF) is always the one
/// `active()` computes, through every kind of parameter change, ramps,
/// fades, the limiter's release and format changes; with nothing moving it
/// is kept (not recomputed per block). `process` also checks it on every
/// block in debug builds.
#[test]
fn settled_stage_mask_follows_every_change() {
    let mut c = Chain::default();
    c.set_format(FMT16).unwrap();
    let x = music16(4096, 7);
    let mut seed = 0x1234_5678u32;
    let mut rnd = move |n: u32| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) % n
    };
    let mut settled_blocks = 0;
    for step in 0..1500 {
        // Values from small sets that include the neutral one, so that
        // stages go on and off (the mask changes) often.
        let pick = |r: u32, v: &[i16]| v[r as usize % v.len()];
        match rnd(16) {
            0 => c.set_eq_preset(if rnd(2) == 0 { Preset::Rock } else { Preset::Flat }),
            1 => c.set_eq_band(
                rnd(5) as usize,
                Band::peaking(EQ5_FREQS[rnd(5) as usize], 100, pick(rnd(9), &[0, 300, -400])),
            ),
            2 => c.set_bass(pick(rnd(9), &[0, 300, -300])),
            3 => c.set_treble(pick(rnd(9), &[0, 200, -500])),
            4 => c.set_crossfeed(if rnd(2) == 0 { None } else { Some(CrossfeedParams::DEFAULT) }),
            5 => c.set_balance(pick(rnd(9), &[0, 40, -70]) as i8),
            6 => c.set_channel_mode(if rnd(4) != 0 { ChannelMode::Stereo } else { ChannelMode::Mono }),
            7 => c.set_limiter(rnd(2) == 0),
            8 => c.fade_to(if rnd(2) == 0 { UNITY } else { UNITY / 4 }, 5 + rnd(40)),
            9 => c.set_eq_preamp(if rnd(2) == 0 {
                EqPreamp::Auto
            } else {
                EqPreamp::Manual(pick(rnd(9), &[0, 600, -300]).into())
            }),
            10 => c.set_meter(if rnd(2) == 0 { MeterMode::Off } else { MeterMode::Peak }),
            11 => {
                let _ = c.set_format(if rnd(2) == 0 { FMT16 } else { Format { channels: 1, ..FMT16 } });
            }
            12 => c.reset(),
            _ => {}
        }
        // A few blocks after each change (ramps run to their end).
        for _ in 0..1 + rnd(6) {
            let ch = usize::from(c.format().channels);
            let at = (rnd(3000) as usize) * 2;
            let mut v: Vec<i16> = x[at..at + 512 * ch].to_vec();
            c.process(&mut PcmBlock::I16(&mut v));
            if let Some(m) = c.settled_mask() {
                assert_eq!(m, c.active(), "step {step}: kept mask out of date");
                settled_blocks += 1;
            }
        }
    }
    assert!(settled_blocks > 100, "the mask is kept once nothing moves ({settled_blocks} blocks)");
    // Flat settings: kept from the first block on; a change clears it.
    let mut c = Chain::default();
    c.set_format(FMT16).unwrap();
    let mut v = music16(256, 1);
    c.process(&mut PcmBlock::I16(&mut v));
    assert_eq!(c.settled_mask(), Some(c.active()));
    c.set_bass(300);
    assert_eq!(c.settled_mask(), None);
    // While the shelf ramps the mask is recomputed; afterwards kept again.
    let mut v = music16(256, 2);
    c.process(&mut PcmBlock::I16(&mut v));
    assert_eq!(c.settled_mask(), None);
    for i in 0..40 {
        let mut v = music16(256, 3 + i);
        c.process(&mut PcmBlock::I16(&mut v));
    }
    assert_eq!(c.settled_mask(), Some(c.active()));
    assert_ne!(c.active() & (1 << StageId::Tone as u16), 0);
}
