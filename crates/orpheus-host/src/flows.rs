//! The installer's flows (WEB-3, 09 §5), shared by the browser and the
//! tests: backup, first install, restore stock, recovery. The update of a
//! running Orpheus is [`crate::devchan::Link::update`] (no ADFU).
//!
//! The rules of 09 §5 / CLAUDE.md, as enforced here:
//! * **Backup first**: [`backup`] reads the whole chip twice and refuses
//!   unless both reads agree; first install and restore take that backup
//!   and refuse a chip that does not look like it.
//! * **Sector 0** only in the first install's boot-record step and the
//!   restore's last steps, and only with a [`Sector0Permit`] (the backup
//!   plus the person's confirmation of that step).
//! * **Never `0x7E0000` and above**: every other write is
//!   `nor::write_full`/`erase_full`/[`program_bits`], all limited to
//!   `0x1000..0x7E0000`.
//! * **Resumable**: every write first compares and skips sectors that
//!   already hold the wanted bytes, so running a flow again after an
//!   interruption continues where it stopped.
//! * Release files are checked against the release manifest's SHA-256 by
//!   the caller ([`sha256_hex`]) before they get here.

use anyhow::{Result, bail};
use orpheus_proto::layout::{NOR_SIZE, SECTOR, STAGE2_MAGIC, dual, full, full_writable};
use orpheus_proto::mem::YMON_PAYLOAD;
use orpheus_proto::slot::{self, LAYOUT_FULL, SlotHeader};
use orpheus_update::slots::{self, SUPERSEDED, SlotState, SwitchStep};
use orpheus_update::{Digest, Sha256};

use crate::adfu::Link;
use crate::nor::{self, BUF, Payloads, Sector0Permit};
use crate::transport::{Progress, Transport};

/// JEDEC id of the YM03's NOR (`5E 40 17`) as `nordump` reports it.
pub const JEDEC: u32 = 0x0017_405E;
/// The sector holding the vendor boot-record copy and stage 2.
pub const STAGE2_SECTOR: u32 = dual::BOOT_SECTOR;
/// Size of the boot record stage 1 replaces in sector 0.
pub const STAGE1_LEN: usize = 0x200;

/// Lowercase hex SHA-256 (release manifest entries).
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().fold(String::with_capacity(64), |mut s, b| {
        const H: &[u8; 16] = b"0123456789abcdef";
        s.push(H[usize::from(b >> 4)] as char);
        s.push(H[usize::from(b & 15)] as char);
        s
    })
}

/// What a whole-chip image holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipKind {
    /// The original firmware (vendor boot record, no stage 2).
    Stock,
    /// Orpheus in the full layout (a valid or superseded slot A or B).
    OrpheusFull,
    /// A stage 2 but no full-layout slot (the dual-boot layout, the
    /// Rockbox port, or an interrupted install).
    Other,
}

/// Classify a whole-chip image.
#[must_use]
pub fn classify(chip: &[u8]) -> ChipKind {
    if chip.len() != NOR_SIZE as usize {
        return ChipKind::Other;
    }
    let st = |b: u32| slots::state_of(&chip[b as usize..b as usize + slot::HEADER_SIZE]);
    if st(full::SLOT_A) != SlotState::Empty || st(full::SLOT_B) != SlotState::Empty {
        return ChipKind::OrpheusFull;
    }
    let s2 = dual::STAGE2 as usize;
    let has_stage2 = chip[s2..s2 + 4] == STAGE2_MAGIC.to_le_bytes();
    // A vendor boot record has type 0xA5 at byte 2.
    if !has_stage2 && chip[2] == 0xA5 { ChipKind::Stock } else { ChipKind::Other }
}

