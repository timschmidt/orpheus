//! Whole recordings through the session into a RAM "card", decoded again
//! with the project's own decoders: PCM and FLAC bit-exact, IMA ADPCM
//! exactly what the encoder reconstructed; header rewrites, cut files,
//! drops with their dropout cue points.

use orpheus_audio::{ContainerInfo, Decoder, PcmBlock, SliceSource, Step, StreamInfo};
use orpheus_codec_flac::FlacDecoder;
use orpheus_codec_wav::WavDecoder;
use orpheus_rec::adpcm::AdpcmEncoder;
use orpheus_rec::chain::ChainConfig;
use orpheus_rec::encoder::{BUF_SAMPLES, Format, MAX_BLOCK_BYTES, MAX_HEADER, Params};
use orpheus_rec::fake::{FakeAdc, Signal};
use orpheus_rec::session::{Config, Session, State, StopReason};

fn decode<'a, D: Decoder<'a>>(file: &[u8], arena: &'a mut Vec<u8>) -> (StreamInfo, Vec<i32>) {
    let mut src = SliceSource::new(file);
    let hint = ContainerInfo::default();
    let plan = D::inspect(&src, &hint).expect("inspect");
    arena.resize(plan.arena, 0);
    let (mut d, info) = D::open(&mut src, arena, &hint).expect("open");
    let mut out = Vec::new();
    for _ in 0..10_000_000 {
        match d.decode(&mut src) {
            Ok(Step::Pcm(PcmBlock::I16(s))) => out.extend(s.iter().map(|&x| i32::from(x))),
            Ok(Step::Pcm(PcmBlock::I32(s))) => out.extend(s.iter().map(|&x| x >> 16)),
            Ok(Step::NeedData(_)) => panic!("all data was there"),
            Ok(Step::EndOfStream) => return (info, out),
            // A file cut by a power loss ends in the middle of a frame.
            Err(_) if CUT.with(std::cell::Cell::get) => return (info, out),
            Err(e) => panic!("{e:?}"),
        }
    }
    panic!("no end");
}

