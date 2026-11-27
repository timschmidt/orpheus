//! Adversarial inputs for the update path (VER-3, 07 §3.7): `.ofw` files
//! from the card or the developer channel, and the developer channel's
//! `UPDATE` requests in any order. Whatever arrives, the installer never
//! writes outside the target slot, never programs over unerased bits, and
//! stage 2 always finds either the old image (first) or the new one,
//! complete and verified.

#[path = "../../../tests/adversarial.rs"]
mod adversarial;

use adversarial::{config, damaged, edges, prefixes, put_le};
use orpheus_proto::hash::crc32_update;
use orpheus_proto::layout::SLOTS;
use orpheus_proto::slot::{self, SlotHeader};
use orpheus_update::Error;
use orpheus_update::install::{Installer, Phase};
use orpheus_update::ofw::{self, CHANGES_MAX, HEADER_SIZE, IMAGE_MAX, check};
use orpheus_update::sim::{SimNor, image, ofw_header};
use orpheus_update::slots::{choose, stage2_pick};
use orpheus_update::wire::Status;
use proptest::prelude::*;

/// The header CRC (`ofw` layout: the word at 0x7C read as 0).
fn reseal(b: &mut [u8; HEADER_SIZE]) {
    let mut c = crc32_update(0xFFFF_FFFF, &b[..0x7C]);
    c = crc32_update(c, &[0; 4]);
    let c = crc32_update(c, &b[0x80..]) ^ 0xFFFF_FFFF;
    b[0x7C..0x80].copy_from_slice(&c.to_le_bytes());
}

/// What `check` promises when it accepts: everything the installer, stage
/// 2 and the pager index with lies inside the image.
fn assert_sane(h: &ofw::Header, s: &SlotHeader) {
    assert!(h.image_len <= IMAGE_MAX && h.image_len >= 0x1004);
    assert!(usize::from(h.changes_len) <= CHANGES_MAX);
    assert!(h.version_str().is_some());
    let res_end = u64::from(s.resident_off) + u64::from(s.resident_len);
    assert!(res_end <= u64::from(h.image_len));
    let pages = (u64::from(s.code_pages) + u64::from(s.rodata_pages)) * u64::from(slot::PAGE_SIZE);
    assert!(u64::from(s.pages_off) + pages <= u64::from(h.image_len));
    assert!(s.resident_off >= 0x1000 && s.pages_off >= 0x1000);
}

#[test]
fn ofw_cut_at_every_byte() {
    let img = image(7, 0x1800, 1);
    let h = ofw_header(&img, "1.2.3", "Line one\nLine two");
    prefixes(&h, |p| assert_eq!(check(p, &img[..128]).unwrap_err(), Error::Short));
    prefixes(&img[..128], |p| assert_eq!(check(&h, p).unwrap_err(), Error::SlotHeader));
    // The change list reader on every cut: never more than is there.
    prefixes(&h, |p| assert!(ofw::changes(p).len() <= p.len().saturating_sub(ofw::CHANGES)));
    assert!(check(&h, &img[..128]).is_ok());
}

#[test]
fn ofw_every_damaged_byte_resealed() {
    // Each byte of the header replaced (complement, 0x00, 0xFF) with the
    // CRC made right again: the header's own checks must refuse or accept
    // only a sane header; the change list stays text.
    let img = image(8, 0x1800, 1);
    let good = ofw_header(&img, "2.0.0", "Changes");
    damaged(&good, |i, d| {
        let mut b: [u8; HEADER_SIZE] = d.try_into().unwrap();
        if !(0x7C..0x80).contains(&i) {
            reseal(&mut b);
        }
        match check(&b, &img[..128]) {
            Ok((h, s)) => assert_sane(&h, &s),
            Err(e) => assert_ne!(e, Error::Short, "byte {i}"),
        }
        let _ = ofw::changes(&b).len();
    });
}

#[test]
fn ofw_length_fields_at_their_edges() {
    let img = image(9, 0x1800, 1);
    let good = ofw_header(&img, "2.0.0", "abc");
    for (off, real) in [(0x10, img.len() as u64), (0x14, 3), (0x08, HEADER_SIZE as u64), (0x04, 1)] {
        for v in edges(32, real) {
            let mut b = good;
            put_le(&mut b, off, 4, v);
            reseal(&mut b);
            if let Ok((h, s)) = check(&b, &img[..128]) {
                assert_sane(&h, &s);
            }
            assert!(ofw::changes(&b).len() <= CHANGES_MAX, "field {off:#x} = {v:#x}");
        }
    }
}

