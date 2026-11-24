//! Host benchmark of the chain: `cargo bench -p yoton-dsp`.
//!
//! Prints nanoseconds per output frame on the host next to the cost model's
//! Cortex-M4 cycles per frame. Host numbers only rank configurations against
//! each other (different core, different instruction set); the device
//! benchmark gives the real figures.

use std::hint::black_box;
use std::time::Instant;

use yoton_audio::{PcmBlock, SampleKind};
use yoton_dsp::{
    Band, Chain, ChannelMode, CrossfeedParams, DitherMode, Format, MeterMode, OutWidth, Preset, RgMode,
    RgSettings, RgTags,
};

const SECONDS: usize = 20;

fn fmt(rate: u32, kind: SampleKind, bits: u8) -> Format {
    Format { rate, channels: 2, kind, bits }
}

fn bench(name: &str, f: Format, setup: impl Fn(&mut Chain)) {
    let mut c = Chain::new(OutWidth::Bits16, false);
    c.set_format(f).unwrap();
    setup(&mut c);
    c.reset();
    let model = c.cost().total;
    let block = 1152 * 2;
    let blocks = SECONDS * f.rate as usize * 2 / block;
    let mut out_frames = 0usize;
    let t = Instant::now();
    match f.kind {
        SampleKind::I16 => {
            let src: Vec<i16> = (0..block).map(|i| ((i * 7919) % 30000) as i16 - 15000).collect();
            let mut buf = src.clone();
            for _ in 0..blocks {
                buf.copy_from_slice(&src);
                out_frames += c.process(&mut PcmBlock::I16(black_box(&mut buf))) / 2;
            }
        }
        SampleKind::I32 => {
            let src: Vec<i32> = (0..block).map(|i| (((i * 7919) % 30000) as i32 - 15000) << 16).collect();
            let mut buf = src.clone();
            for _ in 0..blocks {
                buf.copy_from_slice(&src);
                out_frames += c.process(&mut PcmBlock::I32(black_box(&mut buf))) / 2;
            }
        }
    }
    let ns = t.elapsed().as_nanos() as f64 / out_frames as f64;
    println!("{name:<44} {ns:>7.2} ns/frame (host)   model {model:>5} M4 cycles/frame");
}

fn main() {
    let f16 = fmt(44100, SampleKind::I16, 16);
    bench("flat 16/44.1 (pass-through)", f16, |_| {});
    bench("ReplayGain −7 dB", f16, |c| {
        c.set_replaygain(RgSettings { mode: RgMode::Track, ..RgSettings::default() });
        c.set_track_gains(RgTags { track_gain_mb: Some(-700), ..RgTags::default() });
    });
    bench("EQ 5 bands (Rock)", f16, |c| c.set_eq_preset(Preset::Rock));
    bench("EQ 10 bands", f16, |c| c.set_eq(&[Band::peaking(1000, 70, 300); 10]));
    bench("bass + treble", f16, |c| {
        c.set_bass(600);
        c.set_treble(-300);
    });
    bench("crossfeed", f16, |c| c.set_crossfeed(Some(CrossfeedParams::DEFAULT)));
    bench("balance + mono", f16, |c| {
        c.set_balance(20);
        c.set_channel_mode(ChannelMode::Mono);
    });
    bench("shaped dither (gain −1 dB)", f16, |c| {
        c.set_dither(DitherMode::Shaped);
        c.set_replaygain(RgSettings { mode: RgMode::Track, ..RgSettings::default() });
        c.set_track_gains(RgTags { track_gain_mb: Some(-100), ..RgTags::default() });
    });
    bench("meters only (peak + RMS)", f16, |c| c.set_meter(MeterMode::PeakRms));
    bench("24/48 → 16 bits, TPDF", fmt(48000, SampleKind::I32, 24), |_| {});
    bench("24/96 → 48 kHz", fmt(96000, SampleKind::I32, 24), |_| {});
    bench("24/192 → 48 kHz", fmt(192_000, SampleKind::I32, 24), |_| {});
    bench("everything", f16, |c| {
        c.set_eq(&[Band::peaking(1000, 70, 300); 10]);
        c.set_bass(300);
        c.set_treble(300);
        c.set_crossfeed(Some(CrossfeedParams::DEFAULT));
        c.set_balance(10);
        c.set_meter(MeterMode::PeakRms);
    });
}
