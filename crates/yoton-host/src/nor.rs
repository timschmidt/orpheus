//! NOR flash access through RAM payloads run by ymon.
//!
//! Until WP-T-6 replaces them with Rust payloads, these are the C payloads of
//! the reference repository (`nordump.bin`, `norprog.bin`). `norprog`
//! refuses addresses outside `0x762000..0x7E0000`; this module checks the
//! same range first ([`yoton_proto::layout::dual_writable`]).

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use yoton_proto::layout::{SECTOR, dual_writable};

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
    if !addr.is_multiple_of(SECTOR) {
        bail!("NOR write address {addr:#x} is not sector aligned");
    }
    let mut data = data.to_vec();
    data.resize(data.len().div_ceil(SECTOR as usize) * SECTOR as usize, 0xFF);
    if !dual_writable(addr, data.len() as u32) {
        bail!("refusing NOR write outside 0x762000..0x7E0000: {addr:#x}+{:#x}", data.len());
    }
    let t0 = Instant::now();
    d.load_payload_file(&payload_path("norprog.bin")).context("load norprog")?;
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
