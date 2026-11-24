//! Fuzz-target body of `orpheus-proto` (VER-4), shared by the cargo-fuzz
//! target in `fuzz_targets/` (nightly) and the stable replay of the
//! committed corpus (`tests/fuzz_corpus.rs`, every `cargo test`).
//!
//! [`proto_records`] — NOR and RAM contents after a crash or a reboot as the
//! host tools and the boot stages read them — fault record, mailbox, HIL
//! result block, slot header, persistent log ring, developer-channel INFO
//! — each parsed from the input at an offset byte 0 picks. Oracles: no
//! panic; every record that parses re-encodes to bytes that parse to the
//! same record (and, where the format has no unused bytes, to the input
//! itself); the log ring unrolls to a rotation of its data area of a length
//! within the capacity and the output; the word decoders round-trip.

#![forbid(unsafe_code)]

use orpheus_proto::devchan::Info;
use orpheus_proto::fault::{self, FaultRecord, Kind};
use orpheus_proto::mailbox::{self, BootInfo, Mailbox, Request};
use orpheus_proto::result::{self, ResultBlock, Verdict};
use orpheus_proto::slot::{self, SlotHeader};
use orpheus_proto::{logring, rd32};

/// All record parsers over `data[skip..]` (see the crate docs).
pub fn proto_records(data: &[u8]) {
    let Some((&sel, rest)) = data.split_first() else { return };
    let b = &rest[usize::from(sel & 7).min(rest.len())..];

    if let Some(r) = FaultRecord::from_bytes(b) {
        let enc = r.to_bytes();
        assert_eq!(FaultRecord::from_bytes(&enc).as_ref(), Some(&r));
        // Every field the record keeps is where it was.
        for (o, n) in [(0, 0x58), (fault::OFF_BUILD_ID, 16), (fault::OFF_TEXT, fault::TEXT_LEN)] {
            assert_eq!(&enc[o..o + n], &b[o..o + n], "fault record bytes {o:#x}");
        }
        if let Some(k) = r.kind() {
            assert_eq!(Kind::from_word(k as u32), Some(k));
            assert!(!k.name().is_empty());
        }
        let _ = r.text_str();
        assert!(fault::cfsr_bits(r.cfsr).count() <= 32);
    } else {
        assert!(b.len() < fault::SIZE || rd32(b, 0) != Some(fault::MAGIC));
    }

    match Mailbox::from_bytes(b) {
        Some(m) => {
            // The mailbox has no unused bytes: the encoding is the input.
            assert_eq!(&m.to_bytes()[..], &b[..mailbox::SIZE]);
            let _ = m.boot_attempts();
            let req = Request::from_word(m.request);
            if req != Request::None {
                assert_eq!(req as u32, m.request);
            }
            if let Some(i) = BootInfo::from_word(m.boot_info) {
                assert_eq!(i.to_word(), m.boot_info);
            }
        }
        None => assert!(b.len() < mailbox::SIZE),
    }

    if let Some(r) = ResultBlock::from_bytes(b) {
        assert!(r.n_counters <= result::N_COUNTERS);
        assert_eq!(r.counters().len(), r.n_counters);
        let enc = r.to_bytes();
        assert_eq!(ResultBlock::from_bytes(&enc).as_ref(), Some(&r));
        if let Some(v) = r.verdict {
            assert_eq!(Verdict::from_word(v as u32), Some(v));
        }
        let _ = r.message_str();
    }

    match SlotHeader::from_bytes(b) {
        Ok(h) => {
            let enc = h.to_bytes();
            assert_eq!(SlotHeader::from_bytes(&enc), Ok(h));
            // Only the reserved words may differ.
            for o in (0..0x50).step_by(4).filter(|&o| o != 0x0C) {
                assert_eq!(rd32(&enc, o), rd32(b, o), "slot header word {o:#x}");
            }
        }
        Err(slot::HeaderError::Short) => assert!(b.len() < slot::HEADER_SIZE),
        Err(_) => assert!(b.len() >= slot::HEADER_SIZE),
    }

    match Info::from_bytes(b) {
        Some(i) => assert_eq!(&i.to_bytes()[..], &b[..Info::SIZE]),
        None => assert!(b.len() < Info::SIZE),
    }

    // Log ring: into an output of a size byte 0 picks.
    let mut out = vec![0u8; [logring::CAP, 16, 0, 4096][usize::from(sel >> 6)]];
    if let Some((len, wrapped)) = logring::unroll(b, &mut out) {
        let write = rd32(b, logring::OFF_WRITE).unwrap() as usize;
        let cap = rd32(b, logring::OFF_CAP).unwrap() as usize;
        let area = &b[logring::HEADER..logring::HEADER + cap];
        assert_eq!(len, write.min(cap));
        assert!(len <= out.len());
        assert_eq!(wrapped, write > cap);
        if wrapped {
            let start = write % cap;
            let rot: Vec<u8> = area[start..].iter().chain(&area[..start]).copied().collect();
            assert_eq!(&out[..len], &rot[..len]);
        } else {
            assert_eq!(&out[..len], &area[..len]);
        }
    }

    // Word decoders over the first words.
    for o in (0..b.len().min(64)).step_by(4) {
        let Some(w) = rd32(b, o) else { break };
        if let Some(k) = Kind::from_word(w) {
            assert_eq!(k as u32, w);
        }
        if let Some(i) = BootInfo::from_word(w) {
            assert_eq!(i.to_word(), w);
        }
        if let Some(v) = Verdict::from_word(w) {
            assert_eq!(v as u32, w);
        }
    }
}
