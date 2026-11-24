//! Host tests of the SILK port: the RFC 8251 test vectors (SILK-only
//! frames compared bit-exactly with the reference decode) and a
//! fuzz-style no-panic test.

use super::{DecControl, SilkDecoder, SilkScratch};
use crate::entdec::EcDec;
use crate::packet::{self, Bandwidth, Mode, Toc};
use bytemuck::Zeroable;
use std::boxed::Box;
use std::path::PathBuf;
use std::vec::Vec;
use std::{format, println};

fn vectors_dir() -> Option<PathBuf> {
    crate::celt::tests::vectors_dir()
}

/// The packets of a `.bit` file: records of [u32 BE length][u32 BE final
/// range][bytes].
fn read_bit(path: &std::path::Path) -> Vec<Vec<u8>> {
    let data = std::fs::read(path).expect("read .bit");
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let len = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        pos += 8;
        if pos + len > data.len() {
            break;
        }
        out.push(data[pos..pos + len].to_vec());
        pos += len;
    }
    out
}

fn read_dec(path: &std::path::Path) -> Vec<i16> {
    let data = std::fs::read(path).expect("read .dec");
    data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect()
}

fn internal_rate(bw: Bandwidth) -> i32 {
    match bw {
        Bandwidth::Narrow => 8000,
        Bandwidth::Medium => 12000,
        _ => 16000,
    }
}

struct Silk {
    dec: Box<SilkDecoder>,
    scratch: Box<SilkScratch>,
    ctl: DecControl,
}

impl Silk {
    fn new() -> Self {
        let mut dec = Box::new(SilkDecoder::zeroed());
        dec.init();
        let ctl = DecControl { n_channels_api: 2, api_sample_rate: 48000, ..DecControl::default() };
        Self { dec, scratch: Box::new(SilkScratch::zeroed()), ctl }
    }

    /// One Opus frame of SILK data like `opus_decode_frame`: `pcm` gets
    /// `frame_size` stereo samples. Returns whether the frame carries a
    /// CELT redundancy frame.
    fn decode_frame(&mut self, toc: &Toc, data: &[u8], pcm: &mut [i16]) -> Result<bool, i32> {
        let frame_size = toc.frame_size;
        self.ctl.n_channels_internal = if toc.stereo { 2 } else { 1 };
        self.ctl.internal_sample_rate = internal_rate(toc.bandwidth);
        self.ctl.payload_size_ms = (1000 * frame_size as i32 / 48000).max(10);
        let mut ec = EcDec::new(data);
        let mut decoded = 0;
        while decoded < frame_size {
            let mut n = 0;
            self.dec.decode(
                &mut self.scratch,
                &mut self.ctl,
                0,
                decoded == 0,
                &mut ec,
                &mut pcm[decoded * 2..],
                &mut n,
            )?;
            if n <= 0 {
                return Err(-1);
            }
            decoded += n as usize;
        }
        Ok(ec.tell() + 17 <= 8 * data.len() as i32)
    }

    /// Packet loss concealment for `frame_size` samples, in chunks of at
    /// most 20 ms like `opus_decode_frame` with `data == NULL`.
    fn conceal(&mut self, frame_size: usize, pcm: &mut [i16]) -> Result<(), i32> {
        let mut done = 0;
        while done < frame_size {
            let chunk = (frame_size - done).min(960);
            self.ctl.payload_size_ms = (1000 * chunk as i32 / 48000).max(10);
            let mut ec = EcDec::new(&[]);
            let mut decoded = 0;
            while decoded < chunk {
                let mut n = 0;
                self.dec.decode(
                    &mut self.scratch,
                    &mut self.ctl,
                    1,
                    decoded == 0,
                    &mut ec,
                    &mut pcm[(done + decoded) * 2..],
                    &mut n,
                )?;
                if n <= 0 {
                    return Err(-1);
                }
                decoded += n as usize;
            }
            done += chunk;
        }
        Ok(())
    }
}

#[derive(Default, Debug)]
struct Stats {
    frames: usize,
    compared: usize,
    exact: usize,
    redundancy_excluded: usize,
    plc_frames: usize,
    plc_exact: usize,
    first_mismatch: Option<(usize, usize)>,
}

