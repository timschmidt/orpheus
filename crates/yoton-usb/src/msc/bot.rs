//! Bulk-Only Transport (USB MSC BBB) pump over `embassy-usb-driver`
//! endpoints (06-connectivity §2.4).
//!
//! One command at a time: read a 31-byte CBW, interpret the CDB with
//! [`Scsi`], move the data phase between the block device and the bulk
//! endpoints in `buf`-sized pieces, answer with a 13-byte CSW. Errors in the
//! data phase surface as CHECK CONDITION with sense data.

use embassy_usb_driver::{EndpointError, EndpointIn, EndpointOut};

use super::BlockDevice;
use super::scsi::{Plan, Scsi, Sense, Unit};

pub const CBW_SIG: u32 = 0x4342_5355;
pub const CSW_SIG: u32 = 0x5342_5355;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cbw {
    pub tag: u32,
    pub len: u32,
    pub dir_in: bool,
    pub lun: u8,
    pub cdb: [u8; 16],
    pub cdb_len: u8,
}

impl Cbw {
    #[must_use]
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() != 31 || u32::from_le_bytes([b[0], b[1], b[2], b[3]]) != CBW_SIG {
            return None;
        }
        let cdb_len = b[14] & 0x1F;
        if cdb_len == 0 || cdb_len > 16 {
            return None;
        }
        let mut cdb = [0u8; 16];
        cdb[..usize::from(cdb_len)].copy_from_slice(&b[15..15 + usize::from(cdb_len)]);
        Some(Self {
            tag: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            len: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
            dir_in: b[12] & 0x80 != 0,
            lun: b[13] & 0x0F,
            cdb,
            cdb_len,
        })
    }
}

#[must_use]
pub fn csw(tag: u32, residue: u32, status: u8) -> [u8; 13] {
    let mut c = [0u8; 13];
    c[..4].copy_from_slice(&CSW_SIG.to_le_bytes());
    c[4..8].copy_from_slice(&tag.to_le_bytes());
    c[8..12].copy_from_slice(&residue.to_le_bytes());
    c[12] = status;
    c
}

/// Counters for diagnostics and the USB screen.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub commands: u32,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub errors: u32,
    pub phase_errors: u32,
}

pub struct Bot<'b, I: EndpointIn, O: EndpointOut, B: BlockDevice> {
    pub ep_in: I,
    pub ep_out: O,
    pub dev: B,
    pub scsi: Scsi,
    pub unit: Unit,
    /// Data-phase buffer (a multiple of the block size; the USB pool gives
    /// 48 KiB).
    buf: &'b mut [u8],
    pub stats: Stats,
}

impl<'b, I: EndpointIn, O: EndpointOut, B: BlockDevice> Bot<'b, I, O, B> {
    pub fn new(ep_in: I, ep_out: O, dev: B, unit: Unit, buf: &'b mut [u8]) -> Self {
        Self { ep_in, ep_out, dev, scsi: Scsi::new(), unit, buf, stats: Stats::default() }
    }

    async fn read_exact(&mut self, n: usize) -> Result<usize, EndpointError> {
        let mps = usize::from(self.ep_out.info().max_packet_size).max(1);
        let mut got = 0;
        let cap = self.buf.len();
        while got < n {
            let end = (got + mps).min(cap);
            let k = self.ep_out.read(&mut self.buf[got..end]).await?;
            got += k;
            if k < mps {
                break;
            }
        }
        Ok(got)
    }

    async fn write_all(&mut self, n: usize) -> Result<(), EndpointError> {
        let mps = usize::from(self.ep_in.info().max_packet_size).max(1);
        let mut i = 0;
        while i < n {
            let k = (n - i).min(mps);
            self.ep_in.write(&self.buf[i..i + k]).await?;
            i += k;
        }
        Ok(())
    }

