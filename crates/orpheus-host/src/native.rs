//! The desktop transport (`nusb`) and the blocking front ends `xtask`
//! uses: [`Adfu`], [`DevChannel`], [`nor`]. The protocols themselves are
//! the shared `async` code in [`crate::adfu`], [`crate::devchan`] and
//! [`crate::nor`].

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use nusb::transfer::{Buffer, Bulk, ControlIn, ControlOut, ControlType, In, Out, Recipient};
use nusb::{DeviceInfo, Endpoint, Interface, MaybeFuture};
use orpheus_proto::devchan::{ADFU_PID, ADFU_VID, DEV_PID, DEV_VID, Info};

use crate::adfu::{ExecState, Link as AdfuLink, Mode, YmonFault};
use crate::devchan::Link as DevLink;
use crate::transport::{Control, Transport, block_on};

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

struct BulkPipe {
    _iface: Interface,
    ep_out: Endpoint<Bulk, Out>,
    ep_in: Endpoint<Bulk, In>,
    mps: usize,
}

/// A device opened through `nusb`, with interface 0's bulk endpoints when
/// asked for (ADFU).
pub struct Nusb {
    dev: nusb::Device,
    bulk: Option<BulkPipe>,
    t0: Instant,
}

fn open_device(info: &DeviceInfo) -> Result<nusb::Device> {
    // udev applies permissions a moment after the node appears: retry.
    let t0 = Instant::now();
    loop {
        match info.open().wait() {
            Ok(d) => return Ok(d),
            Err(_) if t0.elapsed() < Duration::from_secs(3) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e).context("open USB device"),
        }
    }
}

impl Nusb {
    /// Open for control transfers only (the developer channel).
    pub fn control(info: &DeviceInfo) -> Result<Self> {
        Ok(Self { dev: open_device(info)?, bulk: None, t0: Instant::now() })
    }

    /// Open and claim interface 0 with its bulk endpoints (ADFU, ymon).
    pub fn bulk(info: &DeviceInfo) -> Result<Self> {
        let dev = open_device(info)?;
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
        Ok(Self { dev, bulk: Some(BulkPipe { _iface: iface, ep_out, ep_in, mps }), t0: Instant::now() })
    }

    fn pipe(&mut self) -> Result<&mut BulkPipe> {
        self.bulk.as_mut().ok_or_else(|| anyhow!("no bulk interface claimed"))
    }
}

fn ms(t: u32) -> Duration {
    Duration::from_millis(u64::from(t))
}

impl Transport for Nusb {
    async fn bulk_out(&mut self, data: &[u8], timeout_ms: u32) -> Result<()> {
        let p = self.pipe()?;
        let mut buf = Buffer::new(data.len());
        buf.extend_from_slice(data);
        let c = p.ep_out.transfer_blocking(buf, ms(timeout_ms));
        c.status.map_err(|e| anyhow!("bulk OUT: {e:?}"))?;
        if c.actual_len != data.len() {
            bail!("bulk OUT short: {} of {}", c.actual_len, data.len());
        }
        Ok(())
    }

    async fn bulk_in(&mut self, len: usize, timeout_ms: u32) -> Result<Vec<u8>> {
        let p = self.pipe()?;
        let c = p.ep_in.transfer_blocking(Buffer::new(len), ms(timeout_ms));
        if let Err(e) = c.status {
            bail!("bulk IN: {e:?}");
        }
        let mut v = c.buffer.into_vec();
        v.truncate(c.actual_len);
        Ok(v)
    }

    async fn clear_halt_in(&mut self) {
        if let Ok(p) = self.pipe() {
            let _ = p.ep_in.clear_halt().wait();
        }
    }

    fn packet_size(&self) -> usize {
        self.bulk.as_ref().map_or(512, |p| p.mps)
    }

    async fn control_in(&mut self, c: Control, len: u16, timeout_ms: u32) -> Result<Vec<u8>> {
        self.dev
            .control_in(
                ControlIn {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: c.request,
                    value: c.value,
                    index: c.index,
                    length: len,
                },
                ms(timeout_ms),
            )
            .wait()
            .map_err(|e| anyhow!("control IN {:#x}: {e:?}", c.request))
    }

    async fn control_out(&mut self, c: Control, data: &[u8], timeout_ms: u32) -> Result<()> {
        self.dev
            .control_out(
                ControlOut {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request: c.request,
                    value: c.value,
                    index: c.index,
                    data,
                },
                ms(timeout_ms),
            )
            .wait()
            .map_err(|e| anyhow!("control OUT {:#x}: {e:?}", c.request))
    }