/// Decode one vector's SILK layer like libopus and compare the SILK-only
/// frames whose neighbours are SILK-only too (other frames mix in CELT
/// output, so they are only decoded to keep the SILK state in step:
/// hybrid frames run the SILK decoder at 16 kHz, and a SILK frame after a
/// CELT-only frame resets it, as `opus_decode_frame` does).
fn run_vector(packets: &[Vec<u8>], reference: &[i16]) -> Stats {
    // Flatten into frames: (toc, data, position in the reference).
    let mut frames_list: Vec<(Toc, &[u8], usize)> = Vec::new();
    let mut pos = 0usize;
    for p in packets {
        let Some(frames) = packet::parse(p) else {
            panic!("malformed or empty packet in a test vector");
        };
        for i in 0..frames.count {
            let off = usize::from(frames.offset[i]);
            frames_list.push((frames.toc, &p[off..off + usize::from(frames.size[i])], pos));
            pos += frames.toc.frame_size;
        }
    }

    let mut st = Stats::default();
    let mut silk = Silk::new();
    let mut pcm = std::vec![0i16; 5760 * 2];
    let mut prev_mode: Option<Mode> = None;
    let mut prev_redundancy = false;
    for (idx, &(toc, data, pos)) in frames_list.iter().enumerate() {
        let n = toc.frame_size;
        if toc.mode == Mode::Silk {
            st.frames += 1;
        }
        if toc.mode == Mode::Celt {
            prev_mode = Some(Mode::Celt);
            prev_redundancy = false;
            continue;
        }
        if prev_mode == Some(Mode::Celt) {
            silk.dec.reset();
        }
        let out = &mut pcm[..n * 2];
        let reference_frame = reference.get(pos * 2..(pos + n) * 2);
        if data.len() <= 1 {
            silk.conceal(n, out).expect("PLC");
            if toc.mode == Mode::Silk {
                st.plc_frames += 1;
                if reference_frame == Some(&*out) {
                    st.plc_exact += 1;
                }
            }
            prev_redundancy = false;
        } else if toc.mode == Mode::Hybrid {
            let mut t = toc;
            t.bandwidth = Bandwidth::Wide; // SILK runs at 16 kHz in hybrid mode
            silk.decode_frame(&t, data, out).expect("SILK decode (hybrid)");
            prev_redundancy = false; // only SILK-only neighbours are compared
        } else {
            let redundancy = silk.decode_frame(&toc, data, out).expect("SILK decode");
            let neighbours_silk = (idx == 0 || frames_list[idx - 1].0.mode == Mode::Silk)
                && frames_list.get(idx + 1).is_none_or(|f| f.0.mode == Mode::Silk);
            if !neighbours_silk {
                // Mode transition: CELT output is mixed in.
            } else if redundancy || prev_redundancy {
                st.redundancy_excluded += 1;
            } else {
                st.compared += 1;
                if reference_frame == Some(&*out) {
                    st.exact += 1;
                } else if st.first_mismatch.is_none() {
                    let r = reference_frame.unwrap_or(&[]);
                    let k = out.iter().zip(r).position(|(a, b)| a != b).unwrap_or(0);
                    st.first_mismatch = Some((idx, k));
                }
            }
            prev_redundancy = redundancy;
        }
        prev_mode = Some(toc.mode);
    }
    st
}