    /// Serve one command. Returns `Err` only when the bus went away.
    pub async fn serve_one(&mut self) -> Result<(), EndpointError> {
        // CBW.
        let mut cbw_buf = [0u8; 31];
        let n = self.ep_out.read(&mut cbw_buf).await?;
        let Some(cbw) = Cbw::parse(&cbw_buf[..n]) else {
            self.stats.phase_errors += 1;
            return Ok(());
        };
        self.stats.commands += 1;
        let mut status = 0u8;
        let mut done = 0u32;
        let plan_kind = {
            let unit = self.unit;
            match self.scsi.command(&unit, &cbw.cdb[..usize::from(cbw.cdb_len)]) {
                Plan::Done => Kind::None,
                Plan::Fail => Kind::Fail,
                Plan::DataIn(d) => {
                    let k = d.len().min(cbw.len as usize).min(self.buf.len());
                    self.buf[..k].copy_from_slice(&d[..k]);
                    Kind::Data(k)
                }
                Plan::Read { lba, count } => Kind::Read(lba, count),
                Plan::Write { lba, count } => Kind::Write(lba, count),
            }
        };
        match plan_kind {
            Kind::None => {}
            Kind::Fail => status = 1,
            Kind::Data(k) => {
                if cbw.dir_in {
                    self.write_all(k).await?;
                    done = k as u32;
                } else {
                    status = 2;
                    self.stats.phase_errors += 1;
                }
            }
            Kind::Read(lba, count) => {
                let bs = self.unit.block_size as usize;
                let total = count as usize * bs;
                if !cbw.dir_in || total > cbw.len as usize {
                    status = 2;
                    self.stats.phase_errors += 1;
                } else {
                    let per = (self.buf.len() / bs).max(1);
                    let mut blk = 0u32;
                    while blk < count {
                        let n = (count - blk).min(per as u32) as usize;
                        if self.dev.read(lba + u64::from(blk), &mut self.buf[..n * bs]).await.is_err() {
                            self.scsi.set_sense(Sense::READ_ERROR);
                            status = 1;
                            self.stats.errors += 1;
                            break;
                        }
                        self.write_all(n * bs).await?;
                        blk += n as u32;
                        done += (n * bs) as u32;
                    }
                    self.stats.bytes_read += u64::from(done);
                }
            }
            Kind::Write(lba, count) => {
                let bs = self.unit.block_size as usize;
                let total = count as usize * bs;
                if cbw.dir_in || total > cbw.len as usize {
                    status = 2;
                    self.stats.phase_errors += 1;
                } else {
                    let per = (self.buf.len() / bs).max(1);
                    let mut blk = 0u32;
                    while blk < count {
                        let n = (count - blk).min(per as u32) as usize;
                        let got = self.read_exact(n * bs).await?;
                        if got != n * bs {
                            status = 2;
                            self.stats.phase_errors += 1;
                            break;
                        }
                        if status == 0
                            && self.dev.write(lba + u64::from(blk), &self.buf[..n * bs]).await.is_err()
                        {
                            self.scsi.set_sense(Sense::WRITE_ERROR);
                            status = 1;
                            self.stats.errors += 1;
                        }
                        blk += n as u32;
                        done += (n * bs) as u32;
                    }
                    self.stats.bytes_written += u64::from(done);
                }
            }
        }
        let residue = cbw.len.saturating_sub(done);
        let c = csw(cbw.tag, residue, status);
        self.ep_in.write(&c).await
    }
}

enum Kind {
    None,
    Fail,
    Data(usize),
    Read(u64, u32),
    Write(u64, u32),
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::convert::Infallible;
    use embassy_futures::block_on;
    use embassy_usb_driver::{Direction, Endpoint, EndpointAddress, EndpointInfo, EndpointType};
    use std::collections::VecDeque;
    use std::vec;
    use std::vec::Vec;

    struct In {
        info: EndpointInfo,
        sent: Vec<Vec<u8>>,
    }
    struct Out {
        info: EndpointInfo,
        queue: VecDeque<Vec<u8>>,
    }
    fn info(dir: Direction) -> EndpointInfo {
        EndpointInfo {
            addr: EndpointAddress::from_parts(1, dir),
            ep_type: EndpointType::Bulk,
            max_packet_size: 512,
            interval_ms: 0,
        }
    }
    impl Endpoint for In {
        fn info(&self) -> &EndpointInfo {
            &self.info
        }
        async fn wait_enabled(&mut self) {}
    }
    impl EndpointIn for In {
        async fn write(&mut self, buf: &[u8]) -> Result<(), EndpointError> {
            self.sent.push(buf.to_vec());
            Ok(())
        }
    }
    impl Endpoint for Out {
        fn info(&self) -> &EndpointInfo {
            &self.info
        }
        async fn wait_enabled(&mut self) {}
    }
    impl EndpointOut for Out {
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, EndpointError> {
            let p = self.queue.pop_front().ok_or(EndpointError::Disabled)?;
            let n = p.len().min(buf.len());
            buf[..n].copy_from_slice(&p[..n]);
            Ok(n)
        }
    }

