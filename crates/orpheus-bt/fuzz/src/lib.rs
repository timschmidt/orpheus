//! Fuzz-target bodies of `orpheus-bt` (VER-4), shared by the cargo-fuzz
//! targets in `fuzz_targets/` (nightly) and the stable replay of the
//! committed corpus (`tests/fuzz_corpus.rs`, every `cargo test`).
//!
//! * [`bt_frame`] — the UART byte stream from the module through the frame
//!   decoder. Oracles: no panic; the frames do not depend on how the stream
//!   is chunked; the statistics count what was emitted; every frame
//!   re-encodes to bytes that decode to exactly that frame, and a run of
//!   re-encoded frames decodes back to the same run; event parsing agrees
//!   with the frame; the volume scales stay in range and round-trip.
//! * [`bt_tx`] — the transmitter state machine fed arbitrary module bytes,
//!   well-formed events, acknowledges, time and every firmware request.
//!   Oracles: no panic; every frame sent is well formed; no restricted
//!   command (CLAUDE.md rule 6) is ever sent except the shutdown (0x24)
//!   after [`Transmitter::shutdown`] was accepted; nothing is sent while the
//!   module is off; the device lists and names stay within their bounds and
//!   every output index names an existing entry; the outputs per step are
//!   bounded.

#![forbid(unsafe_code)]

use orpheus_bt::frame::{self, Decoder, Frame, MAX_RX_PARAMS, MAX_TX_FRAME, SYNC, checksum, is_restricted};
use orpheus_bt::proto::{self, Addr, Event};
use orpheus_bt::transmitter::{MAX_FOUND, MAX_NAME, MAX_PAIRED};
use orpheus_bt::{Config, Output, State, TrackInfo, Transmitter, Transport};

/// A frame's bytes (`AA len_h len_l id params chk`), also for the IDs
/// [`frame::encode`] refuses.
#[must_use]
pub fn raw_frame(id: u8, params: &[u8]) -> Vec<u8> {
    let len = (params.len() + 1) as u16;
    let mut v = vec![SYNC, (len >> 8) as u8, len as u8, id];
    v.extend_from_slice(params);
    v.push(checksum(&v[1..]));
    v
}

fn decode_all(bytes: &[u8], chunk: usize) -> (Vec<Frame>, frame::DecodeStats) {
    let mut d = Decoder::new();
    let mut v = Vec::new();
    for c in bytes.chunks(chunk.max(1)) {
        d.feed(c, |f| v.push(f));
    }
    (v, d.stats)
}

/// The frame decoder (see the crate docs). Byte 0 picks the chunk size.
pub fn bt_frame(data: &[u8]) {
    let Some((&sel, bytes)) = data.split_first() else { return };
    let (whole, stats) = decode_all(bytes, bytes.len());
    let (chunked, cstats) = decode_all(bytes, 1 + usize::from(sel % 17));
    assert_eq!(chunked, whole, "frames depend on the chunking");
    assert_eq!(cstats, stats);
    assert_eq!(stats.frames as usize, whole.len());
    assert!(stats.skipped as usize <= bytes.len());
    assert!(whole.len() <= bytes.len() / 5, "more frames than the bytes can hold");

    let mut cat = Vec::new();
    for f in &whole {
        let p = f.params();
        assert!(p.len() <= MAX_RX_PARAMS);
        let enc = raw_frame(f.id, p);
        let (back, _) = decode_all(&enc, enc.len());
        assert_eq!(back, [*f], "re-encoded frame");
        if !is_restricted(f.id) {
            let mut b = [0u8; MAX_TX_FRAME];
            let n = frame::encode(f.id, p, &mut b).expect("a received frame fits the TX limit");
            assert_eq!(&b[..n], &enc[..]);
        }
        cat.extend_from_slice(&enc);
        check_event(f);
    }
    let (again, _) = decode_all(&cat, 3);
    assert_eq!(again, whole, "a run of re-encoded frames");

    // Volume scales.
    let l = sel % 32;
    assert_eq!(proto::volume_from_sink(proto::volume_to_sink(l)), l);
    assert!(proto::volume_to_sink(sel) <= 127);
    assert!(proto::volume_from_sink(sel) <= 31);
    assert_eq!(proto::volume_from_sink(sel) == 0, sel == 0);
    let ui = sel % 31;
    assert_eq!(proto::ui_from_local(proto::local_from_ui(ui)), ui);
}

