//! Every flow against the simulated player ([`crate::mock`]).

use orpheus_proto::devchan::{REBOOT_ADFU, REBOOT_NORMAL};
use orpheus_proto::layout::{NOR_SIZE, SECTOR, full};
use orpheus_update::sim::{image, ofw_header};

use super::*;
use crate::devchan::Link as DevLink;
use crate::mock::{self, MockPlayer, Mode};
use crate::transport::block_on;

struct Blobs {
    ymon: Vec<u8>,
    nordump: Vec<u8>,
    norprog: Vec<u8>,
    norprog0: Vec<u8>,
}

fn blobs() -> Blobs {
    Blobs {
        ymon: mock::payload(mock::TAG_YMON),
        nordump: mock::payload(mock::TAG_NORDUMP),
        norprog: mock::payload(mock::TAG_NORPROG),
        norprog0: mock::payload(mock::TAG_NORPROG0),
    }
}

fn payloads(b: &Blobs) -> Payloads<'_> {
    Payloads { ymon: &b.ymon, nordump: &b.nordump, norprog: &b.norprog, norprog0: Some(&b.norprog0) }
}

fn release_files() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let fw = image(7, 0x3000, 2);
    let mut s2 = STAGE2_MAGIC.to_le_bytes().to_vec();
    s2.resize(700, 0x5A);
    let mut s1 = vec![0x11u8; STAGE1_LEN];
    s1[2] = 0xA5;
    (fw, s2, s1)
}

fn ofw_file(img: &[u8], version: &str) -> Vec<u8> {
    let mut f = ofw_header(img, version, "Fixes").to_vec();
    f.extend_from_slice(img);
    f
}

fn noop(_: &str, _: u64, _: u64) {}

#[test]
fn backup_reads_twice_and_catches_a_bad_read() {
    let b = blobs();
    let p = payloads(&b);
    let m = MockPlayer::stock(3);
    let want = m.chip();
    let mut d = Link::new(m);
    let got = block_on(backup(&mut d, &p, &mut noop)).unwrap();
    assert_eq!(got, want);
    assert_eq!(classify(&got), ChipKind::Stock);
    d.t.flaky_read = true;
    let e = block_on(backup(&mut d, &p, &mut noop)).unwrap_err();
    assert!(e.to_string().contains("differ"), "{e}");
}

fn installed() -> (MockPlayer, Vec<u8>, Vec<u8>) {
    let b = blobs();
    let p = payloads(&b);
    let (fw, s2, s1) = release_files();
    let mut d = Link::new(MockPlayer::stock(5));
    let bk = block_on(backup(&mut d, &p, &mut noop)).unwrap();
    let permit = Sector0Permit::new(&bk, true).unwrap();
    let rel = Release { firmware: &fw, stage2: &s2, stage1: &s1 };
    block_on(first_install(&mut d, &p, &bk, &rel, &permit, &mut noop)).unwrap();
    (d.t, bk, fw)
}

#[test]
fn first_install_from_stock() {
    let (mut m, bk, fw) = installed();
    let (_, s2, s1) = release_files();
    let chip = m.chip();
    assert_eq!(chip[..STAGE1_LEN], s1[..]);
    assert_eq!(chip[STAGE1_LEN..SECTOR as usize], bk[STAGE1_LEN..SECTOR as usize]);
    let s = STAGE2_SECTOR as usize;
    assert_eq!(chip[s..s + 0x400], bk[s..s + 0x400], "vendor record copy kept");
    assert_eq!(chip[s + 0x400..s + 0x400 + s2.len()], s2[..]);
    assert_eq!(chip[0x1000..0x1000 + fw.len()], fw[..]);
    let end = (0x1000 + fw.len()).div_ceil(0x1000) * 0x1000;
    assert!(chip[end..s].iter().all(|&b| b == 0xFF), "the original firmware is erased");
    assert_eq!(chip[full::FREE_END as usize..], bk[full::FREE_END as usize..], "vendor data untouched");
    assert_eq!((m.violations, m.sector0_writes > 0), (0, true));
    m.boot();
    assert_eq!((m.mode, m.booted()), (Mode::Firmware, Some(0)));
    assert_eq!(classify(&m.chip()), ChipKind::OrpheusFull);
}

