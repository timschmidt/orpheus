//! Host side of the developer channel (EP0 vendor requests to yotonrs).

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use nusb::MaybeFuture;
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient};
use yoton_proto::devchan::{
    DEV_PID, DEV_VID, Info, REQ_FRAME, REQ_INFO, REQ_LOG, REQ_PEEK, REQ_REBOOT, REQ_STATS,
};

use crate::bot;

pub struct DevChannel {
    dev: nusb::Device,
    pub info: nusb::DeviceInfo,
    /// Largest data stage used per request.
    pub chunk: u16,
}

const T: Duration = Duration::from_secs(2);

impl DevChannel {
    pub fn open(wait: Duration) -> Result<Self> {
        let info = bot::wait_for(DEV_VID, DEV_PID, wait)
            .ok_or_else(|| anyhow!("no yotonrs device (1209:0001) on the bus"))?;
        let dev = info.open().wait().context("open 1209:0001")?;
        Ok(Self { dev, info, chunk: 4096 })
    }

    pub fn speed(&self) -> Option<nusb::Speed> {
        self.info.speed()
    }

    pub fn control_in(&self, request: u8, value: u16, index: u16, len: u16) -> Result<Vec<u8>> {
        let r = self
            .dev
            .control_in(
                ControlIn {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request,
                    value,
                    index,
                    length: len,
                },
                T,
            )
            .wait()
            .map_err(|e| anyhow!("control IN {request:#x}: {e:?}"))?;
        Ok(r)
    }

    pub fn control_out(&self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<()> {
        self.dev
            .control_out(
                ControlOut {
                    control_type: ControlType::Vendor,
                    recipient: Recipient::Device,
                    request,
                    value,
                    index,
                    data,
                },
                T,
            )
            .wait()
            .map_err(|e| anyhow!("control OUT {request:#x}: {e:?}"))
    }

    pub fn info(&self) -> Result<Info> {
        let b = self.control_in(REQ_INFO, 0, 0, Info::SIZE as u16)?;
        Info::from_bytes(&b).ok_or_else(|| anyhow!("short INFO reply ({} bytes)", b.len()))
    }

    fn read_by(&self, req: u8, base: u32, len: usize) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            let a = base + out.len() as u32;
            let n = (len - out.len()).min(usize::from(self.chunk));
            let b = self.control_in(req, a as u16, (a >> 16) as u16, n as u16)?;
            if b.is_empty() {
                break;
            }
            out.extend_from_slice(&b);
        }
        Ok(out)
    }

    pub fn peek(&self, addr: u32, len: usize) -> Result<Vec<u8>> {
        self.read_by(REQ_PEEK, addr, len)
    }

    pub fn frame(&self) -> Result<Vec<u8>> {
        self.read_by(REQ_FRAME, 0, yoton_proto::mem::FB_SIZE as usize)
    }

    pub fn log_area(&self) -> Result<Vec<u8>> {
        self.read_by(REQ_LOG, 0, yoton_proto::logring::SIZE)
    }

    pub fn stats(&self, group: u16) -> Result<Vec<u32>> {
        let b = self.control_in(REQ_STATS, group, 0, 256)?;
        Ok(b.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect())
    }

    /// Ask for a reboot (target per `devchan::REBOOT_*`); the device may
    /// reset before the status stage completes, so errors are ignored.
    pub fn reboot(self, target: u16) {
        let _ = self.control_out(REQ_REBOOT, target, 0, &[]);
    }
}
