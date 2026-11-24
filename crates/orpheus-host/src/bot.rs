//! USB mass-storage Bulk-Only Transport over `nusb`, as spoken by the boot
//! ROM's ADFU mode and by `ymon` (both frame their commands as CBW/CSW).

use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use nusb::transfer::{Buffer, Bulk, In, Out};
use nusb::{DeviceInfo, Endpoint, Interface, MaybeFuture};

pub const CBW_SIG: u32 = 0x4342_5355; // "USBC"
pub const CSW_SIG: u32 = 0x5342_5355; // "USBS"
pub const CSW_LEN: usize = 13;

/// Find the first device with this VID/PID.
#[must_use]
pub fn find(vid: u16, pid: u16) -> Option<DeviceInfo> {
    nusb::list_devices().wait().ok()?.find(|d| d.vendor_id() == vid && d.product_id() == pid)
}

/// Wait up to `timeout` for a device to appear.
#[must_use]
pub fn wait_for(vid: u16, pid: u16, timeout: Duration) -> Option<DeviceInfo> {
    let t0 = Instant::now();
    loop {
        if let Some(d) = find(vid, pid) {
            return Some(d);
        }
        if t0.elapsed() > timeout {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Wait up to `timeout` for a device to leave the bus.
pub fn wait_gone(vid: u16, pid: u16, timeout: Duration) -> bool {
    let t0 = Instant::now();
    while find(vid, pid).is_some() {
        if t0.elapsed() > timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    true
}

pub struct Bot {
    pub info: DeviceInfo,
    _iface: Interface,
    ep_out: Endpoint<Bulk, Out>,
    ep_in: Endpoint<Bulk, In>,
    mps: usize,
}

/// Status and data of one command.
pub struct Reply {
    pub status: u8,
    pub data: Vec<u8>,
}

impl Bot {
    pub fn open(info: DeviceInfo) -> Result<Self> {
        let t0 = Instant::now();
        let dev = loop {
            match info.open().wait() {
                Ok(d) => break d,
                Err(_) if t0.elapsed() < Duration::from_secs(3) => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(e).context("open USB device"),
            }
        };
        let iface = dev.detach_and_claim_interface(0).wait().context("claim interface 0")?;
        let desc = iface.descriptor().ok_or_else(|| anyhow!("no interface descriptor"))?;
        let mut ep_in_addr = None;
        let mut ep_out_addr = None;
        for ep in desc.endpoints() {
            if ep.transfer_type() == nusb::descriptors::TransferType::Bulk {
                if ep.address() & 0x80 != 0 {
                    ep_in_addr.get_or_insert(ep.address());
                } else {
                    ep_out_addr.get_or_insert(ep.address());
                }
            }
        }
        let ep_in = iface
            .endpoint::<Bulk, In>(ep_in_addr.ok_or_else(|| anyhow!("no bulk IN endpoint"))?)
            .context("open bulk IN")?;
        let ep_out = iface
            .endpoint::<Bulk, Out>(ep_out_addr.ok_or_else(|| anyhow!("no bulk OUT endpoint"))?)
            .context("open bulk OUT")?;
        let mps = ep_in.max_packet_size();
        Ok(Self { info, _iface: iface, ep_out, ep_in, mps })
    }

    fn write_all(&mut self, data: &[u8], timeout: Duration) -> Result<()> {
        let mut buf = Buffer::new(data.len());
        buf.extend_from_slice(data);
        let c = self.ep_out.transfer_blocking(buf, timeout);
        c.status.map_err(|e| anyhow!("bulk OUT: {e:?}"))?;
        if c.actual_len != data.len() {
            bail!("bulk OUT short: {} of {}", c.actual_len, data.len());
        }
        Ok(())
    }

    fn read_some(&mut self, want: usize, timeout: Duration) -> Result<Vec<u8>> {
        let req = want.div_ceil(self.mps).max(1) * self.mps;
        let c = self.ep_in.transfer_blocking(Buffer::new(req), timeout);
        if let Err(e) = c.status {
            if matches!(e, nusb::transfer::TransferError::Stall) {
                let _ = self.ep_in.clear_halt().wait();
            }
            bail!("bulk IN: {e:?}");
        }
        let mut v = c.buffer.into_vec();
        v.truncate(c.actual_len);
        Ok(v)
    }

    /// One CBW/data/CSW exchange. `cdb` ≤ 16 bytes; `tag` 0 for ADFU.
    pub fn command(
        &mut self,
        cdb: &[u8],
        tag: u32,
        data_out: Option<&[u8]>,
        data_in: usize,
        timeout: Duration,
    ) -> Result<Reply> {
        let dlen = data_out.map_or(data_in, <[u8]>::len);
        let mut cbw = Vec::with_capacity(31);
        cbw.extend_from_slice(&CBW_SIG.to_le_bytes());
        cbw.extend_from_slice(&tag.to_le_bytes());
        cbw.extend_from_slice(&(dlen as u32).to_le_bytes());
        cbw.push(if data_in > 0 { 0x80 } else { 0 });
        cbw.push(0);
        cbw.push(16);
        let mut c16 = [0u8; 16];
        c16[..cdb.len().min(16)].copy_from_slice(&cdb[..cdb.len().min(16)]);
        cbw.extend_from_slice(&c16);
        self.write_all(&cbw, timeout)?;
        if let Some(d) = data_out
            && !d.is_empty()
        {
            self.write_all(d, timeout)?;
        }
        let (data, csw) = if data_in > 0 {
            // Data and CSW may arrive in one transfer (when the data ends on
            // a packet boundary) or in two.
            let mut buf = Vec::with_capacity(data_in + CSW_LEN);
            while buf.len() < data_in {
                let chunk = self.read_some(data_in + CSW_LEN - buf.len(), timeout)?;
                if chunk.is_empty() {
                    bail!("bulk IN returned nothing");
                }
                buf.extend_from_slice(&chunk);
                if chunk.len() % self.mps != 0 {
                    break;
                }
            }
            if buf.len() < data_in + CSW_LEN {
                if buf.len() == CSW_LEN && data_in != CSW_LEN && is_csw(&buf) {
                    // Device answered with status only (error, no data).
                    return Ok(Reply { status: buf[12], data: Vec::new() });
                }
                let csw = self.read_some(CSW_LEN, timeout)?;
                buf.extend_from_slice(&csw);
            }
            if buf.len() < data_in + CSW_LEN {
                bail!("short reply: {} bytes for {data_in} + CSW", buf.len());
            }
            let csw = buf.split_off(buf.len() - CSW_LEN);
            buf.truncate(data_in);
            (buf, csw)
        } else {
            (Vec::new(), self.read_some(CSW_LEN, timeout)?)
        };
        if !is_csw(&csw) {
            bail!("bad CSW {csw:02x?}");
        }
        Ok(Reply { status: csw[12], data })
    }
}

fn is_csw(b: &[u8]) -> bool {
    b.len() == CSW_LEN && b[..4] == CSW_SIG.to_le_bytes()
}