/// Read the whole chip twice through ymon and `nordump`; refuse unless both
/// reads agree and the chip is the YM03's. The backup the person keeps.
pub async fn backup<T: Transport>(
    d: &mut Link<T>,
    p: &Payloads<'_>,
    progress: Progress<'_>,
) -> Result<Vec<u8>> {
    d.ensure_ymon(p.ymon).await?;
    let n = NOR_SIZE as usize;
    let (first, jedec) =
        nor::read(d, p.nordump, 0, n, &mut |_, done, _| progress("backup read 1", done, 2 * n as u64))
            .await?;
    if jedec != JEDEC {
        bail!("unexpected flash chip (JEDEC {jedec:06x}); not a YM03?");
    }
    let (second, _) = nor::read(d, p.nordump, 0, n, &mut |_, done, _| {
        progress("backup read 2", n as u64 + done, 2 * n as u64);
    })
    .await?;
    if let Some(i) = first.iter().zip(&second).position(|(a, b)| a != b) {
        bail!(
            "the two reads of the flash differ at {i:#x}: the backup is not reliable; check the cable and try again"
        );
    }
    Ok(first)
}

/// Clear bits in place (no erase) inside the full layout's writable range:
/// the recovery switch's "supersede" step.
pub async fn program_bits<T: Transport>(
    d: &mut Link<T>,
    p: &Payloads<'_>,
    addr: u32,
    data: &[u8],
) -> Result<()> {
    if data.is_empty()
        || data.len() > 256
        || (addr % 256) as usize + data.len() > 256
        || !full_writable(addr, data.len() as u32)
    {
        bail!("refusing NOR program {addr:#x}+{:#x}", data.len());
    }
    let (cur, _) = nor::read(d, p.nordump, addr, data.len(), &mut |_, _, _| {}).await?;
    if cur == data {
        return Ok(());
    }
    if cur.iter().zip(data).any(|(c, n)| n & !c != 0) {
        bail!("program at {addr:#x} would need an erase");
    }
    d.load_payload(p.norprog).await?;
    d.write(BUF, data).await?;
    let r = d.call(YMON_PAYLOAD, &[2, addr, data.len() as u32], 10_000).await?;
    if r.get(..4) != Some(&[0, 0, 0, 0]) {
        bail!("program at {addr:#x} failed");
    }
    let (back, _) = nor::read(d, p.nordump, addr, data.len(), &mut |_, _, _| {}).await?;
    if back != data {
        bail!("program verify failed at {addr:#x}");
    }
    Ok(())
}

/// The files a first install writes.
pub struct Release<'a> {
    /// The `"YFW1"` slot image (the `.ofw`'s image), full layout.
    pub firmware: &'a [u8],
    /// Stage 2 (≤ 1 KiB, `"YST2"`).
    pub stage2: &'a [u8],
    /// Stage 1: the 512-byte boot record for sector 0.
    pub stage1: &'a [u8],
}

fn check_release(r: &Release<'_>) -> Result<()> {
    let h = SlotHeader::from_bytes(r.firmware).map_err(|e| anyhow::anyhow!("firmware image: {e:?}"))?;
    if h.layout != LAYOUT_FULL || r.firmware.len() > full::SLOT_A_SIZE as usize || r.firmware.len() <= 0x1000
    {
        bail!("firmware image is not a full-layout slot image");
    }
    if r.stage2.len() > dual::STAGE2_MAX as usize
        || r.stage2.get(..4) != Some(&STAGE2_MAGIC.to_le_bytes()[..])
    {
        bail!("stage 2 image invalid");
    }
    if r.stage1.len() != STAGE1_LEN || r.stage1[2] != 0xA5 {
        bail!("stage 1 image invalid");
    }
    Ok(())
}

/// A slot image into a full-layout slot: the body first, the header
/// sector last (until then stage 2 does not start the slot).
pub async fn write_slot<T: Transport>(
    d: &mut Link<T>,
    p: &Payloads<'_>,
    base: u32,
    image: &[u8],
    progress: Progress<'_>,
) -> Result<()> {
    if base != full::SLOT_A && base != full::SLOT_B {
        bail!("not a slot: {base:#x}");
    }
    let head = (SECTOR as usize).min(image.len());
    nor::write_full(d, p, base + SECTOR, &image[head..], progress).await?;
    nor::write_full(d, p, base, &image[..head], &mut |_, _, _| {}).await
}