fn check_event(f: &Frame) {
    let p = f.params();
    match Event::parse(f) {
        Event::Ack { cmd } => assert_eq!((f.id, cmd), (0, p[0])),
        Event::Link { addr, .. } => assert_eq!(addr.0[..], p[2..8]),
        Event::InquiryResult { name, .. } | Event::RemoteName { name, .. } | Event::Paired { name, .. } => {
            assert!(name.len() < p.len() && !name.contains(&0));
        }
        Event::Identity(s) => assert_eq!(s, p),
        Event::Other { id, params } => assert_eq!((id, params), (f.id, p)),
        _ => {}
    }
}

/// Frames the transmitter sent during one step.
#[derive(Default)]
pub struct Tx {
    pub frames: Vec<Vec<u8>>,
}

impl Transport for Tx {
    fn send(&mut self, f: &[u8]) {
        assert!(f.len() >= 5 && f.len() <= MAX_TX_FRAME, "frame length {}: {f:02X?}", f.len());
        assert_eq!(f[0], SYNC);
        assert_eq!(usize::from(u16::from_be_bytes([f[1], f[2]])), f.len() - 4, "{f:02X?}");
        assert_eq!(checksum(&f[1..]), 0, "{f:02X?}");
        self.frames.push(f.to_vec());
    }
}

/// Byte reader over the fuzz input.
struct Ops<'a>(&'a [u8]);

impl Ops<'_> {
    fn byte(&mut self) -> Option<u8> {
        let (&b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(b)
    }
    fn b(&mut self) -> u8 {
        self.byte().unwrap_or(0)
    }
    fn take(&mut self, n: usize) -> &[u8] {
        let n = n.min(self.0.len());
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        a
    }
}

/// Event IDs the module sends.
pub const EVENTS: [u8; 14] =
    [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x23, 0x24, 0x25, 0x26, 0x29, 0x2A, 0x2C];

fn addr_of(t: &Transmitter, k: u8) -> Addr {
    let i = usize::from(k >> 2);
    match k & 3 {
        0 => t.found().get(i).map(|d| d.addr),
        1 => t.paired().get(i).map(|d| d.addr),
        _ => None,
    }
    .unwrap_or(Addr([k, 1, 2, 3, 4, k ^ 0x55]))
}

