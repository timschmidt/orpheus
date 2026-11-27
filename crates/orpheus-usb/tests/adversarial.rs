//! Adversarial USB requests (VER-3, 07 §3.7): every SCSI/MMC opcode with
//! every CDB length and fill, LBA/count/allocation fields at their edges,
//! the Bulk-Only Transport's CBW cut and damaged at every byte, transfer
//! lengths that disagree with the command, and developer-channel requests
//! with every code, type and length. The oracles are the fuzz bodies'
//! (`fuzz/src/lib.rs`): medium accesses inside the medium and never on a
//! read-only or absent unit, CSW tag/status/residue, replies within the
//! transfer and allocation lengths, failures with sense data, no PEEK
//! outside development builds, reboots only for REBOOT.

#[path = "../../../tests/adversarial.rs"]
mod adversarial;
#[path = "../fuzz/src/lib.rs"]
mod bodies;

use adversarial::{config, damaged, edges, prefixes, put_be};
use orpheus_proto::devchan as dc;
use orpheus_usb::msc::bot::{CBW_SIG, Cbw};
use orpheus_usb::msc::scsi::Scsi;
use proptest::prelude::*;

/// Unit selectors of the fuzz body: disk, read-only, absent, CD, DVD, and
/// the same at full-speed packet size / removable.
const UNITS: [u8; 8] = [0, 1, 2, 3, 4, 0x90, 0x13, 0x24];

#[test]
fn every_opcode_every_length_every_unit() {
    for sel in UNITS {
        let u = bodies::unit_of(sel);
        let mut s = Scsi::new();
        for op in 0..=255u8 {
            for len in 1..=16usize {
                for fill in [0x00, 0xFF, 0x80, 0x01, 0x7F] {
                    let mut cdb = vec![fill; len];
                    cdb[0] = op;
                    bodies::check_plan(&mut s, &u, &cdb);
                }
            }
        }
    }
}

/// The LBA (`at`, `n` bytes) and count (`cat`, `cn` bytes) of the read and
/// write commands, big-endian, at their edges and around the medium's end.
#[test]
fn read_write_fields_at_their_edges() {
    let forms: [(u8, usize, usize, usize, usize, usize); 6] = [
        (0x28, 10, 2, 4, 7, 2),
        (0x2A, 10, 2, 4, 7, 2),
        (0xA8, 12, 2, 4, 6, 4),
        (0xAA, 12, 2, 4, 6, 4),
        (0x88, 16, 2, 8, 10, 4),
        (0x8A, 16, 2, 8, 10, 4),
    ];
    for sel in UNITS {
        let u = bodies::unit_of(sel);
        let mut s = Scsi::new();
        for (op, len, at, n, cat, cn) in forms {
            for lba in edges(8 * n as u32, u.blocks).into_iter().chain([u.blocks.saturating_sub(2)]) {
                for count in edges(8 * cn as u32, 2).into_iter().chain([u.blocks, u.blocks + 1]) {
                    let count = count.min(if cn == 2 { 0xFFFF } else { 0xFFFF_FFFF });
                    let mut cdb = vec![0u8; len];
                    cdb[0] = op;
                    put_be(&mut cdb, at, n, lba);
                    put_be(&mut cdb, cat, cn, count);
                    bodies::check_plan(&mut s, &u, &cdb);
                }
            }
        }
    }
}

/// Commands with an allocation length, at every edge of it.
#[test]
fn allocation_lengths_at_their_edges() {
    // (opcode, CDB length, offset, width, a usual value)
    let forms: [(u8, usize, usize, usize, u64); 16] = [
        (0x03, 6, 4, 1, 18),
        (0x12, 6, 3, 2, 36),
        (0x1A, 6, 4, 1, 192),
        (0x5A, 10, 7, 2, 8),
        (0xA0, 12, 6, 4, 16),
        (0x23, 10, 7, 2, 252),
        (0x46, 10, 7, 2, 8),
        (0x4A, 10, 7, 2, 8),
        (0x43, 10, 7, 2, 12),
        (0x51, 10, 7, 2, 34),
        (0x52, 10, 7, 2, 28),
        (0xAD, 12, 8, 2, 2052),
        (0xBD, 12, 8, 2, 8),
        (0xAC, 12, 8, 2, 8),
        (0x9E, 16, 10, 4, 32),
        (0x44, 10, 7, 2, 8),
    ];
    for sel in UNITS {
        let u = bodies::unit_of(sel);
        let mut s = Scsi::new();
        for (op, len, at, n, usual) in forms {
            for v in edges(8 * n as u32, usual) {
                for b1 in [0x00u8, 0x01, 0x02, 0x10, 0xFF] {
                    let mut cdb = vec![0u8; len];
                    cdb[0] = op;
                    cdb[1] = b1;
                    if op == 0x9E {
                        cdb[1] = 0x10 | (b1 & 0xE0);
                    }
                    cdb[2] = b1;
                    put_be(&mut cdb, at, n, v);
                    bodies::check_plan(&mut s, &u, &cdb);
                    // And every shorter CDB of the same command.
                    prefixes(&cdb, |p| {
                        if !p.is_empty() {
                            bodies::check_plan(&mut s, &u, p);
                        }
                    });
                }
            }
        }
    }
}

