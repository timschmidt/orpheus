//! CELT-only RFC 8251 test vectors (01, 07, 11) through the CELT decoder
//! alone: final range of every frame and the output against the
//! reference decode. Skipped when the vectors are not present.

use std::vec::Vec;

use super::{CeltState, MEM_PER_CH, SCRATCH_LEN, decode};
use crate::packet::{Bandwidth, parse};
use crate::pcm::{float2int16, soft_clip};

/// The RFC 8251 vectors: `$OPUS_VECTORS`, else `tests/vectors/opus_newvectors`
/// in the workspace (fetched by `cargo xtask test codecs --gen`).
pub fn vectors_dir() -> Option<std::path::PathBuf> {
    let d = std::env::var_os("OPUS_VECTORS").map_or_else(
        || std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/vectors/opus_newvectors"),
        std::path::PathBuf::from,
    );
    d.join("testvector01.bit").exists().then_some(d)
}

/// `(final range, packet)` records of a `.bit` file.
pub fn read_bit(b: &[u8]) -> Vec<(u32, Vec<u8>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 8 <= b.len() {
        let l = u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
        let r = u32::from_be_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]);
        out.push((r, b[i + 8..i + 8 + l].to_vec()));
        i += 8 + l;
    }
    out
}

#[test]
fn celt_only_vectors() {
    let Some(dir) = vectors_dir() else {
        std::println!("opus test vectors not found: skipped");
        return;
    };
    for v in ["01", "07", "11"] {
        let bit = std::fs::read(dir.join(std::format!("testvector{v}.bit"))).unwrap();
        let dec = std::fs::read(dir.join(std::format!("testvector{v}.dec"))).unwrap();
        let reference: Vec<i16> = dec.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        let mut st = <CeltState as bytemuck::Zeroable>::zeroed();
        let mut mem = std::vec![0f32; 2 * MEM_PER_CH];
        let mut scratch = std::vec![0f32; SCRATCH_LEN];
        st.init(&mut mem, 2);
        let mut out: Vec<i16> = Vec::new();
        let mut softclip = [0f32; 2];
        let (mut rng_ok, mut rng_bad) = (0, 0);
        for (rng, p) in read_bit(&bit) {
            let Some(f) = parse(&p) else { panic!("bad packet") };
            st.end = match f.toc.bandwidth {
                Bandwidth::Narrow => 13,
                Bandwidth::Medium | Bandwidth::Wide => 17,
                Bandwidth::SuperWide => 19,
                Bandwidth::Full => 21,
            };
            st.stream_channels = if f.toc.stereo { 2 } else { 1 };
            let mut pkt = Vec::new();
            let mut last_rng = 0;
            for i in 0..f.count {
                let o = usize::from(f.offset[i]);
                let d = &p[o..o + usize::from(f.size[i])];
                let mut pcm = std::vec![0f32; 2 * f.toc.frame_size];
                decode(&mut st, &mut mem, Some(d), &mut pcm, f.toc.frame_size, None, &mut scratch).unwrap();
                last_rng = st.rng;
                pkt.extend_from_slice(&pcm);
            }
            if last_rng == rng {
                rng_ok += 1;
            } else {
                rng_bad += 1;
            }
            let n = pkt.len() / 2;
            soft_clip(&mut pkt, n, 2, &mut softclip);
            out.extend(pkt.iter().map(|&x| float2int16(x)));
        }
        let n = out.len().min(reference.len());
        let mut sum = 0f64;
        let mut maxd = 0;
        for k in 0..n {
            let d = i32::from(out[k]) - i32::from(reference[k]);
            sum += f64::from(d * d);
            maxd = maxd.max(d.abs());
        }
        let rms = (sum / n as f64).sqrt();
        std::println!(
            "vector {v}: range {rng_ok} ok {rng_bad} bad; {} vs {} samples; rms {:.3} LSB ({:.1} dBFS), max {maxd}",
            out.len(),
            reference.len(),
            rms,
            20.0 * (rms / 32768.0).log10()
        );
        assert_eq!(rng_bad, 0, "vector {v}");
    }
}