thread_local! {
    static CUT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn decode_any(format: Format, file: &[u8]) -> (StreamInfo, Vec<i32>) {
    let mut arena = Vec::new();
    match format {
        Format::Flac => decode::<FlacDecoder<'_>>(file, &mut arena),
        _ => decode::<WavDecoder<'_>>(file, &mut arena),
    }
}

/// A RAM card: the file, the ring bytes, and the write loop.
struct Rig {
    s: Session<Vec<i16>, Vec<u8>>,
    ring: Vec<u8>,
    file: Vec<u8>,
    now: u64,
    /// Every sample fed (after the chain).
    fed: Vec<i16>,
}

impl Rig {
    fn new(params: Params, chain: ChainConfig, ring_bytes: usize) -> Rig {
        let cfg = Config {
            params,
            chain,
            limit_ms: None,
            split_ms: None,
            ring_bytes,
            chunk: 8192,
            max_wait_ms: 500,
        };
        let mut r = Rig {
            s: Session::new(cfg, 1 << 30, 0, vec![0; BUF_SAMPLES], vec![0; MAX_BLOCK_BYTES]),
            ring: vec![0; ring_bytes],
            file: Vec::new(),
            now: 0,
            fed: Vec::new(),
        };
        let ring = &mut r.ring;
        r.s.begin(0, &mut |at, b| ring[at..at + b.len()].copy_from_slice(b));
        r
    }

    fn feed(&mut self, block: &mut [i16]) {
        let ring = &mut self.ring;
        self.s.feed(block, self.now, &mut |at, b| ring[at..at + b.len()].copy_from_slice(b));
        self.fed.extend_from_slice(block);
    }

    /// Issue every write that is due; `false` writes fail.
    fn writes(&mut self, ok: bool) {
        while let Some(w) = self.s.next_write(self.now) {
            if ok {
                let at = w.file_at as usize;
                if self.file.len() < at + w.len {
                    self.file.resize(at + w.len, 0);
                }
                self.file[at..at + w.len].copy_from_slice(&self.ring[w.at..w.at + w.len]);
            }
            self.s.written(ok, self.now);
            if !ok {
                break;
            }
        }
    }

    fn rewrite_header(&mut self, h: &[u8]) {
        if self.file.len() < h.len() {
            self.file.resize(h.len(), 0);
        }
        self.file[..h.len()].copy_from_slice(h);
    }

    fn close(&mut self, why: StopReason) {
        let ring = &mut self.ring;
        self.s.finish(why, self.now, &mut |at, b| ring[at..at + b.len()].copy_from_slice(b));
        self.writes(true);
        assert!(self.s.drained());
        let mut h = [0u8; MAX_HEADER];
        let n = self.s.final_header(&mut h);
        self.rewrite_header(&h[..n]);
        assert_eq!(self.s.state(), State::Done);
    }

    /// Record `ms` of `adc` in blocks of `frames`, writing as it goes.
    fn record(&mut self, adc: &mut FakeAdc, ms: u64, frames: usize) {
        let ch = usize::from(adc.channels);
        let blocks = ms * u64::from(adc.rate) / 1000 / frames as u64;
        let mut b = vec![0i16; frames * ch];
        for _ in 0..blocks {
            adc.fill(&mut b);
            self.feed(&mut b);
            self.now += (frames as u64 * 1000) / u64::from(adc.rate);
            self.writes(true);
            if self.s.header_due(self.now) {
                let mut h = [0u8; MAX_HEADER];
                let n = self.s.header(&mut h, self.now);
                self.rewrite_header(&h[..n]);
            }
        }
    }
}

fn params(rate: u32, channels: u8, format: Format, level: u8) -> Params {
    Params { rate, channels, format, level }
}

#[test]
fn pcm_and_flac_are_bit_exact() {
    let signals = [
        Signal::Tone { hz: 440, amp: 12_000 },
        Signal::Noise(30_000),
        Signal::Bursts { hz: 220, amp: 20_000, on_ms: 300, off_ms: 200 },
        Signal::Silence,
    ];
    for (i, sig) in signals.iter().enumerate() {
        for &(rate, ch) in &[(16_000u32, 1u8), (44_100, 2), (48_000, 1), (8_000, 2)] {
            for (format, level) in [(Format::Wav, 0), (Format::Flac, 0), (Format::Flac, 1), (Format::Flac, 2)]
            {
                let mut rig = Rig::new(params(rate, ch, format, level), ChainConfig::default(), 64 * 1024);
                let mut adc = FakeAdc::new(rate, ch, *sig, i as u32 + 1);
                // An odd block size: blocks straddle the encoder's blocks.
                rig.record(&mut adc, 1_300, 333);
                rig.close(StopReason::User);
                let (info, pcm) = decode_any(format, &rig.file);
                assert_eq!((info.rate, info.channels), (rate, ch));
                let want: Vec<i32> = rig.fed.iter().map(|&s| i32::from(s)).collect();
                assert_eq!(pcm.len(), want.len(), "{format:?} L{level} {rate}/{ch} {sig:?}");
                assert!(pcm == want, "{format:?} L{level} {rate}/{ch} {sig:?}: samples differ");
                if format == Format::Flac {
                    assert_eq!(info.frames, Some((want.len() / usize::from(ch)) as u64));
                }
            }
        }
    }
}

#[test]
fn flac_is_smaller_than_pcm() {
    let mut sizes = [0usize; 4];
    for (k, (format, level)) in
        [(Format::Wav, 0), (Format::Flac, 0), (Format::Flac, 1), (Format::Flac, 2)].into_iter().enumerate()
    {
        let mut rig = Rig::new(params(44_100, 2, format, level), ChainConfig::default(), 64 * 1024);
        let mut adc =
            FakeAdc::new(44_100, 2, Signal::Bursts { hz: 330, amp: 16_000, on_ms: 400, off_ms: 300 }, 9);
        rig.record(&mut adc, 3_000, 512);
        rig.close(StopReason::User);
        sizes[k] = rig.file.len();
    }
    assert!(sizes[1] * 10 < sizes[0] * 7, "{sizes:?}");
    assert!(sizes[2] <= sizes[1] && sizes[3] <= sizes[2] + sizes[2] / 100, "{sizes:?}");
}

#[test]
fn adpcm_decodes_to_the_encoders_reconstruction() {
    for &(rate, ch) in &[(8_000u32, 1u8), (16_000, 1), (44_100, 2), (48_000, 2)] {
        let mut rig = Rig::new(params(rate, ch, Format::Adpcm, 0), ChainConfig::default(), 64 * 1024);
        let mut adc = FakeAdc::new(rate, ch, Signal::Tone { hz: 500, amp: 9_000 }, 4);
        rig.record(&mut adc, 2_000, 256);
        rig.close(StopReason::User);
        let (info, pcm) = decode_any(Format::Adpcm, &rig.file);
        assert_eq!((info.rate, info.channels), (rate, ch));
        // The reference: the same encoder over the same input, keeping
        // its reconstruction.
        let mut e = AdpcmEncoder::new(rate, ch);
        let spb = e.frames_per_block() * usize::from(ch);
        let mut recon = Vec::new();
        let mut out = vec![0u8; e.block_align()];
        for blk in rig.fed.chunks(spb) {
            let mut r = vec![0i16; spb];
            e.encode_recon(blk, &mut out, &mut r);
            r.truncate(blk.len());
            recon.extend(r);
        }
        let want: Vec<i32> = recon.iter().map(|&s| i32::from(s)).collect();
        // The `fact` chunk states the length; the decoder plays the last
        // block whole (padded with its last frame).
        let fact = u32::from_le_bytes(rig.file[48..52].try_into().unwrap());
        assert_eq!(fact as usize, want.len() / usize::from(ch), "{rate}/{ch}: fact");
        assert!(pcm.len() >= want.len() && pcm.len() - want.len() < spb, "{rate}/{ch}");
        let pcm = &pcm[..want.len()];
        assert!(pcm == want.as_slice(), "{rate}/{ch}");
        // And it is a fair copy: error well below the signal.
        let err: f64 = pcm.iter().zip(&rig.fed).map(|(&a, &b)| f64::from(a - i32::from(b)).powi(2)).sum();
        let sig: f64 = rig.fed.iter().map(|&b| f64::from(b).powi(2)).sum();
        assert!(sig / err.max(1.0) > 100.0, "SNR {:.1} dB", 10.0 * (sig / err).log10());
    }
}

#[test]
fn a_file_cut_after_a_header_rewrite_still_plays() {
    for format in [Format::Wav, Format::Adpcm, Format::Flac] {
        let mut rig = Rig::new(params(16_000, 1, format, 1), ChainConfig::default(), 32 * 1024);
        let mut adc = FakeAdc::new(16_000, 1, Signal::Tone { hz: 700, amp: 8_000 }, 2);
        rig.record(&mut adc, 12_500, 320);
        // Power lost here: the card holds what was written and the header
        // of the 10-second rewrite.
        let cut = rig.s.status().written as usize;
        let file = rig.file[..cut].to_vec();
        CUT.with(|c| c.set(true));
        let (_, pcm) = decode_any(format, &file);
        CUT.with(|c| c.set(false));
        assert!(pcm.len() >= 16_000 * 9, "{format:?}: {} frames", pcm.len());
        let want: Vec<i32> = rig.fed[..pcm.len()].iter().map(|&s| i32::from(s)).collect();
        if format != Format::Adpcm {
            assert!(pcm == want, "{format:?}");
        }
    }
}

#[test]
fn a_stalled_card_drops_whole_blocks_and_says_so() {
    for format in [Format::Wav, Format::Flac] {
        let mut rig = Rig::new(params(16_000, 1, format, 0), ChainConfig::default(), 16 * 1024);
        // Noise: FLAC cannot squeeze it much.
        let mut adc = FakeAdc::new(16_000, 1, Signal::Noise(8_000), 5);
        let mut b = vec![0i16; 320];
        let mut sent = 0usize;
        // Normal service keeps up: only the stall drops.
        for i in 0..250 {
            adc.fill(&mut b);
            rig.feed(&mut b);
            sent += b.len();
            rig.now += 20;
            // The card answers nothing for 2 s (blocks 50 to 150).
            if !(50..150).contains(&i) {
                rig.writes(true);
            }
        }
        let st = rig.s.status();
        assert!(st.dropped_blocks > 0 && st.dropouts == 1, "{st:?}");
        assert_eq!(st.frames + st.dropped_frames, sent as u64);
        rig.close(StopReason::User);
        let (_, pcm) = decode_any(format, &rig.file);
        assert_eq!(pcm.len() as u64, st.frames, "{format:?}: the file holds what was kept");
        if format == Format::Wav {
            let at = rig.file.windows(4).position(|w| w == b"cue ").expect("cue chunk");
            assert_eq!(u32::from_le_bytes(rig.file[at + 8..at + 12].try_into().unwrap()), 1);
            assert!(rig.file.windows(7).any(|w| w == b"dropout"));
        }
    }
}

#[test]
fn marks_pause_limit_and_card_full() {
    let mut rig = Rig::new(params(16_000, 1, Format::Wav, 0), ChainConfig::default(), 32 * 1024);
    let mut adc = FakeAdc::new(16_000, 1, Signal::Tone { hz: 300, amp: 8_000 }, 5);
    rig.record(&mut adc, 1_000, 320);
    rig.s.mark();
    rig.s.pause();
    rig.record(&mut adc, 1_000, 320);
    assert_eq!(rig.s.elapsed_ms(), 1_000, "paused: the file does not grow");
    rig.s.resume();
    rig.record(&mut adc, 500, 320);
    assert_eq!(rig.s.elapsed_ms(), 1_500);
    rig.close(StopReason::User);
    assert_eq!(rig.s.status().marks, 1);
    let at = rig.file.windows(4).position(|w| w == b"cue ").expect("cue chunk");
    assert_eq!(u32::from_le_bytes(rig.file[at + 32..at + 36].try_into().unwrap()), 16_000, "mark at 1 s");
    // The RIFF size covers the closing chunks.
    let riff = u32::from_le_bytes(rig.file[4..8].try_into().unwrap()) as usize;
    assert_eq!(riff + 8, rig.file.len());

    // Time limit.
    let cfg = Config {
        params: params(8_000, 1, Format::Adpcm, 0),
        chain: ChainConfig::default(),
        limit_ms: Some(2_000),
        split_ms: None,
        ring_bytes: 16 * 1024,
        chunk: 4096,
        max_wait_ms: 500,
    };
    let mut s = Session::new(cfg, 1 << 30, 0, vec![0i16; BUF_SAMPLES], vec![0u8; MAX_BLOCK_BYTES]);
    let mut ring = vec![0u8; 16 * 1024];
    let mut sink = |at: usize, b: &[u8]| ring[at..at + b.len()].copy_from_slice(b);
    s.begin(0, &mut sink);
    let mut adc = FakeAdc::new(8_000, 1, Signal::Noise(1000), 1);
    let mut b = [0i16; 160];
    let mut t = 0;
    while s.due_stop().is_none() {
        adc.fill(&mut b);
        s.feed(&mut b, t, &mut sink);
        while s.next_write(t).is_some() {
            s.written(true, t);
        }
        t += 20;
    }
    assert_eq!(s.due_stop(), Some(StopReason::TimeLimit));
    assert_eq!(s.elapsed_ms(), 2_000);

    // Card full: 1 MiB + a little free.
    let cfg = Config { limit_ms: None, ..cfg };
    let mut s = Session::new(cfg, (1 << 20) + 9_000, 0, vec![0i16; BUF_SAMPLES], vec![0u8; MAX_BLOCK_BYTES]);
    s.begin(0, &mut sink);
    let mut stopped = None;
    for i in 0..1000 {
        adc.fill(&mut b);
        s.feed(&mut b, i * 20, &mut sink);
        while s.next_write(i * 20).is_some() {
            s.written(true, i * 20);
        }
        if let Some(r) = s.due_stop() {
            stopped = Some(r);
            break;
        }
    }
    assert_eq!(stopped, Some(StopReason::CardFull));
}

#[test]
fn write_errors_stop_the_recording() {
    let mut rig = Rig::new(params(16_000, 1, Format::Wav, 0), ChainConfig::default(), 32 * 1024);
    let mut adc = FakeAdc::new(16_000, 1, Signal::Tone { hz: 300, amp: 8_000 }, 5);
    rig.record(&mut adc, 1_000, 320);
    let mut b = vec![0i16; 320];
    for _ in 0..200 {
        adc.fill(&mut b);
        rig.feed(&mut b);
        rig.now += 20;
        rig.writes(false);
        if rig.s.due_stop().is_some() {
            break;
        }
    }
    assert_eq!(rig.s.due_stop(), Some(StopReason::CardError));
}