fn cbw(tag: u32, len: u32, dir_in: bool, cdb: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; 31];
    b[..4].copy_from_slice(&CBW_SIG.to_le_bytes());
    b[4..8].copy_from_slice(&tag.to_le_bytes());
    b[8..12].copy_from_slice(&len.to_le_bytes());
    b[12] = if dir_in { 0x80 } else { 0 };
    b[14] = cdb.len() as u8;
    b[15..15 + cdb.len()].copy_from_slice(cdb);
    b
}

#[test]
fn cbw_cut_and_damaged_at_every_byte() {
    let good = cbw(0x1234_5678, 512, true, &[0x28, 0, 0, 0, 0, 1, 0, 0, 1, 0]);
    assert!(Cbw::parse(&good).is_some());
    prefixes(&good, |p| assert!(Cbw::parse(p).is_none()));
    let mut long = good.clone();
    long.push(0);
    assert!(Cbw::parse(&long).is_none(), "a 32-byte packet is not a CBW");
    damaged(&good, |i, d| {
        if let Some(c) = Cbw::parse(d) {
            assert!((1..=16).contains(&c.cdb_len), "byte {i}: CDB length {}", c.cdb_len);
            assert!(c.lun <= 15);
        } else {
            assert!(i < 4 || i == 14, "byte {i}: refused");
        }
    });
    for l in 0..=255u8 {
        let mut b = good.clone();
        b[14] = l;
        if let Some(c) = Cbw::parse(&b) {
            assert!((1..=16).contains(&c.cdb_len));
        }
    }
}

/// Script for the `usb_scsi` body (its input format: unit, then per
/// command a kind byte, a 16-bit transfer length, the CDB and for OUT
/// transfers the data packets; 0xC0 = a raw packet).
#[derive(Clone, Debug)]
enum Op {
    Cmd { dir_in: bool, lun: bool, len: u16, blocks: bool, cdb: Vec<u8>, packets: Vec<u8> },
    Raw(Vec<u8>),
}

fn script(sel: u8, ops: &[Op]) -> Vec<u8> {
    let mut v = vec![sel];
    for op in ops {
        match op {
            Op::Cmd { dir_in, lun, len, blocks, cdb, packets } => {
                let k = (u8::from(*dir_in) << 7)
                    | (u8::from(*blocks) << 6)
                    | (u8::from(*lun) << 4)
                    | (cdb.len() as u8 - 1);
                v.push(k);
                v.extend_from_slice(&len.to_le_bytes());
                v.extend_from_slice(cdb);
                if !dir_in {
                    v.push(packets.len() as u8);
                    v.extend_from_slice(packets);
                }
            }
            Op::Raw(b) => {
                v.push(0xC0);
                v.push(b.len() as u8);
                v.extend_from_slice(b);
            }
        }
    }
    v
}

#[test]
fn transfer_lengths_that_disagree_with_the_command() {
    // Every command of a session with the CBW's transfer length at its
    // edges and the wrong direction: the CSW must still say what happened.
    let cmds: [&[u8]; 9] = [
        &[0x12, 0, 0, 0, 36, 0],
        &[0x03, 0, 0, 0, 18, 0],
        &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        &[0x28, 0, 0, 0, 0, 2, 0, 0, 2, 0],
        &[0x2A, 0, 0, 0, 0, 3, 0, 0, 2, 0],
        &[0x28, 0, 0, 0, 0, 63, 0, 0, 1, 0],
        &[0x28, 0, 0, 0, 0, 64, 0, 0, 1, 0],
        &[0x1A, 0, 0x3F, 0, 192, 0],
        &[0x43, 0, 0, 0, 0, 0, 0, 0, 12, 0],
    ];
    for sel in [0u8, 1, 2, 3, 0x80] {
        for cdb in cmds {
            for len in edges(16, 1024) {
                for dir_in in [true, false] {
                    for blocks in [false, true] {
                        let op = Op::Cmd {
                            dir_in,
                            lun: false,
                            len: len as u16,
                            blocks,
                            cdb: cdb.to_vec(),
                            packets: vec![0, 0, 3],
                        };
                        bodies::usb_scsi(&script(sel, &[op.clone(), op]));
                    }
                }
            }
        }
    }
}