#[test]
fn first_install_refuses_a_player_that_is_not_stock() {
    let b = blobs();
    let p = payloads(&b);
    let (fw, s2, s1) = release_files();
    let (m, _, _) = installed();
    let mut d = Link::new(m);
    d.t.mode = Mode::Adfu;
    let bk = block_on(backup(&mut d, &p, &mut noop)).unwrap();
    let before = d.t.chip();
    let permit = Sector0Permit::new(&bk, true).unwrap();
    let rel = Release { firmware: &fw, stage2: &s2, stage1: &s1 };
    assert!(block_on(first_install(&mut d, &p, &bk, &rel, &permit, &mut noop)).is_err());
    assert_eq!(d.t.chip(), before);
    // And bad release files are refused before anything is written.
    let mut d = Link::new(MockPlayer::stock(6));
    let bk = block_on(backup(&mut d, &p, &mut noop)).unwrap();
    let before = d.t.chip();
    for rel in [
        Release { firmware: &fw[..0x800], stage2: &s2, stage1: &s1 },
        Release { firmware: &fw, stage2: &s1, stage1: &s1 },
        Release { firmware: &fw, stage2: &s2, stage1: &s2 },
    ] {
        assert!(block_on(first_install(&mut d, &p, &bk, &rel, &permit, &mut noop)).is_err());
    }
    assert_eq!(d.t.chip(), before);
}

#[test]
fn an_interrupted_first_install_resumes() {
    let (want, _, _) = installed();
    let b = blobs();
    let p = payloads(&b);
    let (fw, s2, s1) = release_files();
    for cut in [5, 40, 120, 400, 700] {
        let mut d = Link::new(MockPlayer::stock(5));
        let bk = block_on(backup(&mut d, &p, &mut noop)).unwrap();
        let permit = Sector0Permit::new(&bk, true).unwrap();
        let rel = Release { firmware: &fw, stage2: &s2, stage1: &s1 };
        d.t.fail_call = Some(d.t.calls + cut);
        let first = block_on(first_install(&mut d, &p, &bk, &rel, &permit, &mut noop));
        assert!(first.is_err(), "cut {cut} did not interrupt");
        // Whatever happened, the player boots Orpheus or stays in USB
        // recovery (stage 2 finds no complete slot).
        let mut probe = MockPlayer::new();
        probe.nor.data.clone_from(&d.t.nor.data);
        probe.boot();
        assert!(probe.mode == Mode::Adfu || probe.booted() == Some(0), "cut {cut}");
        d.t.fail_call = None;
        block_on(first_install(&mut d, &p, &bk, &rel, &permit, &mut noop)).unwrap();
        assert_eq!(d.t.chip(), want.chip(), "cut {cut}");
        assert_eq!(d.t.violations, 0);
    }
}

#[test]
fn restore_puts_the_backup_back_except_the_vendor_data() {
    let b = blobs();
    let p = payloads(&b);
    let (mut m, bk, _) = installed();
    // The original firmware changed its data at the top after the backup.
    m.nor.data[0x7F_0000] ^= 0xFF;
    let top = m.chip()[full::FREE_END as usize..].to_vec();
    let mut d = Link::new(m);
    let permit = Sector0Permit::new(&bk, true).unwrap();
    let n = block_on(restore(&mut d, &p, &bk, &permit, &mut noop)).unwrap();
    assert!(n > 1);
    let chip = d.t.chip();
    assert_eq!(chip[..full::FREE_END as usize], bk[..full::FREE_END as usize]);
    assert_eq!(chip[full::FREE_END as usize..], top[..], "never written");
    assert_eq!(d.t.violations, 0);
    // Again: nothing left to write.
    assert_eq!(block_on(restore(&mut d, &p, &bk, &permit, &mut noop)).unwrap(), 0);
    // A backup of an Orpheus chip is no way back.
    let (m2, _, _) = installed();
    let orpheus = m2.chip();
    assert!(block_on(restore(&mut d, &p, &orpheus, &permit, &mut noop)).is_err());
}