    struct Ram(Vec<u8>);
    impl BlockDevice for Ram {
        type Error = Infallible;
        async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Infallible> {
            let o = lba as usize * 512;
            buf.copy_from_slice(&self.0[o..o + buf.len()]);
            Ok(())
        }
        async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), Infallible> {
            let o = lba as usize * 512;
            self.0[o..o + buf.len()].copy_from_slice(buf);
            Ok(())
        }
    }

    fn cbw(tag: u32, len: u32, dir_in: bool, cdb: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; 31];
        b[..4].copy_from_slice(&CBW_SIG.to_le_bytes());
        b[4..8].copy_from_slice(&tag.to_le_bytes());
        b[8..12].copy_from_slice(&len.to_le_bytes());
        b[12] = if dir_in { 0x80 } else { 0 };
        b[14] = cdb.len() as u8;
        b[15..15 + cdb.len()].copy_from_slice(cdb);
        b
    }

    fn unit() -> Unit {
        Unit {
            removable: true,
            block_size: 512,
            blocks: 64,
            read_only: false,
            present: true,
            vendor: *b"yotonrs ",
            product: *b"test            ",
            revision: *b"0001",
        }
    }

    #[test]
    fn write_then_read_back() {
        let data: Vec<u8> = (0..4 * 512).map(|i| (i * 7 % 251) as u8).collect();
        let mut q = VecDeque::new();
        q.push_back(cbw(1, 2048, false, &[0x2A, 0, 0, 0, 0, 5, 0, 0, 4, 0]));
        for p in data.chunks(512) {
            q.push_back(p.to_vec());
        }
        q.push_back(cbw(2, 2048, true, &[0x28, 0, 0, 0, 0, 5, 0, 0, 4, 0]));
        let mut buf = vec![0u8; 1024];
        let mut bot = Bot::new(
            In { info: info(Direction::In), sent: Vec::new() },
            Out { info: info(Direction::Out), queue: q },
            Ram(vec![0; 64 * 512]),
            unit(),
            &mut buf,
        );
        block_on(bot.serve_one()).unwrap();
        block_on(bot.serve_one()).unwrap();
        let sent: Vec<u8> = bot.ep_in.sent.concat();
        // CSW(1) + 2048 data + CSW(2)
        assert_eq!(sent.len(), 13 + 2048 + 13);
        assert_eq!(&sent[13..13 + 2048], &data[..]);
        assert_eq!(sent[12], 0);
        assert_eq!(sent[sent.len() - 1], 0);
        assert_eq!(bot.stats.bytes_written, 2048);
    }

    #[test]
    fn inquiry_short_and_residue() {
        let mut q = VecDeque::new();
        q.push_back(cbw(7, 96, true, &[0x12, 0, 0, 0, 96, 0]));
        let mut buf = vec![0u8; 512];
        let mut bot = Bot::new(
            In { info: info(Direction::In), sent: Vec::new() },
            Out { info: info(Direction::Out), queue: q },
            Ram(vec![0; 64 * 512]),
            unit(),
            &mut buf,
        );
        block_on(bot.serve_one()).unwrap();
        assert_eq!(bot.ep_in.sent[0].len(), 36);
        let c = &bot.ep_in.sent[1];
        assert_eq!(u32::from_le_bytes([c[8], c[9], c[10], c[11]]), 60);
    }

    #[test]
    fn bad_command_then_sense() {
        let mut q = VecDeque::new();
        q.push_back(cbw(1, 0, false, &[0xEE, 0, 0, 0, 0, 0]));
        q.push_back(cbw(2, 18, true, &[0x03, 0, 0, 0, 18, 0]));
        let mut buf = vec![0u8; 512];
        let mut bot = Bot::new(
            In { info: info(Direction::In), sent: Vec::new() },
            Out { info: info(Direction::Out), queue: q },
            Ram(vec![0; 64 * 512]),
            unit(),
            &mut buf,
        );
        block_on(bot.serve_one()).unwrap();
        assert_eq!(bot.ep_in.sent[0][12], 1);
        block_on(bot.serve_one()).unwrap();
        assert_eq!(bot.ep_in.sent[1][2], 0x05);
    }
}