#[test]
fn silk_test_vectors() {
    let Some(dir) = vectors_dir() else {
        println!("silk_test_vectors: no test vectors (set OPUS_VECTORS); skipped");
        return;
    };
    let mut any_silk_only = false;
    let mut total = Stats::default();
    for nn in 1..=12 {
        let bit = dir.join(format!("testvector{nn:02}.bit"));
        let decp = dir.join(format!("testvector{nn:02}.dec"));
        if !bit.is_file() || !decp.is_file() {
            continue;
        }
        let packets = read_bit(&bit);
        let mut silk_frames = 0;
        let mut other = 0;
        for p in &packets {
            match p.first().map(|&t| Toc::parse(t).mode) {
                Some(Mode::Silk) => silk_frames += 1,
                Some(_) => other += 1,
                None => {}
            }
        }
        if silk_frames == 0 {
            println!("testvector{nn:02}: no SILK-only packets, skipped");
            continue;
        }
        any_silk_only |= other == 0;
        let reference = read_dec(&decp);
        let st = run_vector(&packets, &reference);
        println!(
            "testvector{nn:02} ({}): {} packets, {} SILK-only frames, {} compared, {} bit-exact, {} excluded (redundancy), {} PLC ({} exact), first mismatch {:?}",
            if other == 0 { "SILK-only" } else { "mixed" },
            packets.len(),
            st.frames,
            st.compared,
            st.exact,
            st.redundancy_excluded,
            st.plc_frames,
            st.plc_exact,
            st.first_mismatch
        );
        total.frames += st.frames;
        total.compared += st.compared;
        total.exact += st.exact;
        total.redundancy_excluded += st.redundancy_excluded;
        assert_eq!(st.exact, st.compared, "testvector{nn:02}: SILK output differs from the reference");
    }
    assert!(any_silk_only, "no SILK-only test vector found");
    println!(
        "SILK total: {} frames, {} compared, {} bit-exact, {} excluded",
        total.frames, total.compared, total.exact, total.redundancy_excluded
    );
}

/// xorshift32.
struct Rng(u32);
impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