    async fn sleep_ms(&mut self, ms: u32) {
        std::thread::sleep(Duration::from_millis(u64::from(ms)));
    }

    fn now_ms(&self) -> u64 {
        self.t0.elapsed().as_millis() as u64
    }
}

// ---------------------------------------------------------------- ADFU

/// Where the reference tools and payloads live (read-only).
#[must_use]
pub fn yoton_dir() -> PathBuf {
    std::env::var_os("ORPHEUS_DIR")
        .map_or_else(|| PathBuf::from("/home/tim/Documents/GitHub/yoton"), PathBuf::from)
}

#[must_use]
pub fn payload_path(name: &str) -> PathBuf {
    yoton_dir().join("payloads").join(name)
}

fn read_file(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("read {}", path.display()))
}

/// The boot ROM's ADFU mode / ymon on the desktop (blocking).
pub struct Adfu {
    pub link: AdfuLink<Nusb>,
}

impl Adfu {
    /// Open the ADFU device, waiting up to `wait` for it to appear.
    pub fn open(wait: Duration) -> Result<Self> {
        let info = wait_for(ADFU_VID, ADFU_PID, wait)
            .ok_or_else(|| anyhow!("no ADFU device (10d6:10d6) on the bus"))?;
        // The ROM needs a moment after enumeration before it answers.
        std::thread::sleep(Duration::from_millis(300));
        Ok(Self { link: AdfuLink::new(Nusb::bulk(&info)?) })
    }

    /// Raw command; returns (CSW status, data).
    pub fn xfer(
        &mut self,
        sub: u32,
        len: u32,
        addr: u32,
        data_out: Option<&[u8]>,
        data_in: usize,
        timeout: Duration,
    ) -> Result<(u8, Vec<u8>)> {
        let t = timeout.as_millis().min(u128::from(u32::MAX)) as u32;
        block_on(self.link.xfer(sub, len, addr, data_out, data_in, t))
    }
    pub fn mode(&mut self) -> Option<Mode> {
        block_on(self.link.mode())
    }
    pub fn read(&mut self, addr: u32, n: usize) -> Result<Vec<u8>> {
        block_on(self.link.read(addr, n))
    }
    pub fn read_u32(&mut self, addr: u32) -> Result<u32> {
        block_on(self.link.read_u32(addr))
    }
    pub fn write_step(&mut self, addr: u32, data: &[u8], step: usize) -> Result<()> {
        block_on(self.link.write_step(addr, data, step))
    }
    pub fn write(&mut self, addr: u32, data: &[u8]) -> Result<()> {
        block_on(self.link.write(addr, data))
    }
    pub fn write_u32(&mut self, addr: u32, v: u32) -> Result<()> {
        block_on(self.link.write_u32(addr, v))
    }
    pub fn write_verify(&mut self, addr: u32, data: &[u8], step: usize) -> Result<()> {
        block_on(self.link.write_verify(addr, data, step))
    }
    pub fn jump(&mut self, addr: u32) -> Result<()> {
        block_on(self.link.jump(addr))
    }
    /// Make sure ymon runs (upload from the reference repository if the ROM
    /// answers).
    pub fn ensure_ymon(&mut self) -> Result<()> {
        if self.mode() == Some(Mode::Ymon) {
            return Ok(());
        }
        let blob = read_file(&payload_path("ymon.bin"))?;
        block_on(self.link.ensure_ymon(&blob))
    }
    pub fn exec_state(&mut self) -> Result<ExecState> {
        block_on(self.link.exec_state())
    }
    pub fn ymon_fault(&mut self) -> Result<YmonFault> {
        block_on(self.link.ymon_fault())
    }
    pub fn call(&mut self, addr: u32, args: &[u32], timeout: Duration) -> Result<Vec<u8>> {
        let t = timeout.as_millis().min(u128::from(u32::MAX)) as u32;
        block_on(self.link.call(addr, args, t))
    }
    pub fn load_payload(&mut self, blob: &[u8]) -> Result<()> {
        block_on(self.link.load_payload(blob))
    }
    pub fn load_payload_file(&mut self, path: &Path) -> Result<()> {
        let blob = read_file(path)?;
        self.load_payload(&blob)
    }
    /// System reset through ymon/ROM; the device re-enumerates.
    pub fn reset(mut self) {
        block_on(self.link.reset());
    }
    /// Hand the device back (drop the claim) without resetting it.
    pub fn close(self) {}
}

