//! The fuzz targets' committed corpora and regression inputs, replayed on
//! stable through the same bodies the cargo-fuzz targets run (VER-4); and
//! the seed inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-bt --test
//! fuzz_corpus seeds` rewrites `tests/fuzz-corpus/<target>/seed-*`).

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;

use bodies::{EVENTS, raw_frame};

#[test]
fn bt_frame_corpus() {
    replay::replay("bt_frame", bodies::bt_frame);
}

#[test]
fn bt_tx_corpus() {
    replay::replay("bt_tx", bodies::bt_tx);
}

const HEADSET: [u8; 6] = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
const PHONE: [u8; 6] = [0xA0, 0xB1, 0xC2, 0xD3, 0xE4, 0xF5];

/// The start-up exchange of bluetooth.md §4.1 (the RAM dump).
const RAM_DUMP: &[u8] = &[
    0x00, 0xAA, 0x00, 0x01, 0x23, 0xDC, 0xAA, 0x00, 0x02, 0x00, 0x06, 0xF8, 0xAA, 0x00, 0x09, 0x06, b'A',
    b'T', b'S', b'2', b'8', b'5', b'1', 0x00, 0x39, 0xAA, 0x00, 0x02, 0x00, 0x2C, 0xD2, 0xAA, 0x00, 0x03,
    0x2C, 0x06, 0x07, 0xC4, 0xAA, 0x00, 0x02, 0x00, 0x30, 0xCE, 0xAA, 0x00, 0x02, 0x00, 0x2A, 0xD4,
];

fn frame_seeds() -> Vec<(String, Vec<u8>)> {
    let mut link = vec![0u8, 0, 0];
    link.extend_from_slice(&HEADSET);
    let mut inq = HEADSET.to_vec();
    inq.extend_from_slice(&[0, 0x04, 0x04, 0x24]);
    inq.extend_from_slice(b"Headphones\0\0");
    let mut paired = vec![2u8, 0];
    paired.extend_from_slice(&PHONE);
    paired.extend_from_slice(b"Phone");
    let frames = [
        raw_frame(0x00, &[0x06]),
        raw_frame(0x01, &[0x05]),
        raw_frame(0x02, &link),
        raw_frame(0x03, &inq),
        raw_frame(0x04, &[]),
        raw_frame(0x06, b"ATS2851\0"),
        raw_frame(0x23, &[]),
        raw_frame(0x25, &[0x40]),
        raw_frame(0x26, &[0x7F]),
        raw_frame(0x2A, &paired),
        raw_frame(0x2C, &[6, 7]),
        raw_frame(0x31, &[0x55; 43]),
    ];
    let mut v = Vec::new();
    let mut all = vec![0u8];
    for (i, f) in frames.iter().enumerate() {
        let mut d = vec![i as u8];
        d.extend_from_slice(f);
        v.push((format!("frame-{i}"), d));
        all.extend_from_slice(f);
    }
    v.push(("all".into(), all));
    let mut dump = vec![5u8];
    dump.extend_from_slice(RAM_DUMP);
    v.push(("ram-dump".into(), dump));
    // Noise around frames, a false sync, a bad checksum, a bad length.
    let mut noisy = vec![3u8, 0x13, 0xAA, 0x00, 0x30, 0xAA, 0x00];
    noisy.extend_from_slice(&frames[2]);
    noisy.extend_from_slice(&[0xAA, 0x00, 0x02, 0x00, 0x06, 0x00, 0xAA, 0xFF, 0xFF]);
    noisy.extend_from_slice(&frames[9]);
    v.push(("noisy".into(), noisy));
    v
}

/// Operation encoder for `bt_tx` (see the target).
struct Script(Vec<u8>);