/// A slot header word, by reference.
type Field = fn(&mut SlotHeader) -> &mut u32;

#[test]
fn slot_header_fields_at_their_edges() {
    // Every word of the slot header inside the image at its edge values,
    // with the slot header's CRC made right: `check` accepts only bounds
    // that lie inside the image.
    let img = image(10, 0x2000, 2);
    let base = SlotHeader::from_bytes(&img).unwrap();
    let o = ofw_header(&img, "3.0", "");
    let fields: [(&str, Field); 9] = [
        ("resident_off", |s| &mut s.resident_off),
        ("resident_len", |s| &mut s.resident_len),
        ("load_addr", |s| &mut s.load_addr),
        ("entry", |s| &mut s.entry),
        ("pages_off", |s| &mut s.pages_off),
        ("code_pages", |s| &mut s.code_pages),
        ("rodata_pages", |s| &mut s.rodata_pages),
        ("rodata_code_pages", |s| &mut s.rodata_code_pages),
        ("layout", |s| &mut s.layout),
    ];
    let mut accepted = 0;
    for (name, field) in fields {
        let real = u64::from(*field(&mut base.clone()));
        for v in edges(32, real).into_iter().chain([img.len() as u64, img.len() as u64 - 1]) {
            let mut s = base;
            *field(&mut s) = v as u32;
            if let Ok((h, s)) = check(&o, &s.to_bytes()) {
                assert_sane(&h, &s);
                accepted += 1;
            } else {
                assert!(v != real, "{name}: the real value {v:#x} refused");
            }
        }
    }
    assert!(accepted > 0);
}

#[test]
fn status_reply_cut_and_any_bytes() {
    let s = Status::from_bytes(&[0xFF; 16]).unwrap();
    assert_eq!(s.error(), None);
    prefixes(&[0u8; 16], |p| assert!(Status::from_bytes(p).is_none()));
    for c in 0..=255u8 {
        let mut b = [0u8; 16];
        b[0] = c;
        b[1] = c;
        let s = Status::from_bytes(&b).unwrap();
        assert_eq!(Status::from_bytes(&s.to_bytes()).unwrap().error, c);
    }
}

/// One developer-channel or card-task step of an update.
#[derive(Clone, Debug)]
enum Step {
    /// Image bytes `[off, off + len)`, or (`garbage`) that many 0x5A bytes.
    Data {
        off: u32,
        len: u16,
        garbage: bool,
    },
    /// The next piece in order (most steps, so updates get far).
    Next {
        len: u16,
    },
    Verify {
        buf: u16,
    },
    Commit,
    Abort,
    Start,
    /// Lose power before the next NOR operation (then the player restarts
    /// and stage 2 is asked).
    Cut,
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        48 => (1u16..6000).prop_map(|len| Step::Next { len }),
        4 => (0u32..0x9000, 0u16..3000, any::<bool>()).prop_map(|(off, len, garbage)| Step::Data { off, len, garbage }),
        8 => (0u16..8192).prop_map(|buf| Step::Verify { buf }),
        3 => Just(Step::Commit),
        1 => Just(Step::Abort),
        1 => Just(Step::Start),
        1 => Just(Step::Cut),
    ]
}

fn holds(nor: &SimNor, i: usize, img: &[u8]) -> bool {
    let b = SLOTS[i] as usize;
    nor.data[b..b + img.len()] == *img
}

