//! `xtask dev stage` / `install-stage2`: put orpheus into the free NOR
//! region (dual-boot layout, 02 §2.4). Nothing below 0x762000 is written.

use anyhow::{Context, Result, bail};
use orpheus_host::nor;
use orpheus_proto::hash::{crc32, sum32};
use orpheus_proto::layout::{NOR_SIZE, SECTOR, STAGE2_MAGIC, dual, full};
use orpheus_proto::slot::{LAYOUT_DUAL, LAYOUT_FULL, SlotHeader};

use super::{backup_verify, to_ymon};
use crate::args::Args;
use crate::util::log;

/// Wrap a resident-only flat image (and an optional page store) in a YFW1
/// slot image.
pub fn slot_image(resident: &[u8], pages: &[u8], code_pages: u32, rodata_pages: u32) -> Vec<u8> {
    let res_off = 0x1000u32;
    let pages_off = (res_off + resident.len() as u32).div_ceil(0x1000) * 0x1000;
    let mut h = SlotHeader {
        layout: LAYOUT_DUAL,
        resident_off: res_off,
        resident_len: resident.len() as u32,
        load_addr: 0x0010_0000,
        entry: 0x0010_0001,
        resident_sum: sum32(resident),
        pages_off,
        code_pages,
        rodata_pages,
        pages_crc: crc32(pages),
        res_off: 0,
        res_len: 0,
        verified: 0xFFFF_FFFF,
        build_id: [0; 16],
    };
    let id = crc32(resident).to_le_bytes();
    h.build_id[..4].copy_from_slice(&id);
    let mut out = vec![0xFFu8; pages_off as usize + pages.len()];
    out[..128].copy_from_slice(&h.to_bytes());
    out[res_off as usize..res_off as usize + resident.len()].copy_from_slice(resident);
    out[pages_off as usize..].copy_from_slice(pages);
    out
}

/// The orpheus NOR payload (`fw/payloads/norprog`), built and flattened.
pub fn norprog_payload() -> Result<Vec<u8>> {
    let b = crate::build::build_fw("payload-norprog", "payload-norprog", true, None)?;
    let (img, base) = crate::build::flatten(&b.elf)?;
    if base != orpheus_proto::mem::YMON_PAYLOAD || img.len() > 0x1800 {
        bail!("NOR payload invalid: base {base:#x}, {} bytes", img.len());
    }
    Ok(img)
}

/// Which layout the player has: full when slot A of the full layout holds
/// a valid header.
pub fn player_layout(d: &mut orpheus_host::adfu::Adfu) -> Result<u32> {
    let (h, _) = nor::read(d, full::SLOT_A, 128)?;
    Ok(if SlotHeader::from_bytes(&h).is_ok() { LAYOUT_FULL } else { LAYOUT_DUAL })
}

/// Set the layout id in a slot image's header (and its CRC).
fn set_layout(slot: &mut [u8], layout: u32) -> Result<()> {
    let mut h = SlotHeader::from_bytes(&slot[..128]).map_err(|e| anyhow::anyhow!("slot header: {e:?}"))?;
    h.layout = layout;
    slot[..128].copy_from_slice(&h.to_bytes());
    Ok(())
}

fn read_slot(what: &str) -> Result<Vec<u8>> {
    Ok(if what.ends_with(".yfw") {
        std::fs::read(what)?
    } else {
        let bin = if what.ends_with(".bin") {
            std::fs::read(what)?
        } else {
            std::fs::read(crate::build::build_bringup(what, true)?.bin)?
        };
        slot_image(&bin, &[], 0, 0)
    })
}

pub fn cmd_stage(args: &mut Args) -> Result<String> {
    let what = args.positional("IMAGE.bin, IMAGE.yfw or bring-up name")?;
    let mut slot = read_slot(&what)?;
    backup_verify()?;
    let payload = norprog_payload()?;
    let mut d = to_ymon()?;
    if player_layout(&mut d)? == LAYOUT_FULL {
        if slot.len() as u32 > full::SLOT_A_SIZE {
            bail!("slot image {} bytes > full slot A {} bytes", slot.len(), full::SLOT_A_SIZE);
        }
        set_layout(&mut slot, LAYOUT_FULL)?;
        nor::write_full(&mut d, &payload, full::SLOT_A, &slot, &mut |s| log(s))?;
        return Ok(format!("staged {} bytes into full slot A at {:#x}", slot.len(), full::SLOT_A));
    }
    if slot.len() as u32 > dual::SLOT_A_SIZE {
        bail!("slot image {} bytes > slot A {} bytes", slot.len(), dual::SLOT_A_SIZE);
    }
    nor::write(&mut d, dual::SLOT_A, &slot, &mut |s| log(s))?;
    Ok(format!("staged {} bytes into slot A at {:#x}", slot.len(), dual::SLOT_A))
}

