//! NOR flash access through RAM payloads run by ymon.
//!
//! The payloads: `nordump` (read; the reference repository's C payload),
//! `payload-norprog` (erase/program for the full layout, Rust,
//! `fw/payloads/norprog`, with the HAL's guard), `norprog0` (sector 0
//! only, the reference repository's). The bytes are passed in, so the
//! browser can use the same code with the release's copies.
//!
//! **Guards** (CLAUDE.md rules 1, 09 §5), checked here before the payload,
//! which checks again:
//! * [`write_full`] / [`erase_full`]: only `0x1000..0x7E0000`
//!   (`layout::full_writable`): never sector 0, never the vendor data;
//! * [`write_dual`]: only the dual-boot free region;
//! * sector 0 only through [`write_sector0`], which needs a
//!   [`Sector0Permit`]: made from a whole-chip backup and an explicit
//!   confirmation.

use anyhow::{Result, bail};
use orpheus_proto::layout::{NOR_SIZE, SECTOR, dual_writable, full_writable};
use orpheus_proto::mem::{YMON_ARGS, YMON_PAYLOAD};

use crate::adfu::Link;
use crate::transport::{Progress, Transport};

/// Data buffer the payloads use.
pub const BUF: u32 = 0x0012_2000;
/// Bytes per `nordump` call.
pub const CHUNK: usize = 0x8000;

/// The payload images (bytes as loaded at `YMON_PAYLOAD`).
#[derive(Clone, Copy)]
pub struct Payloads<'a> {
    pub ymon: &'a [u8],
    pub nordump: &'a [u8],
    pub norprog: &'a [u8],
    /// The sector-0 writer (first install, restore only).
    pub norprog0: Option<&'a [u8]>,
}

/// Permission to rewrite NOR sector 0 (the boot record). Exists only when
/// a whole-chip backup is at hand and the person confirmed the step
/// (`xtask dev restore vendor --i-have-a-backup`, the browser installer's
/// confirmed first-install and restore steps).
pub struct Sector0Permit {
    _private: (),
}

impl Sector0Permit {
    /// `backup`: the whole chip as read before anything was written;
    /// `confirmed`: the person said yes to this very step.
    #[must_use]
    pub fn new(backup: &[u8], confirmed: bool) -> Option<Sector0Permit> {
        (confirmed && backup.len() == NOR_SIZE as usize).then_some(Sector0Permit { _private: () })
    }
}

/// Read NOR bytes (raw, not descrambled) with `nordump`. Returns data and
/// the JEDEC id.
pub async fn read<T: Transport>(
    d: &mut Link<T>,
    nordump: &[u8],
    addr: u32,
    len: usize,
    progress: Progress<'_>,
) -> Result<(Vec<u8>, u32)> {
    d.load_payload(nordump).await?;
    let mut out = Vec::with_capacity(len);
    let mut jedec = 0;
    while out.len() < len {
        let n = (len - out.len()).min(CHUNK);
        let a = addr + out.len() as u32;
        let chunk = d.call(YMON_PAYLOAD, &[a, n as u32, 0, 0, 0, 0, 0], 20_000).await?;
        if chunk.len() != n {
            bail!("nordump returned {} bytes for {n}", chunk.len());
        }
        if jedec == 0 {
            jedec = d.read_u32(YMON_ARGS + 12).await?;
        }
        out.extend_from_slice(&chunk);
        progress("read", out.len() as u64, len as u64);
    }
    Ok((out, jedec))
}