proptest! {
    #![proptest_config(config("update-requests-in-any-order", 192))]

    /// Requests in any order, any size, repeated, out of order, garbage
    /// data, power cuts: the old image stays bootable and first until the
    /// new one is complete and verified; nothing outside the target slot
    /// (or the old slot's magic word) is ever written; no program needs an
    /// erase.
    #[test]
    fn update_requests_in_any_order(booted in 0u8..2, start in any::<bool>(),
                                    steps in prop::collection::vec(step(), 1..100)) {
        let old = image(1, 0x1800, 1);
        let new = image(2, 0x2400, 2);
        let mut nor = SimNor::new();
        let old_slot = usize::from(booted);
        nor.load(SLOTS[old_slot], &old);
        let t = choose(true, Some(booted)).unwrap();
        let new_slot = usize::from(t.index);
        let h = ofw_header(&new, "9.9", "");
        let mut ins = Installer::new(&h, &new[..128], t).unwrap();
        let mut cut = false;
        if start {
            ins.start(&mut nor).unwrap();
        }
        for s in steps {
            if cut {
                break;
            }
            let r = match s {
                Step::Start => ins.start(&mut nor),
                Step::Next { len } => {
                    let off = ins.received();
                    let end = (off + u32::from(len)).min(new.len() as u32);
                    ins.write(&mut nor, off, &new[off as usize..end as usize])
                }
                Step::Data { off, len, garbage } => {
                    let end = (u64::from(off) + u64::from(len)).min(new.len() as u64) as usize;
                    let off = (off as usize).min(end);
                    let repeat = end as u32 <= ins.received() && ins.phase() == Phase::Writing;
                    if garbage {
                        ins.write(&mut nor, off as u32, &vec![0x5A; end - off])
                    } else {
                        let r = ins.write(&mut nor, off as u32, &new[off..end]);
                        // A repeat of a piece already taken is accepted.
                        prop_assert!(!repeat || r.is_ok(), "repeat {off}..{end}: {r:?}");
                        r
                    }
                }
                Step::Verify { buf } => {
                    let mut b = vec![0u8; usize::from(buf).max(1)];
                    ins.verify_step(&mut nor, &mut b).map(|_| ())
                }
                Step::Commit => ins.commit(&mut nor),
                Step::Abort => {
                    ins.abort();
                    Ok(())
                }
                Step::Cut => {
                    nor.cut_after = Some(nor.ops);
                    cut = true;
                    Ok(())
                }
            };
            if let Err(e) = r {
                prop_assert!(e != Error::Refused, "a guard refused a write: {s:?}");
            }
            prop_assert_eq!((nor.forbidden, nor.unerased), (0, 0));
            if let Phase::Done = ins.phase() {
                prop_assert!(holds(&nor, new_slot, &new));
            }
        }
        // Whatever happened (and after a power cut), stage 2 starts the
        // old image, or the new one only when it is complete and the
        // installer got to commit it.
        nor.power_on();
        let mut r = nor.reader();
        let picked = stage2_pick(&mut r);
        drop(r);
        match picked {
            Some(i) if i == new_slot => prop_assert!(holds(&nor, new_slot, &new)),
            Some(i) => {
                prop_assert_eq!(i, old_slot);
                prop_assert!(holds(&nor, old_slot, &old));
            }
            None => prop_assert!(false, "nothing bootable"),
        }
    }

    /// An `.ofw` header and slot header of arbitrary bytes (most with the
    /// magic and CRCs made right, so the deeper checks are reached).
    #[test]
    fn any_header_bytes(mut o in prop::collection::vec(any::<u8>(), HEADER_SIZE),
                        mut s in prop::collection::vec(any::<u8>(), 128),
                        fix in any::<bool>()) {
        let mut b: [u8; HEADER_SIZE] = o.as_slice().try_into().unwrap();
        if fix {
            b[..4].copy_from_slice(b"OFW1");
            put_le(&mut b, 4, 4, 1);
            put_le(&mut b, 8, 4, HEADER_SIZE as u64);
            put_le(&mut b, 0x0C, 4, u64::from(slot::LAYOUT_FULL));
            reseal(&mut b);
            if let Ok(mut sh) = SlotHeader::from_bytes(&image(3, 0x1000, 1)) {
                sh.build_id.copy_from_slice(&b[0x28..0x38]);
                sh.resident_off = u32::from_le_bytes(s[..4].try_into().unwrap()) % 0x9000;
                sh.resident_len = u32::from_le_bytes(s[4..8].try_into().unwrap()) % 0x40000;
                sh.pages_off = u32::from_le_bytes(s[8..12].try_into().unwrap()) % 0x80000;
                sh.code_pages = u32::from(s[12]);
                sh.rodata_pages = u32::from(s[13]);
                s = sh.to_bytes().to_vec();
            }
        }
        o = b.to_vec();
        if let Ok((h, sh)) = check(&o, &s) {
            assert_sane(&h, &sh);
            let t = choose(true, Some(0)).unwrap();
            prop_assert!(Installer::new(&o, &s, t).is_ok());
        }
    }
}
