//! The RFC 8251 Opus test vectors through the full decoder (stereo and
//! mono output): the final range of every packet must equal the encoder's,
//! the output is compared with the reference decodes (`.dec`, `m.dec`).
//! Skipped when the vectors are absent (`OPUS_VECTORS`).

use std::vec::Vec;

use bytemuck::Zeroable;

use crate::celt::tests::{read_bit, vectors_dir};
use crate::celt::{CeltState, MEM_PER_CH};
use crate::opus_dec::{OpusDec, OpusState, REGION_WORDS_PER_CH, scratch_words};
use crate::packet::parse;
use crate::silk::SilkDecoder;

pub struct Owned {
    st: OpusState,
    celt: CeltState,
    mem: Vec<f32>,
    silk: std::boxed::Box<SilkDecoder>,
    scratch: Vec<u64>,
    region: Vec<u32>,
    tr: Vec<f32>,
    red: Vec<f32>,
}

impl Owned {
    pub fn new(ch: usize) -> Self {
        Self {
            st: OpusState::zeroed(),
            celt: CeltState::zeroed(),
            mem: std::vec![0.0; ch * MEM_PER_CH],
            silk: std::boxed::Box::new(SilkDecoder::zeroed()),
            scratch: std::vec![0; scratch_words()],
            region: std::vec![0; REGION_WORDS_PER_CH * ch],
            tr: std::vec![0.0; 240 * ch],
            red: std::vec![0.0; 240 * ch],
        }
    }
    pub fn dec(&mut self) -> OpusDec<'_> {
        OpusDec {
            st: &mut self.st,
            celt: &mut self.celt,
            celt_mem: &mut self.mem,
            silk: &mut self.silk,
            scratch: &mut self.scratch,
            region: &mut self.region,
            transition: &mut self.tr,
            redundant: &mut self.red,
        }
    }
}

/// Decode a `.bit` file; returns (output, packets with a range mismatch).
pub fn decode_vector(bit: &[u8], ch: usize, disable_inv: bool) -> (Vec<i16>, usize, usize) {
    let mut o = Owned::new(ch);
    let mut d = o.dec();
    d.init(ch, 0);
    d.celt.disable_inv = i32::from(disable_inv || ch == 1);
    let mut out = Vec::new();
    let (mut bad, mut total) = (0, 0);
    for (rng, p) in read_bit(bit) {
        total += 1;
        let Some(f) = parse(&p) else {
            bad += 1;
            continue;
        };
        for i in 0..f.count {
            let n = d.decode_packet_frame(&p, &f, i);
            out.extend_from_slice(d.output(n));
        }
        if d.st.range_final != rng {
            bad += 1;
        }
    }
    (out, bad, total)
}

pub fn rms_db(a: &[i16], b: &[i16]) -> (f64, i32) {
    let n = a.len().min(b.len());
    let mut sum = 0f64;
    let mut maxd = 0;
    for k in 0..n {
        let d = i32::from(a[k]) - i32::from(b[k]);
        sum += f64::from(d * d);
        maxd = maxd.max(d.abs());
    }
    let rms = (sum / n.max(1) as f64).sqrt();
    (if rms == 0.0 { f64::NEG_INFINITY } else { 20.0 * (rms / 32768.0).log10() }, maxd)
}

#[test]
fn rfc8251_vectors() {
    let Some(dir) = vectors_dir() else {
        std::println!("opus test vectors not found: skipped");
        return;
    };
    let mut failures = Vec::new();
    for v in 1..=12 {
        let bit = std::fs::read(dir.join(std::format!("testvector{v:02}.bit"))).unwrap();
        for (ch, suffix) in [(2usize, ""), (1, "m")] {
            let refb = std::fs::read(dir.join(std::format!("testvector{v:02}{suffix}.dec"))).unwrap();
            let reference: Vec<i16> =
                refb.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
            // `m.dec` is the stereo decode with intensity phase inversion
            // disabled (RFC 8251 §10, for mono down-mixing players).
            let (out, bad, total) = decode_vector(&bit, 2, ch == 1);
            let (db, maxd) = rms_db(&out, &reference);
            std::println!(
                "vector {v:02}{suffix:1}: {total} packets, range mismatches {bad}; {} vs {} samples; rms {db:.1} dBFS, max {maxd} LSB",
                out.len(),
                reference.len()
            );
            if bad != 0 || out.len() != reference.len() || db > -80.0 {
                failures.push(std::format!("{v:02}{suffix}"));
            }
        }
    }
    assert!(failures.is_empty(), "failed: {failures:?}");
}