impl Script {
    fn new(config: u8) -> Self {
        Script(vec![config])
    }
    fn ev(&mut self, id: u8, p: &[u8]) -> &mut Self {
        let k = EVENTS.iter().position(|&e| e == id).unwrap() as u8;
        self.0.extend_from_slice(&[1, k, p.len() as u8]);
        self.0.extend_from_slice(p);
        self
    }
    fn ack(&mut self) -> &mut Self {
        self.0.push(3);
        self
    }
    fn tick(&mut self, k: u8) -> &mut Self {
        self.0.extend_from_slice(&[4, k]);
        self
    }
    fn wait(&mut self, k: u8) -> &mut Self {
        self.0.extend_from_slice(&[5, k]);
        self
    }
    fn on(&mut self) -> &mut Self {
        self.0.extend_from_slice(&[6, 0]);
        self
    }
    fn api(&mut self, k: u8, a: u8) -> &mut Self {
        self.0.extend_from_slice(&[7, k, a]);
        self
    }
    fn raw(&mut self, b: &[u8]) -> &mut Self {
        self.0.extend_from_slice(&[0, b.len() as u8]);
        self.0.extend_from_slice(b);
        self
    }
    fn started(config: u8) -> Self {
        let mut s = Script::new(config);
        s.on().ev(0x23, &[]).ack().ev(0x06, b"ATS2851\0").ack().ack();
        let mut p = vec![1u8, 0];
        p.extend_from_slice(&HEADSET);
        p.extend_from_slice(b"Headset");
        s.ev(0x2A, &p).wait(20);
        s
    }
}

fn tx_seeds() -> Vec<(String, Vec<u8>)> {
    let mut v = Vec::new();
    let mut s = Script::new(0);
    s.on().raw(&RAM_DUMP[..44]).wait(10).raw(&RAM_DUMP[44..]).wait(20);
    v.push(("ram-dump".into(), s.0.clone()));
    v.push(("started".into(), Script::started(0).0));
    v.push(("reconnect".into(), Script::started(0x41).0));
    // Inquiry with results and names.
    let mut s = Script::started(0);
    s.api(0, 0).ack();
    let mut inq = PHONE.to_vec();
    inq.extend_from_slice(&[0, 0x0C, 0x02, 0x5A]);
    s.ev(0x03, &inq);
    let mut inq2 = HEADSET.to_vec();
    inq2.extend_from_slice(&[0, 0x04, 0x04, 0x24]);
    inq2.extend_from_slice(b"Cans");
    s.ev(0x03, &inq2).ev(0x04, &[]).api(2, 0).ack();
    let mut name = PHONE.to_vec();
    name.extend_from_slice(b"Pixel");
    s.ev(0x05, &name).tick(10);
    v.push(("inquiry".into(), s.0.clone()));
    // Connect to the first found device, stream, volume, metadata,
    // disconnect.
    s.api(3, 0).ack();
    let mut link = vec![0u8, 0];
    link.extend_from_slice(&PHONE);
    s.ev(0x02, &link).ack().ack();
    let mut ctl = vec![2u8, 0];
    ctl.extend_from_slice(&PHONE);
    s.ev(0x02, &ctl).ack().ev(0x25, &[0x30]).api(8, 1).ack().api(9, 20).ack().ev(0x26, &[0x60]);
    s.api(11, 9).ack().ack().ack().ack().ack().ack().ack().api(10, 0).ack().ev(0x01, &[3]);
    v.push(("connected".into(), s.0.clone()));
    s.api(5, 0).ack();
    let mut down = vec![1u8, 0];
    down.extend_from_slice(&PHONE);
    s.ev(0x02, &down).api(6, 1).ack().api(7, 0).ack();
    v.push(("disconnect".into(), s.0.clone()));
    // Connect timeout and cancel.
    let mut s = Script::started(0);
    s.api(3, 1).ack().wait(255).wait(255).ack().ev(0x29, &[1]).tick(3);
    v.push(("connect-timeout".into(), s.0));
    // Unacknowledged commands: link down, restarts.
    let mut s = Script::new(0x30);
    s.on().ev(0x23, &[]);
    for _ in 0..8 {
        s.wait(200).tick(50);
    }
    v.push(("link-down".into(), s.0));
    // Shutdown.
    let mut s = Script::started(0);
    s.api(12, 0).ack().ev(0x24, &[]).on().ev(0x23, &[]);
    v.push(("shutdown".into(), s.0));
    // First start: forget all pairings.
    v.push(("forget-all".into(), Script::started(0x08).0));
    v
}

#[test]
fn seeds() {
    let f = frame_seeds();
    for (_, d) in &f {
        bodies::bt_frame(d);
    }
    let t = tx_seeds();
    for (_, d) in &t {
        bodies::bt_tx(d);
    }
    replay::write_seeds("bt_frame", &f);
    replay::write_seeds("bt_tx", &t);
}