#[test]
fn guards_refuse_sector0_and_the_vendor_data() {
    let b = blobs();
    let p = payloads(&b);
    let mut d = Link::new(MockPlayer::stock(9));
    block_on(d.ensure_ymon(&b.ymon)).unwrap();
    let before = d.t.chip();
    let sector = vec![0u8; SECTOR as usize];
    for (addr, len) in
        [(0u32, 0x1000usize), (0x800, 0x1000), (0x7DF000, 0x2000), (0x7E0000, 0x1000), (0x7FF000, 0x1000)]
    {
        assert!(
            block_on(nor::write_full(&mut d, &p, addr, &vec![0u8; len], &mut noop)).is_err(),
            "{addr:#x}"
        );
    }
    assert!(block_on(nor::write_dual(&mut d, &p, 0x1000, &sector, &mut noop)).is_err());
    assert!(block_on(nor::erase_full(&mut d, &p, 0, 0x1000, &mut noop)).is_err());
    assert!(block_on(nor::erase_full(&mut d, &p, 0x7DF000, 0x2000, &mut noop)).is_err());
    assert!(block_on(program_bits(&mut d, &p, 0, &[0; 4])).is_err());
    assert!(block_on(program_bits(&mut d, &p, 0x7E0000, &[0; 4])).is_err());
    assert!(block_on(write_slot(&mut d, &p, 0, &sector, &mut noop)).is_err());
    // No permit without a whole backup and a yes.
    assert!(Sector0Permit::new(&before[..0x1000], true).is_none());
    assert!(Sector0Permit::new(&before, false).is_none());
    // The sector-0 path writes sector 0 only.
    let permit = Sector0Permit::new(&before, true).unwrap();
    assert!(block_on(nor::write_sector0(&mut d, &p, &vec![0u8; 0x2000], &permit, &mut noop)).is_err());
    assert_eq!(d.t.chip(), before, "nothing was written");
    // The payload itself refuses too (if the host-side guard were bypassed).
    block_on(d.load_payload(&b.norprog)).unwrap();
    for (op, addr) in [(1u32, 0u32), (1, 0x7E0000), (2, 0x10)] {
        let r = block_on(d.call(orpheus_proto::mem::YMON_PAYLOAD, &[op, addr, 4], 1000)).unwrap();
        assert_eq!(r[..4], 0xBADu32.to_le_bytes());
    }
    assert_eq!(d.t.chip(), before);
    assert_eq!(d.t.violations, 3, "the mock counted the attempts");
}

#[test]
fn update_through_the_developer_channel_then_switch_back() {
    let old = image(1, 0x2000, 2);
    let new = image(2, 0x2800, 3);
    let mut d = DevLink::new(MockPlayer::new().with_orpheus(&old));
    assert_eq!(d.t.booted(), Some(0));
    let file = ofw_file(&new, "0.2.0");
    // A damaged file never reaches the player.
    let mut bad = file.clone();
    let n = bad.len();
    bad[n - 100] ^= 1;
    assert!(block_on(d.update(&bad, &mut noop)).is_err());
    assert_eq!(d.t.booted(), Some(0));
    let slot = block_on(d.update(&file, &mut noop)).unwrap();
    assert_eq!(slot, 1);
    block_on(d.reboot(REBOOT_NORMAL));
    assert_eq!(d.t.booted(), Some(1));
    let info = block_on(d.info()).unwrap();
    assert_eq!(info.build_id[0], 2);
    // The new firmware keeps failing: recovery switches back to the old one.
    block_on(d.reboot(REBOOT_ADFU));
    let b = blobs();
    let p = payloads(&b);
    let mut a = Link::new(d.t);
    let s = block_on(slots(&mut a, &p)).unwrap();
    assert!(s.can_switch);
    assert!(matches!(s.a, SlotState::Superseded(_)) && matches!(s.b, SlotState::Valid(_)));
    block_on(switch_slot(&mut a, &p)).unwrap();
    assert_eq!(a.t.booted(), Some(0));
    block_on(switch_slot(&mut a, &p)).unwrap();
    assert_eq!(a.t.booted(), Some(1));
    assert_eq!(a.t.violations, 0);
    // And a fresh copy of a firmware into slot A starts first.
    let third = image(3, 0x1800, 1);
    block_on(reinstall(&mut a, &p, &third, &mut noop)).unwrap();
    assert_eq!(a.t.booted(), Some(0));
    assert_eq!(a.t.nor.data[0x1000..0x1000 + third.len()], third[..]);
    assert_eq!(a.t.chip().len(), NOR_SIZE as usize);
}

#[test]
fn update_refused_by_the_player_reports_why() {
    let old = image(1, 0x2000, 2);
    let mut d = DevLink::new(MockPlayer::new().with_orpheus(&old));
    // A file built for the dual-boot layout (its slot header says so).
    let mut img = image(4, 0x2000, 1);
    let mut h = SlotHeader::from_bytes(&img).unwrap();
    h.layout = orpheus_proto::slot::LAYOUT_DUAL;
    img[..128].copy_from_slice(&h.to_bytes());
    let file = ofw_file(&img, "0.3.0");
    let e = block_on(d.update(&file, &mut noop)).unwrap_err();
    assert!(e.to_string().contains("layout"), "{e}");
    assert_eq!(d.t.booted(), Some(0));
    assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}
