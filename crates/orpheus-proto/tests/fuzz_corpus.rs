//! The fuzz target's committed corpus and regression inputs, replayed on
//! stable through the body the cargo-fuzz target runs (VER-4); and the seed
//! inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-proto --test
//! fuzz_corpus seeds` rewrites `tests/fuzz-corpus/proto_records/seed-*`).

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;

use orpheus_proto::devchan::Info;
use orpheus_proto::fault::{FaultRecord, Kind};
use orpheus_proto::mailbox::{BootInfo, Mailbox};
use orpheus_proto::result::{ResultBlock, Verdict};
use orpheus_proto::slot::{LAYOUT_FULL, SlotHeader};
use orpheus_proto::{logring, wr32};

#[test]
fn proto_records_corpus() {
    replay::replay("proto_records", bodies::proto_records);
}

fn seeds() -> Vec<(String, Vec<u8>)> {
    let with = |sel: u8, b: &[u8]| {
        let mut v = vec![sel];
        v.extend(std::iter::repeat_n(0xEE, usize::from(sel & 7)));
        v.extend_from_slice(b);
        v
    };
    let mut text = [0u8; 128];
    text[..18].copy_from_slice(b"panicked at x.rs:3");
    let f = FaultRecord {
        kind: Kind::Panic as u32,
        pc: 0x0010_2345,
        lr: 0x0010_2001,
        cfsr: 0x0000_8200,
        bfar: 0xE000_0000,
        count: 2,
        build_id: [0xAB; 16],
        text,
        ..FaultRecord::default()
    };
    let mut m = Mailbox::test(7, 3, &[1, 2, 3]);
    m.boot_info = BootInfo { path: 1, attempt: 2, keys: 4 }.to_word();
    m.boot_count = 0x594D_4202;
    let mut msg = [0u8; 128];
    msg[..4].copy_from_slice(b"ok 1");
    let mut counters = [0u32; 64];
    counters[..3].copy_from_slice(&[10, 20, 30]);
    let r = ResultBlock {
        test_id: 7,
        verdict: Some(Verdict::Pass),
        counters,
        n_counters: 3,
        message: msg,
        seq: 9,
    };
    let h = SlotHeader {
        layout: LAYOUT_FULL,
        resident_off: 0x80,
        resident_len: 0x1_3000,
        load_addr: 0x0010_0000,
        entry: 0x0010_0101,
        pages_off: 0x1_4000,
        code_pages: 73,
        rodata_pages: 16,
        build_id: [3; 16],
        ..SlotHeader::default()
    };
    let i =
        Info { version: 1, layout: 2, personality: 1, uptime_ms: 12345, battery_mv: 3900, ..Info::default() };
    let ring = |write: u32| {
        let mut a = vec![0u8; logring::SIZE];
        wr32(&mut a, logring::OFF_MAGIC, logring::MAGIC);
        wr32(&mut a, logring::OFF_WRITE, write);
        wr32(&mut a, logring::OFF_CAP, logring::CAP as u32);
        for (k, b) in a[logring::HEADER..].iter_mut().enumerate() {
            *b = (k % 251) as u8;
        }
        a
    };
    vec![
        ("fault".into(), with(0, &f.to_bytes())),
        ("mailbox".into(), with(1, &m.to_bytes())),
        ("result".into(), with(2, &r.to_bytes())),
        ("slot".into(), with(3, &h.to_bytes())),
        ("info".into(), with(4, &i.to_bytes())),
        ("ring".into(), with(0, &ring(100))),
        ("ring-wrapped".into(), with(0xC0, &ring(5000))),
        ("ring-small-out".into(), with(0x40, &ring(4000))),
    ]
}

#[test]
fn write_seeds() {
    let s = seeds();
    for (_, d) in &s {
        bodies::proto_records(d);
    }
    replay::write_seeds("proto_records", &s);
}
