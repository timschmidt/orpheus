//! Fuzz-target body of `orpheus-sdcard` (VER-4), shared by the cargo-fuzz
//! target in `fuzz_targets/` (nightly) and the stable replay of the
//! committed corpus (`tests/fuzz_corpus.rs`, every `cargo test`).
//!
//! [`sd_registers`] — the 128-bit CID and CSD registers and the 32-bit R1
//! and OCR responses a card (or a broken one) sends. Oracles: no panic or
//! overflow; every bit field reads as the same bits of the 128-bit value;
//! the CID fields are the register's bytes; a CSD parses exactly when its
//! structure version is 1–3, its capacity follows the version's formula
//! and its byte size does not overflow; the response flags are their bits.

#![forbid(unsafe_code)]

use orpheus_sdcard::{Cid, Csd, Ocr, R128, State, Status, r128_from_bytes};

fn field(v: u128, hi: u32, lo: u32) -> u32 {
    let w = hi - lo + 1;
    let mask = if w >= 128 { u128::MAX } else { (1u128 << w) - 1 };
    ((v >> lo) & mask) as u32
}

fn check_register(b: &[u8; 16]) {
    let r: R128 = r128_from_bytes(b);
    let v = u128::from_be_bytes(*b);
    // Fields the parsers use, then a few the input picks.
    let mut pairs: Vec<(u32, u32)> = vec![
        (127, 126),
        (103, 96),
        (95, 84),
        (83, 80),
        (73, 62),
        (49, 47),
        (69, 48),
        (75, 48),
        (127, 120),
        (55, 24),
        (19, 12),
        (11, 8),
        (31, 0),
        (127, 96),
    ];
    for k in b.chunks_exact(2) {
        let hi = u32::from(k[0] % 128);
        let lo = hi.saturating_sub(u32::from(k[1] % 32));
        pairs.push((hi, lo));
    }
    for (hi, lo) in pairs {
        assert_eq!(r.bits(hi, lo), field(v, hi, lo), "bits {hi}:{lo} of {b:02X?}");
    }

    let c = Cid::parse(&r);
    assert_eq!(c.manufacturer, b[0]);
    assert_eq!(c.oem, [b[1], b[2]]);
    assert_eq!(c.name, [b[3], b[4], b[5], b[6], b[7]]);
    assert_eq!(c.revision, b[8]);
    assert_eq!(c.serial, u32::from_be_bytes([b[9], b[10], b[11], b[12]]));
    assert_eq!(c.date, (2000 + u16::from((b[13] & 0xF) << 4 | b[14] >> 4), b[14] & 0xF));

    let structure = b[0] >> 6;
    match Csd::parse(&r) {
        Some(d) => {
            assert!(structure <= 2);
            assert_eq!(d.version, structure + 1);
            assert_eq!(d.tran_speed, b[3]);
            assert_eq!(u32::from(d.ccc), field(v, 95, 84));
            let blocks = match structure {
                0 => {
                    let bytes =
                        (u64::from(field(v, 73, 62)) + 1) << (field(v, 49, 47) + 2) << field(v, 83, 80);
                    bytes / 512
                }
                1 => (u64::from(field(v, 69, 48)) + 1) * 1024,
                _ => (u64::from(field(v, 75, 48)) + 1) * 1024,
            };
            assert_eq!(d.blocks, blocks);
            assert!(d.blocks.checked_mul(512).is_some_and(|x| x == d.bytes()));
        }
        None => assert_eq!(structure, 3),
    }
}

fn check_word(w: u32) {
    let s = Status(w);
    let st = s.state();
    let code = (w >> 9) & 0xF;
    match st {
        State::Other(x) => assert!(u32::from(x) == code && code > 8),
        _ => assert!(code <= 8),
    }
    assert_eq!(s.ready_for_data(), w & (1 << 8) != 0);
    assert_eq!(s.has_error(), w & Status::ERRORS != 0);
    let o = Ocr(w);
    assert_eq!(o.ready(), w >> 31 == 1);
    assert_eq!(o.high_capacity(), (w >> 30) & 1 == 1);
}

/// Every 16-byte window of the input as a register, every 4-byte window
/// as a response word.
pub fn sd_registers(data: &[u8]) {
    for chunk in data.chunks(16).take(256) {
        let mut b = [0u8; 16];
        b[..chunk.len()].copy_from_slice(chunk);
        check_register(&b);
        for w in b.chunks_exact(4) {
            check_word(u32::from_be_bytes([w[0], w[1], w[2], w[3]]));
        }
    }
}