/// Switch to the full layout (02 §2.4, decision D-2): firmware into full
/// slot A over the stock firmware, then erase the rest of the stock
/// firmware and the old dual-boot slot. Sector 0, the settings store, the
/// vault, the boot sector and the vendor data above 0x7E0000 stay.
pub fn cmd_install_full(args: &mut Args) -> Result<String> {
    if !args.flag("i-have-a-backup") {
        bail!("install-full erases the stock firmware; pass --i-have-a-backup (decision D-2)");
    }
    let what = args.next_positional().unwrap_or_else(|| "target/orpheus/firmware.yfw".into());
    let mut slot = read_slot(&what)?;
    if slot.len() as u32 > full::SLOT_A_SIZE {
        bail!("slot image {} bytes > full slot A {} bytes", slot.len(), full::SLOT_A_SIZE);
    }
    set_layout(&mut slot, LAYOUT_FULL)?;
    backup_verify()?;
    let before = crate::util::root().join("local/nor-before-full.bin");
    if std::fs::metadata(&before).map(|m| m.len()).unwrap_or(0) != u64::from(NOR_SIZE) {
        bail!(
            "no dump of the current NOR in local/nor-before-full.bin; run `xtask dev nordump 0 0x800000 local/nor-before-full.bin` first"
        );
    }
    // Stage 2 must be the one that tries the full-layout slots.
    let b = crate::build::build_fw("stage2", "stage2", true, None)?;
    let (s2, _) = crate::build::flatten(&b.elf)?;
    let payload = norprog_payload()?;
    let mut d = to_ymon()?;
    let (cur, _) = nor::read(&mut d, dual::STAGE2, s2.len())?;
    if cur != s2 {
        bail!(
            "stage 2 in NOR is not the current build (it may not know the full layout); run `xtask dev install-stage2` first"
        );
    }
    log("writing the firmware into full slot A (over the stock firmware)");
    nor::write_full(&mut d, &payload, full::SLOT_A, &slot, &mut |s| log(s))?;
    let start = (full::SLOT_A + slot.len() as u32).div_ceil(SECTOR) * SECTOR;
    log(&format!(
        "erasing the rest of the stock firmware and the old slot: {start:#x}..{:#x}",
        dual::SETTINGS
    ));
    nor::erase_full(&mut d, &payload, start, dual::SETTINGS - start, &mut |s| log(s))?;
    Ok(format!(
        "full layout installed: firmware {} bytes at {:#x}; {start:#x}..{:#x} erased; settings, vault, boot sector kept",
        slot.len(),
        full::SLOT_A,
        dual::SETTINGS
    ))
}

/// `restore vendor [--verify]`: compare the NOR with the stock backup, or
/// write back every differing sector (sector 0 with the reference repo's
/// sector-0 payload) — the way back from the full layout.
pub fn cmd_restore_vendor(args: &mut Args) -> Result<String> {
    let verify = args.flag("verify");
    backup_verify()?;
    let backup = std::fs::read(super::DUMPS.join("nor_backup_8m.bin")).context("read the stock backup")?;
    let mut d = to_ymon()?;
    let (cur, _) = nor::read(&mut d, 0, NOR_SIZE as usize)?;
    let diff: Vec<u32> = (0..NOR_SIZE / SECTOR)
        .filter(|&i| {
            let r = (i * SECTOR) as usize..((i + 1) * SECTOR) as usize;
            cur[r.clone()] != backup[r]
        })
        .map(|i| i * SECTOR)
        .collect();
    if verify {
        if diff.is_empty() {
            return Ok("NOR equals the stock backup".into());
        }
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for &a in &diff {
            match runs.last_mut() {
                Some(r) if r.1 == a => r.1 = a + SECTOR,
                _ => runs.push((a, a + SECTOR)),
            }
        }
        for (a, b) in &runs {
            log(&format!("  differs: {a:#x}..{b:#x}"));
        }
        return Ok(format!("{} sectors differ from the stock backup", diff.len()));
    }
    if !args.flag("i-have-a-backup") {
        bail!("restore vendor rewrites the whole chip including sector 0; pass --i-have-a-backup");
    }
    // The vendor data above 0x7E0000 is never written by orpheus; where it
    // differs from the backup, the stock firmware updated it after the
    // backup was taken (checked on 2026-11-24), so the newer data stays.
    let payload = norprog_payload()?;
    for &a in diff.iter().filter(|&&a| a != 0 && a < full::FREE_END) {
        let r = a as usize..(a + SECTOR) as usize;
        nor::write_full(&mut d, &payload, a, &backup[r], &mut |_| {})?;
    }
    log(&format!(
        "{} sectors restored above sector 0",
        diff.iter().filter(|&&a| a != 0 && a < full::FREE_END).count()
    ));
    if diff.first() == Some(&0) {
        // Sector 0 last: until then stage 1 + stage 2 keep USB recovery.
        nor::write_sector0(&mut d, &backup[..SECTOR as usize], &mut |s| log(s))?;
    }
    Ok("stock firmware restored; the player boots it on the next reset".into())
}