fn op() -> impl Strategy<Value = Op> {
    let cdb = prop_oneof![
        // Real commands with any fields.
        (
            prop::sample::select(vec![
                0x00u8, 0x03, 0x12, 0x1A, 0x1B, 0x1E, 0x23, 0x25, 0x28, 0x2A, 0x2F, 0x35, 0x43, 0x46, 0x4A,
                0x51, 0x52, 0x5A, 0x88, 0x8A, 0x9E, 0xA0, 0xA8, 0xAA, 0xAD, 0xBD, 0xBE, 0xCB, 0xCC,
            ]),
            prop::collection::vec(any::<u8>(), 5..16)
        )
            .prop_map(|(o, mut rest)| {
                rest.insert(0, o);
                rest
            }),
        prop::collection::vec(any::<u8>(), 1..=16),
    ];
    prop_oneof![
        8 => (any::<bool>(), any::<bool>(), any::<u16>(), any::<bool>(), cdb, prop::collection::vec(any::<u8>(), 0..6))
            .prop_map(|(dir_in, lun, len, blocks, cdb, packets)| Op::Cmd { dir_in, lun, len, blocks, cdb, packets }),
        1 => prop::collection::vec(any::<u8>(), 0..63).prop_map(Op::Raw),
    ]
}

/// A devchan request for the `usb_devchan` body: setup packet, then the
/// OUT data or the IN buffer size (× 4).
#[derive(Clone, Debug)]
struct Req {
    rt: u8,
    req: u8,
    value: u16,
    index: u16,
    length: u16,
    data: Vec<u8>,
    buf4: u8,
}

fn devchan_script(dev: bool, reqs: &[Req]) -> Vec<u8> {
    let mut v = vec![u8::from(dev)];
    for r in reqs {
        v.extend([r.rt, r.req]);
        v.extend(r.value.to_le_bytes());
        v.extend(r.index.to_le_bytes());
        v.extend(r.length.to_le_bytes());
        if r.rt & 0x80 == 0 {
            v.push(r.data.len() as u8);
            v.extend_from_slice(&r.data);
        } else {
            v.push(r.buf4);
        }
    }
    v
}

#[test]
fn every_devchan_request_code_type_and_length() {
    let types = [0xC0u8, 0x40, 0xC1, 0x41, 0xA0, 0x20, 0x80, 0x00, 0xC2, 0xE0];
    for dev in [false, true] {
        for req in 0..=255u8 {
            let mut reqs = Vec::new();
            for rt in types {
                for length in edges(16, 64) {
                    for value in [0u16, 1, 0x7FFF, 0xFFFF] {
                        let data =
                            if rt & 0x80 == 0 { vec![0xA5; (length as usize).min(255)] } else { vec![] };
                        reqs.push(Req { rt, req, value, index: value, length: length as u16, data, buf4: 0 });
                        reqs.push(Req {
                            rt,
                            req,
                            value,
                            index: 0,
                            length: length as u16,
                            data: vec![1; 7],
                            buf4: 255,
                        });
                    }
                }
            }
            // The body stops after 200 requests: feed them in batches.
            for batch in reqs.chunks(150) {
                bodies::usb_devchan(&devchan_script(dev, batch));
            }
        }
    }
}

fn req() -> impl Strategy<Value = Req> {
    (
        prop::sample::select(vec![0xC0u8, 0x40, 0xC1, 0x41, 0x80, 0x21]),
        prop_oneof![4 => 0u8..=0x0E, 1 => any::<u8>()],
        any::<u16>(),
        any::<u16>(),
        prop_oneof![any::<u16>(), 0u16..80],
        prop::collection::vec(any::<u8>(), 0..64),
        any::<u8>(),
    )
        .prop_map(|(rt, req, value, index, length, data, buf4)| Req {
            rt,
            req,
            value,
            index,
            length,
            data,
            buf4,
        })
}

proptest! {
    #![proptest_config(config("usb-scsi-sessions", 256))]

    /// Commands, transfer lengths, directions, LUNs, data packets and
    /// garbage packets in any order, on every kind of unit.
    #[test]
    fn scsi_sessions_in_any_order(sel in any::<u8>(), ops in prop::collection::vec(op(), 1..24)) {
        bodies::usb_scsi(&script(sel, &ops));
    }
}

proptest! {
    #![proptest_config(config("usb-devchan-sequences", 256))]

    /// Developer-channel requests in any order (REBOOT, KEYS, TIME, PUT,
    /// TEST, PEEK… interleaved with standard and class requests).
    #[test]
    fn devchan_requests_in_any_order(dev in any::<bool>(), reqs in prop::collection::vec(req(), 1..40)) {
        bodies::usb_devchan(&devchan_script(dev, &reqs));
        let _ = dc::REQ_UPDATE;
    }
}
