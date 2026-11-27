//! Adversarial inputs (VER-3, 07 §3.7) for the Bluetooth module's UART byte stream and the transmitter
//! state machine: every
//! committed seed of the crate's fuzz targets swept (`sweep` in
//! `tests/adversarial.rs`: cut at every byte of its head and at spread
//! points beyond, every head byte damaged, 16/32-bit fields at
//! 0/1/max−1/max at every head offset in both byte orders) through the
//! fuzz bodies, whose oracles are in `fuzz/src/lib.rs` and
//! `docs/verification/baseline-2026-11.md` §8.1.

#[path = "../../../tests/adversarial.rs"]
mod adversarial;
#[path = "../fuzz/src/lib.rs"]
#[allow(dead_code)]
mod bodies;

#[allow(unused_imports)]
use adversarial::{config, seeds, sweep};

/// Head bytes swept for `bt_frame`.
const HEAD_BT_FRAME: usize = 64;
/// Head bytes swept for `bt_tx`.
const HEAD_BT_TX: usize = 64;

#[test]
fn bt_frame_seeds_swept() {
    for (name, s) in seeds("bt_frame") {
        eprintln!("{name}");
        sweep(&s, 0, HEAD_BT_FRAME, bodies::bt_frame);
    }
}

#[test]
fn bt_tx_seeds_swept() {
    for (name, s) in seeds("bt_tx") {
        eprintln!("{name}");
        sweep(&s, 0, HEAD_BT_TX, bodies::bt_tx);
    }
}

use orpheus_bt::frame::{MAX_RX_PARAMS, SYNC};
use proptest::prelude::*;

#[test]
fn every_event_every_length_split_at_every_byte() {
    // Each event id with every parameter length up to one past the
    // limit, a wrong checksum, a length field at its edges, a stray sync
    // byte inside: decoded whole and in every chunk size (bt_frame's
    // selector byte picks 1–17), and fed to the transmitter.
    for &id in bodies::EVENTS.iter().chain(&[0x00u8, 0x7F, 0xFF]) {
        for n in [0, 1, 2, 6, 7, 8, MAX_RX_PARAMS - 1, MAX_RX_PARAMS, MAX_RX_PARAMS + 1] {
            let params: Vec<u8> = (0..n).map(|i| (i as u8).wrapping_mul(37) ^ SYNC).collect();
            let good = bodies::raw_frame(id, &params);
            let mut bad_sum = good.clone();
            *bad_sum.last_mut().unwrap() ^= 0x01;
            let mut variants = vec![good.clone(), bad_sum];
            for len in [0u16, 1, 0x00FF, 0x0100, 0x7FFF, 0xFFFF] {
                let mut b = good.clone();
                b[1..3].copy_from_slice(&len.to_be_bytes());
                variants.push(b);
            }
            let mut stray = good.clone();
            stray.insert(2, SYNC);
            variants.push(stray);
            for v in variants {
                // Each variant between two good frames.
                let mut stream = bodies::raw_frame(0x03, &[1, 2, 3]);
                stream.extend_from_slice(&v);
                stream.extend_from_slice(&bodies::raw_frame(0x03, &[4, 5, 6]));
                for sel in 0..17u8 {
                    let mut d = vec![sel];
                    d.extend_from_slice(&stream);
                    bodies::bt_frame(&d);
                }
                // To the transmitter: power on, the bytes, time.
                let mut d = vec![0x01, 6, 0, 0, 48];
                d.extend(stream.iter().take(48));
                d.extend([4, 200, 5, 255]);
                bodies::bt_tx(&d);
            }
        }
    }
}

/// One `bt_tx` operation (its grammar: op % 10 and its argument bytes).
fn tx_op() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        // Raw module bytes.
        prop::collection::vec(any::<u8>(), 0..48).prop_map(|b| {
            let mut v = vec![0, b.len() as u8];
            v.extend(b);
            v
        }),
        // A well-formed event with any parameters.
        (any::<u8>(), prop::collection::vec(any::<u8>(), 0..=MAX_RX_PARAMS)).prop_map(|(e, p)| {
            let mut v = vec![1, e, p.len() as u8];
            v.extend(p);
            v
        }),
        Just(vec![3]),
        any::<u8>().prop_map(|t| vec![4, t]),
        any::<u8>().prop_map(|t| vec![5, t]),
        any::<u8>().prop_map(|p| vec![6, p]),
        (0u8..16, any::<u8>()).prop_map(|(k, a)| vec![7, k, a]),
    ]
}

proptest! {
    #![proptest_config(config("bt-requests-and-events-in-any-order", 512))]

    /// Firmware requests, module events, acknowledges, time and power in
    /// any order (the body's oracles: no restricted command, nothing sent
    /// while off, lists and names in bounds).
    #[test]
    fn requests_and_events_in_any_order(cfg in any::<u8>(), ops in prop::collection::vec(tx_op(), 1..80)) {
        let mut d = vec![cfg];
        for o in ops {
            d.extend(o);
        }
        bodies::bt_tx(&d);
    }
}