/// The developer channel on the desktop (blocking).
pub struct DevChannel {
    pub link: DevLink<Nusb>,
    pub info: DeviceInfo,
}

impl DevChannel {
    pub fn open(wait: Duration) -> Result<Self> {
        let info = wait_for(DEV_VID, DEV_PID, wait)
            .ok_or_else(|| anyhow!("no orpheus device (1209:0001) on the bus"))?;
        Ok(Self { link: DevLink::new(Nusb::control(&info)?), info })
    }
    pub fn speed(&self) -> Option<nusb::Speed> {
        self.info.speed()
    }
    pub fn control_in(&self, request: u8, value: u16, index: u16, len: u16) -> Result<Vec<u8>> {
        self.with(|l| block_on(l.control_in(request, value, index, len)))
    }
    pub fn control_out(&self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<()> {
        self.with(|l| block_on(l.control_out(request, value, index, data)))
    }
    pub fn info(&self) -> Result<Info> {
        self.with(|l| block_on(l.info()))
    }
    pub fn peek(&self, addr: u32, len: usize) -> Result<Vec<u8>> {
        self.with(|l| block_on(l.peek(addr, len)))
    }
    pub fn frame(&self) -> Result<Vec<u8>> {
        self.with(|l| block_on(l.frame()))
    }
    pub fn log_area(&self) -> Result<Vec<u8>> {
        self.with(|l| block_on(l.log_area()))
    }
    pub fn stats(&self, group: u16) -> Result<Vec<u32>> {
        self.with(|l| block_on(l.stats(group)))
    }
    /// Whether the developer interface has the bulk OUT endpoint of
    /// uploads (`orpheus_proto::devchan::upload::BULK_EP`, PERF-SD round 2).
    pub fn has_bulk_upload(&self) -> bool {
        let Ok(c) = self.link.t.dev.active_configuration() else { return false };
        c.interface_alt_settings().any(|a| {
            a.interface_number() == 0
                && a.endpoints().any(|e| e.address() == orpheus_proto::devchan::upload::BULK_EP)
        })
    }

    /// Send `data` to the bulk OUT endpoint of uploads, in transfers of at
    /// most 64 KiB (the player takes it as `upload::pieces`).
    pub fn bulk_upload(&self, data: &[u8], timeout: Duration) -> Result<()> {
        let iface = self.link.t.dev.detach_and_claim_interface(0).wait().context("claim interface 0")?;
        let mut ep = iface
            .endpoint::<Bulk, Out>(orpheus_proto::devchan::upload::BULK_EP)
            .context("open the upload endpoint")?;
        for piece in data.chunks(64 * 1024) {
            let mut buf = Buffer::new(piece.len());
            buf.extend_from_slice(piece);
            let c = ep.transfer_blocking(buf, timeout);
            c.status.map_err(|e| anyhow!("bulk upload: {e:?}"))?;
            if c.actual_len != piece.len() {
                bail!("bulk upload short: {} of {}", c.actual_len, piece.len());
            }
        }
        Ok(())
    }

    /// Firmware update through the running firmware (UPD-1).
    pub fn update(&mut self, file: &[u8], progress: crate::transport::Progress<'_>) -> Result<u8> {
        block_on(self.link.update(file, progress))
    }
    /// The write path of an update only, then `ABORT` (`Link::update_part`).
    pub fn update_part(
        &mut self,
        file: &[u8],
        stop_after: Option<usize>,
        progress: crate::transport::Progress<'_>,
    ) -> Result<Option<u8>> {
        block_on(self.link.update_part(file, stop_after, progress))
    }
    /// Ask for a reboot (target per `devchan::REBOOT_*`).
    pub fn reboot(mut self, target: u16) {
        block_on(self.link.reboot(target));
    }
    /// `nusb` calls take `&self`; the link's take `&mut self` (the browser's
    /// cannot overlap). A short-lived copy of the handle bridges the two.
    fn with<R>(&self, f: impl FnOnce(&mut DevLink<Nusb>) -> R) -> R {
        let mut l = DevLink {
            t: Nusb { dev: self.link.t.dev.clone(), bulk: None, t0: self.link.t.t0 },
            chunk: self.link.chunk,
        };
        f(&mut l)
    }
}

/// NOR access on the desktop with the payloads from disk (the reference
/// repository's `nordump`/`norprog0`, `fw/payloads/norprog` built by
/// `xtask`).
pub mod nor {
    use anyhow::Result;
    use orpheus_proto::layout::{SECTOR, dual_writable};