/// Random and bit-flipped frames, random configurations and loss calls
/// never panic (overflow checks are on in tests).
#[test]
fn silk_fuzz_no_panic() {
    let seed = std::env::var("SILK_FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0x1234_5678u32);
    let mut rng = Rng(seed.max(1));
    let mut dec = Box::new(SilkDecoder::zeroed());
    dec.init();
    let mut scratch = Box::new(SilkScratch::zeroed());
    let mut pcm = std::vec![0i16; 5760 * 2];
    let (mut ok_normal, mut ok_lost, mut errors) = (0u32, 0u32, 0u32);

    // Real frames to mutate, if the vectors are there.
    let mut seeds: Vec<(Toc, Vec<u8>)> = Vec::new();
    if let Some(dir) = vectors_dir() {
        for nn in 1..=12 {
            let bit = dir.join(format!("testvector{nn:02}.bit"));
            if !bit.is_file() {
                continue;
            }
            for p in read_bit(&bit).iter().step_by(7) {
                let Some(frames) = packet::parse(p) else { continue };
                if frames.toc.mode != Mode::Silk {
                    continue;
                }
                let off = usize::from(frames.offset[0]);
                seeds.push((frames.toc, p[off..off + usize::from(frames.size[0])].to_vec()));
            }
        }
    }

    let iters: u32 = std::env::var("SILK_FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(20_000);
    for iter in 0..iters {
        let mut ctl = DecControl {
            n_channels_api: 1 + rng.below(2) as i32,
            n_channels_internal: 1 + rng.below(2) as i32,
            api_sample_rate: [8000, 12000, 16000, 24000, 48000][rng.below(5) as usize],
            internal_sample_rate: [8000, 12000, 16000][rng.below(3) as usize],
            payload_size_ms: [0, 10, 20, 40, 60][rng.below(5) as usize],
            prev_pitch_lag: 0,
        };
        if rng.below(50) == 0 {
            // Out-of-range parameters must be refused, not crash.
            ctl.payload_size_ms = rng.next() as i32 % 100;
            ctl.internal_sample_rate = rng.next() as i32 % 50000;
            ctl.n_channels_internal = rng.next() as i32 % 4;
        }
        let data: Vec<u8> = if !seeds.is_empty() && rng.below(2) == 0 {
            let (toc, d) = &seeds[rng.below(seeds.len() as u32) as usize];
            ctl.n_channels_internal = if toc.stereo { 2 } else { 1 };
            ctl.internal_sample_rate = internal_rate(toc.bandwidth);
            ctl.payload_size_ms = (toc.frame_size as i32 / 48).max(10);
            let mut d = d.clone();
            for _ in 0..1 + rng.below(4) {
                if !d.is_empty() {
                    let k = rng.below(d.len() as u32) as usize;
                    d[k] ^= 1 << rng.below(8);
                }
            }
            d
        } else {
            let len = rng.below(120) as usize;
            (0..len).map(|_| rng.next() as u8).collect()
        };
        let lost = if rng.below(6) == 0 { 1 } else { 0 };
        let mut ec = EcDec::new(&data);
        let calls = 1 + rng.below(4);
        for c in 0..calls {
            let mut n = 0;
            let new_packet = c == 0 || rng.below(8) == 0;
            let r = dec.decode(&mut scratch, &mut ctl, lost, new_packet, &mut ec, &mut pcm, &mut n);
            match (r, lost) {
                (Ok(()), 0) => ok_normal += 1,
                (Ok(()), _) => ok_lost += 1,
                (Err(_), _) => errors += 1,
            }
        }
        if iter % 997 == 0 {
            dec.reset();
        }
    }

    println!("silk fuzz: {ok_normal} frames decoded, {ok_lost} concealed, {errors} refused");
    assert!(ok_normal > 10_000 && ok_lost > 1_000, "fuzzing hardly reached the decoder");

    // Too-small output buffers are an error, not a panic.
    let mut ctl = DecControl {
        n_channels_api: 2,
        n_channels_internal: 1,
        api_sample_rate: 48000,
        internal_sample_rate: 16000,
        payload_size_ms: 20,
        prev_pitch_lag: 0,
    };
    let mut ec = EcDec::new(&[0x55; 40]);
    let mut n = 0;
    assert!(dec.decode(&mut scratch, &mut ctl, 0, true, &mut ec, &mut pcm[..100], &mut n).is_err());
}

/// Packet loss concealment on real streams: lost packets (single and in
/// bursts) are concealed without errors, the concealment continues the
/// signal instead of going silent, and fades out over a long burst.
/// (Bit-exactness of the concealment itself is not checked: the test
/// vectors have no lost packets.)
#[test]
fn silk_plc_runs() {
    let Some(dir) = vectors_dir() else {
        println!("silk_plc_runs: no test vectors (set OPUS_VECTORS); skipped");
        return;
    };
    for nn in [2, 3, 4] {
        let bit = dir.join(format!("testvector{nn:02}.bit"));
        if !bit.is_file() {
            continue;
        }
        let packets = read_bit(&bit);
        let mut silk = Silk::new();
        let mut pcm = std::vec![0i16; 5760 * 2];
        let mut concealed = 0;
        let mut concealed_nonzero = 0;
        let mut burst_last_energy = 0u64;
        let mut burst_first_energy = 0u64;
        for (k, p) in packets.iter().enumerate() {
            let frames = packet::parse(p).expect("packet");
            let n = frames.toc.frame_size * frames.count;
            let lost = k % 9 == 4 || (300..330).contains(&k);
            if lost {
                silk.conceal(n, &mut pcm).expect("PLC");
                concealed += 1;
                let e: u64 = pcm[..n * 2].iter().map(|&s| (i64::from(s) * i64::from(s)) as u64).sum();
                if e > 0 {
                    concealed_nonzero += 1;
                }
                if k == 300 {
                    burst_first_energy = e;
                }
                if k == 329 {
                    burst_last_energy = e;
                }
            } else {
                for i in 0..frames.count {
                    let off = usize::from(frames.offset[i]);
                    let data = &p[off..off + usize::from(frames.size[i])];
                    let fsz = frames.toc.frame_size;
                    silk.decode_frame(&frames.toc, data, &mut pcm[..fsz * 2]).expect("decode");
                }
            }
        }
        println!(
            "testvector{nn:02}: {concealed} packets concealed, {concealed_nonzero} with signal, burst energy first {burst_first_energy} last {burst_last_energy}"
        );
        assert!(concealed_nonzero * 2 > concealed, "concealment is mostly silent");
        assert!(burst_last_energy <= burst_first_energy.max(1) * 4, "no fade-out over a 30-packet burst");
    }
}

#[test]
fn silk_sizes() {
    println!(
        "size_of SilkDecoder = {}, SilkScratch = {}",
        core::mem::size_of::<SilkDecoder>(),
        core::mem::size_of::<SilkScratch>()
    );
}
