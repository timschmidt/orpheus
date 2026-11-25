//! Host side of the developer channel (EP0 vendor requests to orpheus).
//!
//! [`Link`] is the protocol over any [`Transport`]; [`DevChannel`] (desktop)
//! is the blocking front end over `nusb`. [`Link::update`] is the firmware
//! update without ADFU (UPD-1): `xtask dev update` and the browser
//! installer's Update flow.

use anyhow::{Result, anyhow, bail};
use orpheus_proto::devchan::{
    Info, REQ_FRAME, REQ_INFO, REQ_LOG, REQ_PEEK, REQ_REBOOT, REQ_STATS, REQ_TEST, REQ_UPDATE,
};
use orpheus_update::wire::{self, State, Status};
use orpheus_update::{Error as UpdError, ofw};

use crate::transport::{Control, Progress, Transport};

const T_MS: u32 = 2000;

/// The developer channel over a transport.
pub struct Link<T: Transport> {
    pub t: T,
    /// Largest data stage used per request.
    pub chunk: u16,
}

impl<T: Transport> Link<T> {
    pub fn new(t: T) -> Self {
        Self { t, chunk: 1024 }
    }

    pub async fn control_in(&mut self, request: u8, value: u16, index: u16, len: u16) -> Result<Vec<u8>> {
        self.t.control_in(Control { request, value, index }, len, T_MS).await
    }

    pub async fn control_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<()> {
        self.t.control_out(Control { request, value, index }, data, T_MS).await
    }

    pub async fn info(&mut self) -> Result<Info> {
        let b = self.control_in(REQ_INFO, 0, 0, Info::SIZE as u16).await?;
        Info::from_bytes(&b).ok_or_else(|| anyhow!("short INFO reply ({} bytes)", b.len()))
    }