fn status(r: &[u8]) -> u32 {
    r.get(..4).map_or(u32::MAX, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Erase and program sector-aligned `data` at `addr` with `payload`,
/// skipping sectors that already hold the data (so a repeated run resumes),
/// and verify by reading back. `allow` is the region check.
async fn write_with<T: Transport>(
    d: &mut Link<T>,
    p: &Payloads<'_>,
    prog: &[u8],
    addr: u32,
    data: &[u8],
    allow: fn(u32, u32) -> bool,
    progress: Progress<'_>,
) -> Result<()> {
    if !addr.is_multiple_of(SECTOR) {
        bail!("NOR write address {addr:#x} is not sector aligned");
    }
    let mut data = data.to_vec();
    data.resize(data.len().div_ceil(SECTOR as usize) * SECTOR as usize, 0xFF);
    if data.is_empty() || !allow(addr, data.len() as u32) {
        bail!("refusing NOR write {addr:#x}+{:#x}", data.len());
    }
    let (cur, _) = read(d, p.nordump, addr, data.len(), &mut |_, _, _| {}).await?;
    d.load_payload(prog).await?;
    let total = data.len() as u64;
    for (si, sector) in data.chunks(SECTOR as usize).enumerate() {
        let sa = addr + (si as u32) * SECTOR;
        let off = si * SECTOR as usize;
        if cur[off..off + SECTOR as usize] == *sector {
            continue;
        }
        if !allow(sa, SECTOR) {
            bail!("refusing NOR erase at {sa:#x}");
        }
        let r = d.call(YMON_PAYLOAD, &[1, sa, 0], 10_000).await?;
        if status(&r) != 0 {
            bail!("erase at {sa:#x} failed: status {:#x}", status(&r));
        }
        for (pi, page) in sector.chunks(256).enumerate() {
            if page.iter().all(|&b| b == 0xFF) {
                continue;
            }
            let pa = sa + (pi as u32) * 256;
            d.write(BUF, page).await?;
            let r = d.call(YMON_PAYLOAD, &[2, pa, page.len() as u32], 10_000).await?;
            if status(&r) != 0 {
                bail!("program at {pa:#x} failed: status {:#x}", status(&r));
            }
        }
        progress("write", off as u64 + u64::from(SECTOR), total);
    }
    let (back, _) = read(d, p.nordump, addr, data.len(), &mut |_, _, _| {}).await?;
    if back != data {
        let i = back.iter().zip(&data).position(|(a, b)| a != b).unwrap_or(0);
        bail!("NOR verify FAILED at {:#x}", addr as usize + i);
    }
    Ok(())
}

/// Write anywhere the full layout allows (`0x1000..0x7E0000`).
pub async fn write_full<T: Transport>(
    d: &mut Link<T>,
    p: &Payloads<'_>,
    addr: u32,
    data: &[u8],
    progress: Progress<'_>,
) -> Result<()> {
    write_with(d, p, p.norprog, addr, data, full_writable, progress).await
}

/// Write inside the dual-boot free region (`0x762000..0x7E0000`).
pub async fn write_dual<T: Transport>(
    d: &mut Link<T>,
    p: &Payloads<'_>,
    addr: u32,
    data: &[u8],
    progress: Progress<'_>,
) -> Result<()> {
    write_with(d, p, p.norprog, addr, data, dual_writable, progress).await
}

/// Rewrite NOR sector 0 (the boot record) with the sector-0 payload.
pub async fn write_sector0<T: Transport>(
    d: &mut Link<T>,
    p: &Payloads<'_>,
    data: &[u8],
    _permit: &Sector0Permit,
    progress: Progress<'_>,
) -> Result<()> {
    if data.len() != SECTOR as usize {
        bail!("sector 0 image must be exactly one sector");
    }
    let Some(prog) = p.norprog0 else { bail!("no sector-0 payload") };
    write_with(d, p, prog, 0, data, |a, l| a == 0 && l == SECTOR, progress).await
}

/// Erase `[addr, addr + len)` (sector aligned, full-layout guard), skipping
/// sectors that already read back erased.
pub async fn erase_full<T: Transport>(
    d: &mut Link<T>,
    p: &Payloads<'_>,
    addr: u32,
    len: u32,
    progress: Progress<'_>,
) -> Result<u32> {
    if !addr.is_multiple_of(SECTOR) || !len.is_multiple_of(SECTOR) || !full_writable(addr, len) {
        bail!("refusing NOR erase {addr:#x}+{len:#x}");
    }
    let mut erased = 0;
    let mut a = addr;
    while a < addr + len {
        let n = (addr + len - a).min(CHUNK as u32);
        let (cur, _) = read(d, p.nordump, a, n as usize, &mut |_, _, _| {}).await?;
        d.load_payload(p.norprog).await?;
        for (i, s) in cur.chunks(SECTOR as usize).enumerate() {
            if s.iter().all(|&b| b == 0xFF) {
                continue;
            }
            let sa = a + i as u32 * SECTOR;
            let r = d.call(YMON_PAYLOAD, &[1, sa, 0], 10_000).await?;
            if status(&r) != 0 {
                bail!("erase at {sa:#x} failed: status {:#x}", status(&r));
            }
            erased += 1;
        }
        a += n;
        progress("erase", u64::from(a - addr), u64::from(len));
    }
    let (back, _) = read(d, p.nordump, addr, len as usize, &mut |_, _, _| {}).await?;
    if let Some(i) = back.iter().position(|&b| b != 0xFF) {
        bail!("NOR erase verify FAILED at {:#x}", addr as usize + i);
    }
    Ok(erased)
}