/// First install (from the original firmware, in ADFU): stage 2, then
/// sector 0 (stage 1 + the vendor bytes after it), then Orpheus into slot
/// A (header last), then the rest of the original firmware erased. From
/// the boot-record step on, the player starts stage 2, which starts
/// Orpheus once slot A is complete and USB recovery until then: the order
/// keeps it reachable at every point. `backup`: [`backup`]'s result, taken
/// first; it must show the original firmware.
pub async fn first_install<T: Transport>(
    d: &mut Link<T>,
    p: &Payloads<'_>,
    backup: &[u8],
    rel: &Release<'_>,
    permit: &Sector0Permit,
    progress: Progress<'_>,
) -> Result<()> {
    if classify(backup) != ChipKind::Stock {
        bail!("the backup does not show the original firmware: use Update or Recovery instead");
    }
    check_release(rel)?;
    d.ensure_ymon(p.ymon).await?;
    // 1. Stage 2 beside the vendor record copy's sector.
    let s = STAGE2_SECTOR as usize;
    let mut sector = backup[s..s + SECTOR as usize].to_vec();
    sector[0x400..0x800].fill(0xFF);
    sector[0x400..0x400 + rel.stage2.len()].copy_from_slice(rel.stage2);
    progress("stage 2", 0, 4);
    nor::write_full(d, p, STAGE2_SECTOR, &sector, &mut |_, _, _| {}).await?;
    // 2. The boot record: stage 1, the vendor bytes after it unchanged.
    progress("boot record", 1, 4);
    let mut s0 = backup[..SECTOR as usize].to_vec();
    s0[..STAGE1_LEN].copy_from_slice(rel.stage1);
    nor::write_sector0(d, p, &s0, permit, &mut |_, _, _| {}).await?;
    // 3. Orpheus.
    progress("firmware", 2, 4);
    write_slot(d, p, full::SLOT_A, rel.firmware, progress).await?;
    // 4. The rest of the original firmware, the settings and the vault
    //    (Orpheus starts them fresh); stage 2's sector and the vendor data
    //    above 0x7E0000 stay.
    let start = (full::SLOT_A + rel.firmware.len() as u32).div_ceil(SECTOR) * SECTOR;
    progress("erase", 3, 4);
    nor::erase_full(d, p, start, STAGE2_SECTOR - start, progress).await?;
    progress("done", 4, 4);
    Ok(())
}

/// Restore the original firmware from `backup`: every sector that differs
/// below `0x7E0000`, sector 0 then stage 2's sector last (so until the
/// boot record is back the player keeps starting stage 2, and from then on
/// the original firmware). The vendor data at `0x7E0000` and above is never
/// written. Returns the sectors written.
pub async fn restore<T: Transport>(
    d: &mut Link<T>,
    p: &Payloads<'_>,
    backup: &[u8],
    permit: &Sector0Permit,
    progress: Progress<'_>,
) -> Result<u32> {
    if classify(backup) != ChipKind::Stock {
        bail!("this backup is not of the original firmware");
    }
    d.ensure_ymon(p.ymon).await?;
    let end = full::FREE_END;
    let (cur, _) =
        nor::read(d, p.nordump, 0, end as usize, &mut |_, done, total| progress("compare", done, total))
            .await?;
    let differs = |a: u32| {
        let r = a as usize..(a + SECTOR) as usize;
        cur[r.clone()] != backup[r]
    };
    let body: Vec<u32> =
        (1..end / SECTOR).map(|i| i * SECTOR).filter(|&a| a != STAGE2_SECTOR && differs(a)).collect();
    let mut written = 0u32;
    let total = body.len() as u64 + 2;
    // Runs of differing sectors, written in order.
    let mut i = 0;
    while i < body.len() {
        let mut j = i + 1;
        while j < body.len() && body[j] == body[j - 1] + SECTOR {
            j += 1;
        }
        let a = body[i];
        let b = body[j - 1] + SECTOR;
        nor::write_full(d, p, a, &backup[a as usize..b as usize], &mut |_, _, _| {}).await?;
        written += (j - i) as u32;
        progress("restore", u64::from(written), total);
        i = j;
    }
    if differs(0) {
        nor::write_sector0(d, p, &backup[..SECTOR as usize], permit, &mut |_, _, _| {}).await?;
        written += 1;
    }
    if differs(STAGE2_SECTOR) {
        let s = STAGE2_SECTOR as usize;
        nor::write_full(d, p, STAGE2_SECTOR, &backup[s..s + SECTOR as usize], &mut |_, _, _| {}).await?;
        written += 1;
    }
    let (back, _) =
        nor::read(d, p.nordump, 0, end as usize, &mut |_, done, total| progress("verify", done, total))
            .await?;
    if let Some(at) = back.iter().zip(&backup[..end as usize]).position(|(a, b)| a != b) {
        bail!("restore verify failed at {at:#x}");
    }
    Ok(written)
}

