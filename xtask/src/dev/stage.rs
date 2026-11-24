//! `xtask dev stage` / `install-stage2`: put yotonrs into the free NOR
//! region (dual-boot layout, 02 §2.4). Nothing below 0x762000 is written.

use anyhow::{Context, Result, bail};
use yoton_host::nor;
use yoton_proto::hash::{crc32, sum32};
use yoton_proto::layout::{STAGE2_MAGIC, dual};
use yoton_proto::slot::{LAYOUT_DUAL, SlotHeader};

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

pub fn cmd_stage(args: &mut Args) -> Result<String> {
    let what = args.positional("IMAGE.bin, IMAGE.yfw or bring-up name")?;
    let slot = if what.ends_with(".yfw") {
        std::fs::read(&what)?
    } else {
        let bin = if what.ends_with(".bin") {
            std::fs::read(&what)?
        } else {
            std::fs::read(crate::build::build_bringup(&what, true)?.bin)?
        };
        slot_image(&bin, &[], 0, 0)
    };
    if slot.len() as u32 > dual::SLOT_A_SIZE {
        bail!("slot image {} bytes > slot A {} bytes", slot.len(), dual::SLOT_A_SIZE);
    }
    backup_verify()?;
    let mut d = to_ymon()?;
    nor::write(&mut d, dual::SLOT_A, &slot, &mut |s| log(s))?;
    Ok(format!("staged {} bytes into slot A at {:#x}", slot.len(), dual::SLOT_A))
}

pub fn cmd_install_stage2(_args: &mut Args) -> Result<String> {
    let b = crate::build::build_fw("stage2", "stage2", true, None)?;
    let (img, base) = crate::build::flatten(&b.elf)?;
    if base != yoton_proto::layout::STAGE2_LOAD {
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
    if base != yoton_proto::layout::STAGE2_LOAD || img.len() > 1024 {
        bail!("stage 2 image invalid");
    }
    let mut d = to_ymon()?;
    if adfu {
        d.write_u32(yoton_proto::mailbox::ADDR_ADFU, yoton_proto::mailbox::ADFU_MAGIC)?;
    }
    d.write_verify(base, &img, 0x200)?;
    d.jump(base + 0x10)?;
    d.close();
    log("stage 2 started from RAM; watching the bus");
    let t0 = std::time::Instant::now();
    let mut last = yoton_host::device::State::Ymon;
    while t0.elapsed() < std::time::Duration::from_secs(40) {
        let s = yoton_host::device::state();
        if s != last {
            log(&format!("  {:5.1} s: {}", t0.elapsed().as_secs_f32(), s.name()));
            last = s;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Ok(format!("final state: {}", last.name()))
}