pub fn cmd_install_stage2(_args: &mut Args) -> Result<String> {
    let b = crate::build::build_fw("stage2", "stage2", true, None)?;
    let (img, base) = crate::build::flatten(&b.elf)?;
    if base != orpheus_proto::layout::STAGE2_LOAD {
        bail!("stage 2 links at {base:#x}, expected 0x102400");
    }
    if img.len() > dual::STAGE2_MAX as usize
        || u32::from_le_bytes([img[0], img[1], img[2], img[3]]) != STAGE2_MAGIC
    {
        bail!("stage 2 image invalid ({} bytes)", img.len());
    }
    backup_verify()?;
    let mut d = to_ymon()?;
    let (cur, _) = nor::read(&mut d, dual::BOOT_SECTOR, 0x1000)?;
    if cur[2] != 0xA5 {
        bail!(
            "vendor boot record copy at 0x7DF000 looks wrong (byte 2 = {:#x}); not touching the sector",
            cur[2]
        );
    }
    let mut sector = cur.clone();
    sector[0x400..0x800].fill(0xFF);
    sector[0x400..0x400 + img.len()].copy_from_slice(&img);
    let old_s2 = &cur[0x400..0x800];
    std::fs::write(crate::util::root().join("local/stage2-previous.bin"), old_s2)
        .context("save old stage 2")?;
    nor::write(&mut d, dual::BOOT_SECTOR, &sector, &mut |s| log(s))?;
    Ok(format!(
        "stage 2 ({} bytes) installed at 0x7DF400; previous one saved in local/stage2-previous.bin",
        img.len()
    ))
}

/// Run the freshly built stage 2 from RAM (as stage 1 would: loaded at
/// 0x102400, entered at +0x11) to prove its paths before it goes to NOR.
/// `--adfu` sets the mailbox ADFU word first.
pub fn cmd_stage2_ramtest(args: &mut Args) -> Result<String> {
    let adfu = args.flag("adfu");
    let b = crate::build::build_fw("stage2", "stage2", true, None)?;
    let (img, base) = crate::build::flatten(&b.elf)?;
    if base != orpheus_proto::layout::STAGE2_LOAD || img.len() > 1024 {
        bail!("stage 2 image invalid");
    }
    let mut d = to_ymon()?;
    let mut mb = orpheus_proto::mailbox::Mailbox::default();
    if adfu {
        mb.adfu = orpheus_proto::mailbox::ADFU_MAGIC;
    }
    d.write(orpheus_proto::mailbox::BASE, &mb.to_bytes())?;
    d.write(orpheus_proto::result::BASE, &[0u8; orpheus_proto::result::SIZE])?;
    d.write(orpheus_proto::fault::BASE, &[0u8; orpheus_proto::fault::SIZE])?;
    d.write_verify(base, &img, 0x200)?;
    d.jump(base + 0x10)?;
    d.close();
    log("stage 2 started from RAM; watching the bus");
    let t0 = std::time::Instant::now();
    let mut last = orpheus_host::device::State::Ymon;
    while t0.elapsed() < std::time::Duration::from_secs(40) {
        let s = orpheus_host::device::state();
        if s != last {
            log(&format!("  {:5.1} s: {}", t0.elapsed().as_secs_f32(), s.name()));
            last = s;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Ok(format!("final state: {}", last.name()))
}
