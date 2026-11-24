//! NOR flash access through RAM payloads run by ymon.
//!
//! Until WP-T-6 replaces them with Rust payloads, these are the C payloads of
//! the reference repository (`nordump.bin`, `norprog.bin`). `norprog`
//! refuses addresses outside `0x762000..0x7E0000`; this module checks the
//! same range first ([`yoton_proto::layout::dual_writable`]).

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use yoton_proto::layout::{SECTOR, dual_writable, full_writable};

use crate::adfu::{Adfu, payload_path};

/// Data buffer the payloads use.
const BUF: u32 = 0x0012_2000;
const CHUNK: usize = 0x8000;

/// Read NOR bytes (raw, not descrambled). Returns data and the JEDEC id.
pub fn read(d: &mut Adfu, addr: u32, len: usize) -> Result<(Vec<u8>, u32)> {
    d.load_payload_file(&payload_path("nordump.bin"))?;
    let mut out = Vec::with_capacity(len);
    let mut jedec = 0;
    while out.len() < len {
        let n = (len - out.len()).min(CHUNK);
        let a = addr + out.len() as u32;
        let chunk =
            d.call(yoton_proto::mem::YMON_PAYLOAD, &[a, n as u32, 0, 0, 0, 0, 0], Duration::from_secs(20))?;
        if chunk.len() != n {
            bail!("nordump returned {} bytes for {n}", chunk.len());
        }
        if jedec == 0 {
            jedec = d.read_u32(yoton_proto::mem::YMON_ARGS + 12)?;
        }
        out.extend_from_slice(&chunk);
    }
    Ok((out, jedec))
}

/// Erase and program `data` at `addr` (sector aligned) and verify by reading
/// back. Only inside the dual-boot free region.
pub fn write(d: &mut Adfu, addr: u32, data: &[u8], log: &mut dyn FnMut(&str)) -> Result<()> {
    if !dual_writable(addr, data.len().div_ceil(SECTOR as usize) as u32 * SECTOR) {
        bail!("refusing NOR write outside 0x762000..0x7E0000: {addr:#x}+{:#x}", data.len());
    }
    let blob = std::fs::read(payload_path("norprog.bin")).context("read norprog.bin")?;
    write_with(d, &blob, addr, data, log)
}

/// As [`write`], anywhere the full layout allows (`0x1000..0x7E0000`),
/// with the yotonrs payload `payload` (`payload-norprog`, whose own guard
/// is the same).
pub fn write_full(
    d: &mut Adfu,
    payload: &[u8],
    addr: u32,
    data: &[u8],
    log: &mut dyn FnMut(&str),
) -> Result<()> {
    if !full_writable(addr, data.len().div_ceil(SECTOR as usize) as u32 * SECTOR) {
        bail!("refusing NOR write outside 0x1000..0x7E0000: {addr:#x}+{:#x}", data.len());
    }
    write_with(d, payload, addr, data, log)
}

/// Erase `[addr, addr + len)` (sector aligned, full-layout guard) without
/// programming; skips sectors that already read back erased.
pub fn erase_full(
    d: &mut Adfu,
    payload: &[u8],
    addr: u32,
    len: u32,
    log: &mut dyn FnMut(&str),
) -> Result<()> {
    if !addr.is_multiple_of(SECTOR) || !len.is_multiple_of(SECTOR) || !full_writable(addr, len) {
        bail!("refusing NOR erase {addr:#x}+{len:#x}");
    }
    let t0 = Instant::now();
    let mut erased = 0;
    let mut a = addr;
    while a < addr + len {
        let n = (addr + len - a).min(CHUNK as u32);
        let (cur, _) = read(d, a, n as usize)?;
        d.load_payload(payload)?;
        for (i, s) in cur.chunks(SECTOR as usize).enumerate() {
            if s.iter().all(|&b| b == 0xFF) {
                continue;
            }
            let sa = a + i as u32 * SECTOR;
            let r = d.call(yoton_proto::mem::YMON_PAYLOAD, &[1, sa, 0], Duration::from_secs(10))?;
            let status = u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
            if status != 0 {
                bail!("erase at {sa:#x} failed: status {status:#x}");
            }
            erased += 1;
        }
        a += n;
        if (a - addr).is_multiple_of(0x10_0000) {
            log(&format!("  erased up to {a:#x}"));
        }
    }
    let (back, _) = read(d, addr, len as usize)?;
    if let Some(i) = back.iter().position(|&b| b != 0xFF) {
        bail!("NOR erase verify FAILED at {:#x}", addr as usize + i);
    }
    log(&format!("erased {erased} sectors in {addr:#x}+{len:#x} in {:.1} s", t0.elapsed().as_secs_f32()));
    Ok(())
}

fn write_with(d: &mut Adfu, payload: &[u8], addr: u32, data: &[u8], log: &mut dyn FnMut(&str)) -> Result<()> {
    if !addr.is_multiple_of(SECTOR) {
        bail!("NOR write address {addr:#x} is not sector aligned");
    }
    let mut data = data.to_vec();
    data.resize(data.len().div_ceil(SECTOR as usize) * SECTOR as usize, 0xFF);
    let t0 = Instant::now();
    d.load_payload(payload).context("load NOR payload")?;
    let pay = yoton_proto::mem::YMON_PAYLOAD;
    for (si, sector) in data.chunks(SECTOR as usize).enumerate() {
        let sa = addr + (si as u32) * SECTOR;
        let r = d.call(pay, &[1, sa, 0], Duration::from_secs(10))?;
        let status = u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
        if status != 0 {
            bail!("erase at {sa:#x} failed: status {status:#x}");
        }
        for (pi, page) in sector.chunks(256).enumerate() {
            if page.iter().all(|&b| b == 0xFF) {
                continue;
            }
            let pa = sa + (pi as u32) * 256;
            d.write(BUF, page)?;
            let r = d.call(pay, &[2, pa, page.len() as u32], Duration::from_secs(10))?;
            let status = u32::from_le_bytes([r[0], r[1], r[2], r[3]]);
            if status != 0 {
                bail!("program at {pa:#x} failed: status {status:#x}");
            }
        }
        if si % 16 == 15 {
            log(&format!("  {:#x} programmed", sa + SECTOR));
        }
    }
    let (back, _) = read(d, addr, data.len())?;
    if back != data {
        let i = back.iter().zip(&data).position(|(a, b)| a != b).unwrap_or(0);
        bail!("NOR verify FAILED at {:#x}", addr as usize + i);
    }
    log(&format!(
        "wrote and verified {} bytes at {addr:#x} in {:.1} s",
        data.len(),
        t0.elapsed().as_secs_f32()
    ));
    Ok(())
}

/// Rewrite NOR sector 0 (the boot record) with the reference repo's
/// sector-0-only payload. Only `install-boot` and `restore vendor` call
/// this (CLAUDE.md rule 1).
pub fn write_sector0(d: &mut Adfu, data: &[u8], log: &mut dyn FnMut(&str)) -> Result<()> {
    if data.len() != SECTOR as usize {
        bail!("sector 0 image must be exactly one sector");
    }
    let blob = std::fs::read(payload_path("norprog0.bin")).context("read norprog0.bin")?;
    write_with(d, &blob, 0, data, log)
}