    async fn read_by(&mut self, req: u8, base: u32, len: usize) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(len);
        while out.len() < len {
            let a = base + out.len() as u32;
            let n = (len - out.len()).min(usize::from(self.chunk));
            let b = self.control_in(req, a as u16, (a >> 16) as u16, n as u16).await?;
            if b.is_empty() {
                break;
            }
            out.extend_from_slice(&b);
        }
        Ok(out)
    }

    pub async fn peek(&mut self, addr: u32, len: usize) -> Result<Vec<u8>> {
        self.read_by(REQ_PEEK, addr, len).await
    }

    pub async fn frame(&mut self) -> Result<Vec<u8>> {
        self.read_by(REQ_FRAME, 0, orpheus_proto::mem::FB_SIZE as usize).await
    }

    pub async fn log_area(&mut self) -> Result<Vec<u8>> {
        self.read_by(REQ_LOG, 0, orpheus_proto::logring::SIZE).await
    }

    pub async fn stats(&mut self, group: u16) -> Result<Vec<u32>> {
        let b = self.control_in(REQ_STATS, group, 0, 256).await?;
        Ok(b.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect())
    }

    /// A test command and, after `wait_ms`, its reply.
    pub async fn command(&mut self, text: &str, wait_ms: u32) -> Result<String> {
        self.control_out(REQ_TEST, 0, 0, text.as_bytes()).await?;
        self.t.sleep_ms(wait_ms).await;
        let r = self.control_in(REQ_TEST, 0, 0, 1024).await?;
        Ok(String::from_utf8_lossy(&r).into_owned())
    }

    /// Ask for a reboot (target per `devchan::REBOOT_*`); the device may
    /// reset before the status stage completes, so errors are ignored.
    pub async fn reboot(&mut self, target: u16) {
        let _ = self.control_out(REQ_REBOOT, target, 0, &[]).await;
    }

    pub async fn update_status(&mut self) -> Result<Status> {
        let b = self.control_in(REQ_UPDATE, 0, 0, Status::SIZE as u16).await?;
        Status::from_bytes(&b).ok_or_else(|| anyhow!("short UPDATE status ({} bytes)", b.len()))
    }

    async fn update_fail(&mut self, what: &str) -> anyhow::Error {
        match self.update_status().await {
            Ok(s) => anyhow!("{what}: {}", s.error().map_or("refused", UpdError::text)),
            Err(e) => anyhow!("{what}: {e:#}"),
        }
    }

    /// Install `file` (a whole `.ofw`) into the inactive slot through the
    /// running firmware: check it here (header, SHA-256), `BEGIN`, the image
    /// in 1 KiB chunks, `COMMIT`, wait for the read-back and the slot
    /// switch. Does not reboot (see [`Link::reboot`]). Returns the target
    /// slot index.
    pub async fn update(&mut self, file: &[u8], progress: Progress<'_>) -> Result<u8> {
        let (h, _) = check_file(file)?;
        let image = &file[ofw::HEADER_SIZE..];
        let mut begin = Vec::with_capacity(wire::BEGIN_LEN);
        begin.extend_from_slice(&file[..ofw::HEADER_SIZE]);
        begin.extend_from_slice(&image[..orpheus_proto::slot::HEADER_SIZE]);
        if self.control_out(REQ_UPDATE, wire::OP_BEGIN, 0, &begin).await.is_err() {
            return Err(self.update_fail("player refused the update").await);
        }
        let total = u64::from(h.image_len);
        for (i, c) in image.chunks(wire::CHUNK).enumerate() {
            let idx = u16::try_from(i).map_err(|_| anyhow!("image too large"))?;
            // One retry: a lost acknowledgement makes the player see the
            // chunk again, which it compares instead of writing.
            let first = self.control_out(REQ_UPDATE, wire::OP_DATA, idx, c).await;
            if first.is_err() && self.control_out(REQ_UPDATE, wire::OP_DATA, idx, c).await.is_err() {
                return Err(self.update_fail(&format!("chunk {i}")).await);
            }
            progress("write", ((i + 1) * wire::CHUNK).min(image.len()) as u64, total);
        }
        if self.control_out(REQ_UPDATE, wire::OP_COMMIT, 0, &[]).await.is_err() {
            return Err(self.update_fail("commit").await);
        }
        let t0 = self.t.now_ms();
        loop {
            self.t.sleep_ms(100).await;
            // The player blocks USB briefly while it reads back: retry.
            let Ok(s) = self.update_status().await else {
                if self.t.now_ms() - t0 > 120_000 {
                    bail!("no update status for two minutes");
                }
                continue;
            };
            match s.state {
                State::Done => return Ok(s.slot),
                State::Failed => {
                    bail!("update failed: {}", s.error().map_or("unknown error", UpdError::text))
                }
                State::Verifying | State::Committing => progress("verify", u64::from(s.verified), total),
                State::Idle | State::Card | State::Writing => {
                    if s.error != 0 {
                        bail!("update failed: {}", s.error().map_or("unknown error", UpdError::text));
                    }
                }
            }
            if self.t.now_ms() - t0 > 120_000 {
                bail!("update did not finish within two minutes");
            }
        }
    }
}

/// Check a whole `.ofw` file: header, image length, slot header, SHA-256.
pub fn check_file(file: &[u8]) -> Result<(ofw::Header, orpheus_proto::slot::SlotHeader)> {
    use orpheus_update::Digest as _;
    if file.len() < ofw::HEADER_SIZE + orpheus_proto::slot::HEADER_SIZE {
        bail!("not an update file (too short)");
    }
    let image = &file[ofw::HEADER_SIZE..];
    let (h, s) = ofw::check(&file[..ofw::HEADER_SIZE], image).map_err(|e| anyhow!("{}", e.text()))?;
    if image.len() != h.image_len as usize {
        bail!("file length {} does not match the header's image length {}", file.len(), h.image_len);
    }
    let digest: [u8; 32] = orpheus_update::Sha256::digest(image).into();
    if digest != h.sha256 {
        bail!("{}", UpdError::Hash.text());
    }
    Ok((h, s))
}

#[cfg(feature = "native")]
pub use crate::native::DevChannel;