/// The transmitter state machine (see the crate docs). The input is a
/// sequence of operations; byte 0 configures it.
pub fn bt_tx(data: &[u8]) {
    let mut ops = Ops(data);
    let c = ops.b();
    let cfg = Config {
        auto_reconnect: c & 1 != 0,
        reconnect_attempts: (c >> 1) & 3,
        forget_all_on_start: c & 8 != 0,
        max_restarts: (c >> 4) & 3,
        ..Config::default()
    };
    let mut t = Transmitter::new(cfg);
    if c & 0x40 != 0 {
        t.set_last_device(Some(Addr([1, 2, 3, 4, 5, 6])));
    }
    let mut now = 0u64;
    let mut shutdown_asked = false;
    let mut steps = 0;
    while let Some(op) = ops.byte() {
        steps += 1;
        if steps > 400 {
            break;
        }
        let mut tx = Tx::default();
        let was = t.state();
        let dropped = t.outputs_dropped;
        let vol_before = t.volume();
        let mut powered_on = false;
        match op % 10 {
            0 => {
                let n = usize::from(ops.b() % 48);
                let b = ops.take(n).to_vec();
                t.on_bytes(&b, now, &mut tx);
            }
            1 | 2 => {
                let id = EVENTS[usize::from(ops.b()) % EVENTS.len()];
                let n = usize::from(ops.b()) % (MAX_RX_PARAMS + 1);
                let p = ops.take(n).to_vec();
                t.on_bytes(&raw_frame(id, &p), now, &mut tx);
            }
            3 => {
                // Acknowledge what the module was last sent.
                if let Some(&id) = t.last_frame().get(3) {
                    t.on_bytes(&raw_frame(0, &[id]), now, &mut tx);
                }
            }
            4 => {
                now += u64::from(ops.b()) * 37 + 1;
                t.tick(now, &mut tx);
            }
            5 => {
                now += u64::from(ops.b()) * 600 + 1;
                t.tick(now, &mut tx);
            }
            6 => {
                if ops.b() & 1 == 0 {
                    t.power_on(now, &mut tx);
                    powered_on = true;
                } else {
                    t.power_off();
                }
            }
            _ => {
                let k = ops.b();
                let a = ops.b();
                let _ = match k % 16 {
                    0 => t.start_inquiry(now, &mut tx),
                    1 => t.stop_inquiry(now, &mut tx),
                    2 => t.resolve_names(now, &mut tx),
                    3 => t.connect(addr_of(&t, a), now, &mut tx),
                    4 => t.cancel_connect(now, &mut tx),
                    5 => t.disconnect(now, &mut tx),
                    6 => t.forget(if a & 1 == 0 { None } else { Some(addr_of(&t, a >> 1)) }, now, &mut tx),
                    7 => t.refresh_paired(now, &mut tx),
                    8 => {
                        t.set_audio_active(a & 1 != 0, now, &mut tx);
                        Ok(())
                    }
                    9 => t.set_volume(a, now, &mut tx),
                    10 => t.query_volume(now, &mut tx),
                    11 => {
                        let s = "Ünïcødé title that is longer than the field allows for a sink display";
                        let cut = s.char_indices().nth(usize::from(a) % 60).map_or(s.len(), |(i, _)| i);
                        let info = TrackInfo { length_s: u16::from(a) * 7, fields: [&s[..cut], "", s, "x"] };
                        t.send_metadata(&info, now, &mut tx)
                    }
                    12 => {
                        let r = t.shutdown(now, &mut tx);
                        shutdown_asked |= r.is_ok();
                        r
                    }
                    13 => {
                        t.set_last_device(Some(addr_of(&t, a)));
                        Ok(())
                    }
                    _ => {
                        t.set_config(Config { auto_reconnect: a & 1 != 0, ..cfg });
                        Ok(())
                    }
                };
            }
        }
        // Frames: restricted IDs only as the shutdown, and only after it
        // was asked for; nothing while the module stays off.
        for f in &tx.frames {
            let id = f[3];
            if is_restricted(id) {
                assert!(id == 0x24 && shutdown_asked, "restricted command {id:#04x} sent: {f:02X?}");
            }
        }
        if was == State::Off && t.state() == State::Off && !powered_on {
            assert!(tx.frames.is_empty(), "sent while off: {:02X?}", tx.frames);
        }
        // Lists and outputs.
        assert!(t.paired().len() <= MAX_PAIRED);
        assert!(t.found().len() <= MAX_FOUND);
        for d in t.paired().iter().chain(t.found()) {
            assert!(d.name().len() <= MAX_NAME);
        }
        assert!(t.identity().len() <= 8);
        if let Some(v) = t.volume() {
            assert!(v <= 31);
        }
        let mut n = 0;
        let mut last_state = None;
        while let Some(o) = t.next_output() {
            n += 1;
            assert!(n <= 64, "outputs do not drain");
            match o {
                Output::Found(i) | Output::NameResolved(i) => {
                    assert!(usize::from(i) < t.found().len(), "{o:?} with {} found", t.found().len());
                }
                Output::SinkVolume { raw, local } => {
                    assert!(local <= 31);
                    if raw > 127 && matches!(op % 10, 1 | 2) {
                        assert_eq!(
                            t.volume(),
                            vol_before,
                            "a \"no absolute volume\" report changed the volume"
                        );
                    }
                    assert_eq!(local, proto::volume_from_sink(raw));
                }
                Output::State(s) => last_state = Some(s),
                _ => {}
            }
        }
        if let Some(s) = last_state
            && t.outputs_dropped == dropped
        {
            assert_eq!(s, t.state(), "the last state output is not the state");
        }
    }
}