    use super::{Adfu, payload_path, read_file};
    use crate::nor::{self as proto, Payloads, Sector0Permit};
    use crate::transport::block_on;

    /// ymon, nordump, norprog, norprog0.
    type Files = (Vec<u8>, Vec<u8>, Vec<u8>, Option<Vec<u8>>);

    fn payloads(norprog: Vec<u8>, with0: bool) -> Result<Files> {
        let ymon = read_file(&payload_path("ymon.bin"))?;
        let nordump = read_file(&payload_path("nordump.bin"))?;
        let p0 = if with0 { Some(read_file(&payload_path("norprog0.bin"))?) } else { None };
        Ok((ymon, nordump, norprog, p0))
    }

    /// Read NOR bytes (raw). Returns data and the JEDEC id.
    pub fn read(d: &mut Adfu, addr: u32, len: usize) -> Result<(Vec<u8>, u32)> {
        let nordump = read_file(&payload_path("nordump.bin"))?;
        block_on(proto::read(&mut d.link, &nordump, addr, len, &mut |_, _, _| {}))
    }

    fn logger<'a>(log: &'a mut dyn FnMut(&str)) -> impl FnMut(&str, u64, u64) + 'a {
        let mut last = 0u64;
        move |what, done, total| {
            if done >= last + 0x4_0000 || done == total {
                last = done;
                log(&format!("  {what} {done:#x} of {total:#x}"));
            }
        }
    }

    /// Erase and program `data` at `addr` (sector aligned) inside the
    /// dual-boot free region with the reference repository's `norprog`.
    pub fn write(d: &mut Adfu, addr: u32, data: &[u8], log: &mut dyn FnMut(&str)) -> Result<()> {
        if !dual_writable(addr, data.len().div_ceil(SECTOR as usize) as u32 * SECTOR) {
            anyhow::bail!("refusing NOR write outside 0x762000..0x7E0000: {addr:#x}+{:#x}", data.len());
        }
        let (ymon, nordump, norprog, _) = payloads(read_file(&payload_path("norprog.bin"))?, false)?;
        let p = Payloads { ymon: &ymon, nordump: &nordump, norprog: &norprog, norprog0: None };
        block_on(proto::write_dual(&mut d.link, &p, addr, data, &mut logger(log)))?;
        log(&format!("wrote and verified {} bytes at {addr:#x}", data.len()));
        Ok(())
    }

    /// As [`write`], anywhere the full layout allows, with `payload`
    /// (`payload-norprog`).
    pub fn write_full(
        d: &mut Adfu,
        payload: &[u8],
        addr: u32,
        data: &[u8],
        log: &mut dyn FnMut(&str),
    ) -> Result<()> {
        let (ymon, nordump, norprog, _) = payloads(payload.to_vec(), false)?;
        let p = Payloads { ymon: &ymon, nordump: &nordump, norprog: &norprog, norprog0: None };
        block_on(proto::write_full(&mut d.link, &p, addr, data, &mut logger(log)))?;
        log(&format!("wrote and verified {} bytes at {addr:#x}", data.len()));
        Ok(())
    }

    /// Erase `[addr, addr + len)` (full-layout guard).
    pub fn erase_full(
        d: &mut Adfu,
        payload: &[u8],
        addr: u32,
        len: u32,
        log: &mut dyn FnMut(&str),
    ) -> Result<()> {
        let (ymon, nordump, norprog, _) = payloads(payload.to_vec(), false)?;
        let p = Payloads { ymon: &ymon, nordump: &nordump, norprog: &norprog, norprog0: None };
        let n = block_on(proto::erase_full(&mut d.link, &p, addr, len, &mut logger(log)))?;
        log(&format!("erased {n} sectors in {addr:#x}+{len:#x}"));
        Ok(())
    }

    /// Rewrite NOR sector 0 with the reference repository's sector-0
    /// payload; needs the whole-chip `backup` (CLAUDE.md rule 1: only
    /// `restore vendor --i-have-a-backup`).
    pub fn write_sector0(d: &mut Adfu, data: &[u8], backup: &[u8], log: &mut dyn FnMut(&str)) -> Result<()> {
        let permit =
            Sector0Permit::new(backup, true).ok_or_else(|| anyhow::anyhow!("no whole-chip backup"))?;
        let (ymon, nordump, norprog, p0) = payloads(Vec::new(), true)?;
        let p = Payloads { ymon: &ymon, nordump: &nordump, norprog: &norprog, norprog0: p0.as_deref() };
        block_on(proto::write_sector0(&mut d.link, &p, data, &permit, &mut logger(log)))
    }
}