/// The two full-layout slots as recovery shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slots {
    pub a: SlotState,
    pub b: SlotState,
    /// Whether [`switch_slot`] has another image to start.
    pub can_switch: bool,
}

async fn headers<T: Transport>(d: &mut Link<T>, p: &Payloads<'_>) -> Result<(Vec<u8>, Vec<u8>)> {
    let (a, _) = nor::read(d, p.nordump, full::SLOT_A, slot::HEADER_SIZE, &mut |_, _, _| {}).await?;
    let (b, _) = nor::read(d, p.nordump, full::SLOT_B, slot::HEADER_SIZE, &mut |_, _, _| {}).await?;
    Ok((a, b))
}

/// Recovery: what the slots hold.
pub async fn slots<T: Transport>(d: &mut Link<T>, p: &Payloads<'_>) -> Result<Slots> {
    d.ensure_ymon(p.ymon).await?;
    let (a, b) = headers(d, p).await?;
    Ok(Slots { a: slots::state_of(&a), b: slots::state_of(&b), can_switch: slots::switch(&a, &b).is_some() })
}

/// Recovery: make stage 2 start the other slot (after an update that keeps
/// failing to start, which ends in USB recovery). At most two small writes
/// to the slots' header sectors; after each one a complete image starts.
pub async fn switch_slot<T: Transport>(d: &mut Link<T>, p: &Payloads<'_>) -> Result<()> {
    d.ensure_ymon(p.ymon).await?;
    let (a, b) = headers(d, p).await?;
    let Some(steps) = slots::switch(&a, &b) else { bail!("no other firmware to switch to") };
    for s in steps.iter().flatten() {
        match *s {
            SwitchStep::Restore { slot, header } => {
                let mut sector = vec![0xFFu8; SECTOR as usize];
                sector[..header.len()].copy_from_slice(&header);
                nor::write_full(d, p, slot, &sector, &mut |_, _, _| {}).await?;
            }
            SwitchStep::Supersede { slot } => program_bits(d, p, slot, &SUPERSEDED.to_le_bytes()).await?,
        }
    }
    Ok(())
}

/// Recovery: put a firmware image into slot A again (it starts first).
pub async fn reinstall<T: Transport>(
    d: &mut Link<T>,
    p: &Payloads<'_>,
    firmware: &[u8],
    progress: Progress<'_>,
) -> Result<()> {
    let h = SlotHeader::from_bytes(firmware).map_err(|e| anyhow::anyhow!("firmware image: {e:?}"))?;
    if h.layout != LAYOUT_FULL || firmware.len() > full::SLOT_A_SIZE as usize {
        bail!("firmware image is not a full-layout slot image");
    }
    d.ensure_ymon(p.ymon).await?;
    write_slot(d, p, full::SLOT_A, firmware, progress).await
}

#[cfg(test)]
mod tests;
