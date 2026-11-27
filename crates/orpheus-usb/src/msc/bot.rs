//! Bulk-Only Transport (USB MSC BBB) pump over `embassy-usb-driver`
//! endpoints (06-connectivity §2.4).
//!
//! One command at a time: read a 31-byte CBW, interpret the CDB with
//! [`Scsi`], move the data phase between the block device and the bulk
//! endpoints, answer with a 13-byte CSW. Block data moves through the two
//! halves of `buf` with the device and USB working at once (PERF-SD,
//! 06 §2.4); a sequential READ is read ahead by one half. Errors in the
//! data phase surface as CHECK CONDITION with sense data, and the host's
//! data phase is completed first (padding / draining, BOT §6.7).

use embassy_futures::join::join;
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

/// A bulk IN endpoint that sends a data piece of any length as one
/// transfer of consecutive packets (the last may be short). The ATJ2157's
/// controller takes up to 64 KiB per DMA start and packetises it itself
/// (06-connectivity §2.1), so the firmware's endpoint sends a piece in one
/// go; the default sends one max-size packet per `write`, which any
/// `embassy-usb-driver` endpoint can do (PERF-SD: the card reader sent
/// 512 B per DMA, one wait each, 2.3 MB/s).
#[allow(async_fn_in_trait)]
pub trait BulkIn: EndpointIn {
    async fn send(&mut self, buf: &[u8]) -> Result<(), EndpointError> {
        let mps = usize::from(self.info().max_packet_size).max(1);
        for p in buf.chunks(mps) {
            self.write(p).await?;
        }
        Ok(())
    }
}

/// A bulk OUT endpoint that receives a data piece of known length (a data
/// phase: whole max-size packets, the host's last one may be short) as one
/// transfer. Returns the bytes received, fewer than `buf.len()` when the
/// host ended with a short packet. The default reads packet by packet.
#[allow(async_fn_in_trait)]
pub trait BulkOut: EndpointOut {
    async fn receive(&mut self, buf: &mut [u8]) -> Result<usize, EndpointError> {
        let mps = usize::from(self.info().max_packet_size).max(1);
        let mut got = 0;
        while got < buf.len() {
            let end = (got + mps).min(buf.len());
            let k = self.read(&mut buf[got..end]).await?;
            got += k;
            if k < mps {
                break;
            }
        }
        Ok(got)
    }
}

/// Counters for diagnostics and the USB screen.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub commands: u32,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub errors: u32,
    pub phase_errors: u32,
    /// READs whose first piece had been read ahead.
    pub read_ahead_hits: u32,
}

/// Blocks read past the end of the last READ while its last piece went
/// out (06-connectivity §2.4): valid in buffer half `half`.
#[derive(Clone, Copy)]
struct Ahead {
    lba: u64,
    blocks: u32,
    half: u8,
}

/// How a block data phase ended.
enum Moved {
    All,
    /// The device failed after this many good bytes (READ: sent; WRITE:
    /// written); the host's data was still taken (WRITE).
    Failed(u32),
    /// The host ended the data with a short packet after this many bytes
    /// (WRITE): a phase error.
    Short(u32),
}

pub struct Bot<'b, I: BulkIn, O: BulkOut, B: BlockDevice> {
    pub ep_in: I,
    pub ep_out: O,
    pub dev: B,
    pub scsi: Scsi,
    pub unit: Unit,
    /// Data-phase buffer: two halves of whole blocks, one moving over USB
    /// while the device fills or empties the other (06 §2.4's double
    /// buffer). It must hold at least two blocks.
    buf: &'b mut [u8],
    ahead: Option<Ahead>,
    /// The block after the last READ (sequential reads are read ahead;
    /// a random one is not: it would cost the card a half for nothing).
    last_end: u64,
    pub stats: Stats,
}

impl<'b, I: BulkIn, O: BulkOut, B: BlockDevice> Bot<'b, I, O, B> {
    pub fn new(ep_in: I, ep_out: O, dev: B, unit: Unit, buf: &'b mut [u8]) -> Self {
        Self {
            ep_in,
            ep_out,
            dev,
            scsi: Scsi::new(),
            unit,
            buf,
            ahead: None,
            last_end: u64::MAX,
            stats: Stats::default(),
        }
    }

    /// Blocks per buffer half (0: the buffer holds fewer than two).
    fn per(&self) -> u32 {
        (self.buf.len() / 2 / self.unit.block_size.max(1) as usize) as u32
    }

    /// Serve one command. Returns `Err` only when the bus went away.
    pub async fn serve_one(&mut self) -> Result<(), EndpointError> {
        // CBW (word aligned: a DMA reaches it without a bounce).
        let mut cbw_buf = Words([0u8; 32]);
        let n = self.ep_out.read(&mut cbw_buf.0[..31]).await?;
        let Some(cbw) = Cbw::parse(&cbw_buf.0[..n]) else {
            self.stats.phase_errors += 1;
            return Ok(());
        };
        self.stats.commands += 1;
        let mut status = 0u8;
        // Bytes of the data phase that moved, and the good ones of them.
        let (mut moved, mut good) = (0u32, 0u32);
        let kind = {
            let unit = self.unit;
            match self.scsi.command(&unit, &cbw.cdb[..usize::from(cbw.cdb_len)]) {
                Plan::Done => Kind::None,
                Plan::Fail => Kind::Fail,
                Plan::DataIn(d) => {
                    let k = d.len().min(cbw.len as usize).min(self.buf.len());
                    self.buf[..k].copy_from_slice(&d[..k]);
                    Kind::Data(k)
                }
                Plan::DataInPadded { head, len } => {
                    let k = len.min(cbw.len as usize).min(self.buf.len());
                    let h = head.len().min(k);
                    self.buf[..h].copy_from_slice(&head[..h]);
                    self.buf[h..k].fill(0);
                    Kind::Data(k)
                }
                Plan::Read { lba, count } => Kind::Read(lba, count),
                Plan::Write { lba, count } => Kind::Write(lba, count),
            }
        };
        // Only a READ keeps (or makes) a read-ahead; anything else may
        // reuse the buffer or change the blocks.
        if !matches!(kind, Kind::Read(..) | Kind::None) {
            self.ahead = None;
        }
        match kind {
            Kind::None => {}
            Kind::Fail => status = 1,
            Kind::Data(k) => {
                if cbw.dir_in {
                    self.ep_in.send(&self.buf[..k]).await?;
                    (moved, good) = (k as u32, k as u32);
                } else {
                    status = 2;
                    self.stats.phase_errors += 1;
                }
            }
            Kind::Read(lba, count) | Kind::Write(lba, count) => {
                let write = matches!(kind, Kind::Write(..));
                let total = u64::from(count) * u64::from(self.unit.block_size);
                if cbw.dir_in == write || total > u64::from(cbw.len) || self.per() == 0 {
                    status = 2;
                    self.stats.phase_errors += 1;
                } else {
                    let total = total as u32;
                    let r = if write {
                        self.write_blocks(lba, count).await?
                    } else {
                        let r = self.read_blocks(lba, count).await?;
                        self.last_end = lba + u64::from(count);
                        r
                    };
                    match r {
                        Moved::All => (moved, good) = (total, total),
                        Moved::Failed(n) => {
                            status = 1;
                            self.stats.errors += 1;
                            self.scsi.set_sense(if write { Sense::WRITE_ERROR } else { Sense::READ_ERROR });
                            (moved, good) = (if write { total } else { n }, n);
                        }
                        Moved::Short(n) => {
                            status = 2;
                            self.stats.phase_errors += 1;
                            (moved, good) = (n, n);
                        }
                    }
                    if write {
                        self.stats.bytes_written += u64::from(good);
                    } else {
                        self.stats.bytes_read += u64::from(good);
                    }
                }
            }
        }
        // The host's data phase ends before the CSW (BOT §6.7, the
        // thirteen cases): an IN phase the device cut short on a packet
        // boundary (a failed command, an error mid-READ) is padded with
        // zeros, since a short packet is what ends it otherwise; OUT data
        // the device did not take is drained. Before PERF-SD a failed
        // READ went straight to the CSW, which the host then read as data.
        if status != 2 && moved < cbw.len {
            let rest = cbw.len - moved;
            let mps = u32::from(self.ep_in.info().max_packet_size).max(1);
            if cbw.dir_in && moved % mps == 0 {
                self.pad(rest).await?;
            } else if !cbw.dir_in {
                self.drain(rest).await?;
            }
        }
        let residue = cbw.len.saturating_sub(good);
        let c = Words(csw(cbw.tag, residue, status));
        self.ep_in.write(&c.0).await
    }

    /// Send `n` zero bytes (an IN data phase's padding).
    async fn pad(&mut self, mut n: u32) -> Result<(), EndpointError> {
        self.ahead = None;
        let k = self.buf.len().min(n as usize);
        self.buf[..k].fill(0);
        while n > 0 {
            let k = self.buf.len().min(n as usize);
            self.ep_in.send(&self.buf[..k]).await?;
            n -= k as u32;
        }
        Ok(())
    }

    /// Take and drop up to `n` bytes of OUT data (until a short packet).
    async fn drain(&mut self, mut n: u32) -> Result<(), EndpointError> {
        self.ahead = None;
        while n > 0 {
            let k = self.buf.len().min(n as usize);
            let got = self.ep_out.receive(&mut self.buf[..k]).await?;
            if got < k {
                break;
            }
            n -= k as u32;
        }
        Ok(())
    }

    /// READ: the device fills one half while the other goes out (both
    /// DMAs run at once); after the last piece of a sequential READ the
    /// next half's worth is read ahead.
    async fn read_blocks(&mut self, lba: u64, count: u32) -> Result<Moved, EndpointError> {
        let bs = self.unit.block_size as usize;
        let per = self.per();
        let h = per as usize * bs;
        let (end, sequential) = (self.unit.blocks, lba == self.last_end);
        let Self { ep_in, dev, buf, ahead, stats, .. } = self;
        let (x, y) = buf.split_at_mut(h);
        let mut halves = [x, &mut y[..h]];
        let mut n = count.min(per);
        let mut c = match ahead.take() {
            Some(a) if a.lba == lba && a.blocks >= n => {
                stats.read_ahead_hits += 1;
                usize::from(a.half)
            }
            _ => {
                if dev.read(lba, &mut halves[0][..n as usize * bs]).await.is_err() {
                    return Ok(Moved::Failed(0));
                }
                0
            }
        };
        let mut done = 0u32;
        loop {
            let next = lba + u64::from(done + n);
            let rest = count - done - n;
            let (m, past) = if rest > 0 {
                (rest.min(per), false)
            } else if sequential && next + u64::from(per) <= end {
                (per, true)
            } else {
                (0, false)
            };
            let [a, b] = &mut halves;
            let (cur, nxt): (&[u8], &mut [u8]) = if c == 0 { (a, b) } else { (b, a) };
            let piece = &cur[..n as usize * bs];
            let (sent, got) = if m > 0 {
                join(ep_in.send(piece), dev.read(next, &mut nxt[..m as usize * bs])).await
            } else {
                (ep_in.send(piece).await, Ok(()))
            };
            sent?;
            done += n;
            if past || m == 0 {
                if past && got.is_ok() {
                    *ahead = Some(Ahead { lba: next, blocks: per, half: (c ^ 1) as u8 });
                }
                return Ok(Moved::All);
            }
            if got.is_err() {
                return Ok(Moved::Failed(done * bs as u32));
            }
            n = m;
            c ^= 1;
        }
    }

    /// WRITE: the host fills one half while the device writes the other.
    /// After a device error the rest of the host's data is still taken
    /// (and dropped), so the CSW can report it.
    async fn write_blocks(&mut self, lba: u64, count: u32) -> Result<Moved, EndpointError> {
        let bs = self.unit.block_size as usize;
        let per = self.per();
        let h = per as usize * bs;
        let Self { ep_out, dev, buf, .. } = self;
        let (x, y) = buf.split_at_mut(h);
        let mut halves = [x, &mut y[..h]];
        let mut n = count.min(per);
        let len = n as usize * bs;
        let got = ep_out.receive(&mut halves[0][..len]).await?;
        if got != len {
            return Ok(Moved::Short(got as u32));
        }
        let (mut c, mut done, mut failed) = (0usize, 0u32, None);
        loop {
            let m = (count - done - n).min(per);
            let [a, b] = &mut halves;
            let (cur, nxt): (&[u8], &mut [u8]) = if c == 0 { (a, b) } else { (b, a) };
            let piece = &cur[..n as usize * bs];
            let skip = failed.is_some();
            let write = async { skip || dev.write(lba + u64::from(done), piece).await.is_ok() };
            let (ok, got) = if m > 0 {
                join(write, ep_out.receive(&mut nxt[..m as usize * bs])).await
            } else {
                (write.await, Ok(0))
            };
            if !ok {
                failed = Some(done * bs as u32);
            }
            done += n;
            if m == 0 {
                return Ok(failed.map_or(Moved::All, Moved::Failed));
            }
            let got = got?;
            if got != m as usize * bs {
                return Ok(Moved::Short(done * bs as u32 + got as u32));
            }
            n = m;
            c ^= 1;
        }
    }
}

/// Bytes on a word boundary (the CBW and CSW, for the controller's DMA).
#[repr(C, align(4))]
struct Words<const N: usize>([u8; N]);

#[derive(Clone, Copy)]
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
    use crate::msc::scsi::DeviceKind;
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
    impl BulkIn for In {}
    impl BulkOut for Out {}
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
            kind: DeviceKind::Disk,
            removable: true,
            block_size: 512,
            blocks: 64,
            read_only: false,
            present: true,
            vendor: *b"orpheus ",
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

    /// A fragmented ISO exported as a CD through the extent table: TOC,
    /// 2048-byte reads across runs, the padded DVD structure.
    #[test]
    fn cdrom_image_over_bot() {
        use crate::msc::extent::{Extent, ExtentTable, ImageDevice};
        // 10 blocks of 2048 = 40 card sectors, in 3 runs on the card.
        let mut card = std::vec![0u8; 64 * 512];
        let file: Vec<u8> = (0..10 * 2048).map(|i| (i / 7 % 256) as u8).collect();
        let runs = [(50u32, 0usize, 12usize), (20, 12, 20), (5, 32, 8)];
        for (lba, from, n) in runs {
            card[lba as usize * 512..(lba as usize + n) * 512]
                .copy_from_slice(&file[from * 512..(from + n) * 512]);
        }
        let mut st = [Extent::default(); 8];
        let mut t = ExtentTable::new(&mut st);
        for (lba, _, n) in runs {
            t.push(lba, n as u32).unwrap();
        }
        let card = Ram(card);
        let img = ImageDevice::new(card, t, file.len() as u64, 2048).unwrap();
        let unit = Unit::cdrom(file.len() as u64, *b"orpheus ", *b"USB Drive CD    ", *b"0001");
        let mut q = VecDeque::new();
        q.push_back(cbw(1, 12, true, &[0x43, 0, 0, 0, 0, 0, 0xAA, 0, 12, 0]));
        q.push_back(cbw(2, 4 * 2048, true, &[0x28, 0, 0, 0, 0, 4, 0, 0, 4, 0]));
        q.push_back(cbw(3, 2052, true, &[0xAD, 0, 0, 0, 0, 0, 0, 0, 0x08, 0x04, 0, 0]));
        let mut buf = std::vec![0u8; 4096];
        let mut bot = Bot::new(
            In { info: info(Direction::In), sent: Vec::new() },
            Out { info: info(Direction::Out), queue: q },
            img,
            unit,
            &mut buf,
        );
        for _ in 0..3 {
            block_on(bot.serve_one()).unwrap();
        }
        let sent = &bot.ep_in.sent;
        assert_eq!(sent[0], [0, 10, 1, 1, 0, 0x14, 0xAA, 0, 0, 0, 0, 10]);
        let all: Vec<u8> = sent[2..].concat();
        assert_eq!(all.len(), 4 * 2048 + 13 + 2052 + 13);
        assert_eq!(&all[..4 * 2048], &file[4 * 2048..8 * 2048]);
        // A CD-sized image: READ DVD STRUCTURE fails with sense; the host's
        // 2052-byte IN phase is padded with zeros (BOT §6.7), the residue
        // says none of it is data.
        let pad = &all[4 * 2048 + 13..4 * 2048 + 13 + 2052];
        assert!(pad.iter().all(|&b| b == 0));
        let last = &sent[sent.len() - 1];
        assert_eq!((last[12], u32::from_le_bytes([last[8], last[9], last[10], last[11]])), (1, 2052));
        assert_eq!(bot.scsi.sense().asc, 0x30);
    }

    #[test]
    fn padded_data_in() {
        let unit = Unit::cdrom(2 * 1024 * 1024 * 1024, *b"orpheus ", *b"USB Drive DVD   ", *b"0001");
        let mut q = VecDeque::new();
        q.push_back(cbw(1, 2052, true, &[0xAD, 0, 0, 0, 0, 0, 0, 0, 0x08, 0x04, 0, 0]));
        q.push_back(cbw(2, 100, true, &[0xAD, 0, 0, 0, 0, 0, 0, 0, 0x08, 0x04, 0, 0]));
        let mut buf = std::vec![0xAAu8; 4096];
        let mut bot = Bot::new(
            In { info: info(Direction::In), sent: Vec::new() },
            Out { info: info(Direction::Out), queue: q },
            Ram(Vec::new()),
            unit,
            &mut buf,
        );
        block_on(bot.serve_one()).unwrap();
        block_on(bot.serve_one()).unwrap();
        let sent = &bot.ep_in.sent;
        let first: Vec<u8> = sent[..5].concat();
        assert_eq!(first.len(), 2052);
        assert_eq!(&first[..2], &[0x08, 0x02]);
        assert!(first[21..].iter().all(|&b| b == 0));
        assert_eq!(sent[5][12], 0);
        // Host allocated 100 bytes: 100 sent, no residue.
        assert_eq!(sent[6].len(), 100);
        assert_eq!(&sent[7][8..13], &[0, 0, 0, 0, 0]);
    }

    // ---- PERF-SD: the double-buffered pipeline -------------------------

    use core::cell::RefCell;
    use core::future::poll_fn;
    use core::task::Poll;
    use std::rc::Rc;
    use std::string::{String, ToString};

    type Log = Rc<RefCell<Vec<String>>>;

    /// Pending once, then ready: lets `join` run the other side.
    async fn yield_once() {
        let mut first = true;
        poll_fn(|cx| {
            if first {
                first = false;
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
    }

    /// Endpoints and a device that log when each transfer starts and ends
    /// and yield in between, as the hardware's DMAs do.
    struct LIn {
        info: EndpointInfo,
        log: Log,
        sent: Vec<Vec<u8>>,
    }
    struct LOut {
        info: EndpointInfo,
        log: Log,
        queue: VecDeque<Vec<u8>>,
    }
    struct LDev {
        data: Vec<u8>,
        log: Log,
        /// Fail the read or write that touches this block.
        bad: Option<u64>,
        reads: u32,
    }
    impl Endpoint for LIn {
        fn info(&self) -> &EndpointInfo {
            &self.info
        }
        async fn wait_enabled(&mut self) {}
    }
    impl EndpointIn for LIn {
        async fn write(&mut self, buf: &[u8]) -> Result<(), EndpointError> {
            self.sent.push(buf.to_vec());
            Ok(())
        }
    }
    impl BulkIn for LIn {
        async fn send(&mut self, buf: &[u8]) -> Result<(), EndpointError> {
            self.log.borrow_mut().push(std::format!("send {} start", buf.len()));
            yield_once().await;
            self.sent.push(buf.to_vec());
            self.log.borrow_mut().push("send end".to_string());
            Ok(())
        }
    }
    impl Endpoint for LOut {
        fn info(&self) -> &EndpointInfo {
            &self.info
        }
        async fn wait_enabled(&mut self) {}
    }
    impl EndpointOut for LOut {
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, EndpointError> {
            let p = self.queue.pop_front().ok_or(EndpointError::Disabled)?;
            let n = p.len().min(buf.len());
            buf[..n].copy_from_slice(&p[..n]);
            Ok(n)
        }
    }
    impl BulkOut for LOut {
        async fn receive(&mut self, buf: &mut [u8]) -> Result<usize, EndpointError> {
            self.log.borrow_mut().push(std::format!("recv {} start", buf.len()));
            yield_once().await;
            let mut got = 0;
            while got < buf.len() {
                let p = self.queue.pop_front().ok_or(EndpointError::Disabled)?;
                buf[got..got + p.len()].copy_from_slice(&p);
                got += p.len();
                if p.len() < 512 {
                    break;
                }
            }
            self.log.borrow_mut().push("recv end".to_string());
            Ok(got)
        }
    }
    impl BlockDevice for LDev {
        type Error = ();
        async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
            self.reads += 1;
            self.log.borrow_mut().push(std::format!("read {lba} start"));
            yield_once().await;
            self.log.borrow_mut().push("read end".to_string());
            let n = buf.len() as u64 / 512;
            if self.bad.is_some_and(|b| (lba..lba + n).contains(&b)) {
                return Err(());
            }
            let o = lba as usize * 512;
            buf.copy_from_slice(&self.data[o..o + buf.len()]);
            Ok(())
        }
        async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), ()> {
            self.log.borrow_mut().push(std::format!("write {lba} start"));
            yield_once().await;
            self.log.borrow_mut().push("write end".to_string());
            let n = buf.len() as u64 / 512;
            if self.bad.is_some_and(|b| (lba..lba + n).contains(&b)) {
                return Err(());
            }
            let o = lba as usize * 512;
            self.data[o..o + buf.len()].copy_from_slice(buf);
            Ok(())
        }
    }

    fn piped(blocks: usize, buf: &mut [u8]) -> (Bot<'_, LIn, LOut, LDev>, Log) {
        let log: Log = Rc::default();
        let data = (0..blocks * 512).map(|i| (i * 13 % 251) as u8).collect();
        let mut u = unit();
        u.blocks = blocks as u64;
        let bot = Bot::new(
            LIn { info: info(Direction::In), log: log.clone(), sent: Vec::new() },
            LOut { info: info(Direction::Out), log: log.clone(), queue: VecDeque::new() },
            LDev { data, log: log.clone(), bad: None, reads: 0 },
            u,
            buf,
        );
        (bot, log)
    }

    fn rw10(op: u8, lba: u32, n: u16) -> [u8; 10] {
        let l = lba.to_be_bytes();
        let c = n.to_be_bytes();
        [op, 0, l[0], l[1], l[2], l[3], 0, c[0], c[1], 0]
    }

    /// Serve one queued command; the bytes sent (data, CSW) and the CSW.
    fn serve(bot: &mut Bot<'_, LIn, LOut, LDev>, cbw_bytes: Vec<u8>, data: &[u8]) -> (Vec<u8>, [u8; 13]) {
        bot.ep_out.queue.push_back(cbw_bytes);
        for p in data.chunks(512) {
            bot.ep_out.queue.push_back(p.to_vec());
        }
        let from = bot.ep_in.sent.len();
        block_on(bot.serve_one()).unwrap();
        let all: Vec<u8> = bot.ep_in.sent[from..].concat();
        let (body, c) = all.split_at(all.len() - 13);
        (body.to_vec(), c.try_into().unwrap())
    }

    fn residue(c: &[u8; 13]) -> u32 {
        u32::from_le_bytes([c[8], c[9], c[10], c[11]])
    }

    /// A READ of five halves: each half goes out while the device reads
    /// the next (the log shows the next read start before the send ends),
    /// in whole halves, not packets.
    ///
    /// PERF-SD, 2026-11-27: the card reader sent each 512-byte packet
    /// with a DMA and a wait of its own and read the card and sent to the
    /// host one after the other (2.3 MB/s; 06 §2.4 wants ≥ 10 MB/s).
    #[test]
    fn regress_card_reader_sent_512_bytes_per_transfer() {
        let mut buf = vec![0u8; 8 * 512];
        let (mut bot, log) = piped(64, &mut buf);
        let (d, c) = serve(&mut bot, cbw(1, 20 * 512, true, &rw10(0x28, 3, 20)), &[]);
        assert_eq!(c[12], 0);
        assert_eq!(d, bot.dev.data[3 * 512..23 * 512]);
        let sizes: Vec<usize> = bot.ep_in.sent.iter().map(Vec::len).collect();
        assert_eq!(sizes, [2048, 2048, 2048, 2048, 2048, 13]);
        let l = log.borrow();
        // read 3, then (send 0 with read 7), (send 1 with read 11), ...
        assert_eq!(&l[..2], ["read 3 start", "read end"]);
        for (k, lba) in [7, 11, 15, 19].iter().enumerate() {
            let at = 2 + 4 * k;
            assert_eq!(l[at], "send 2048 start");
            assert_eq!(l[at + 1], std::format!("read {lba} start"));
        }
        // The first READ of a run is not read ahead.
        assert_eq!(bot.dev.reads, 5);
    }

    /// Sequential READs: the next half was read while the last piece went
    /// out, so the next READ starts from it; a READ elsewhere is not read
    /// ahead (it would cost the card a half for nothing).
    #[test]
    fn sequential_reads_are_read_ahead_random_ones_not() {
        let mut buf = vec![0u8; 8 * 512];
        let (mut bot, _) = piped(64, &mut buf);
        for (i, lba) in [0u32, 4, 8, 12].iter().enumerate() {
            let (d, c) = serve(&mut bot, cbw(i as u32, 2048, true, &rw10(0x28, *lba, 4)), &[]);
            assert_eq!(c[12], 0);
            assert_eq!(d, bot.dev.data[*lba as usize * 512..(*lba as usize + 4) * 512]);
        }
        // READ 0 (1 read), READ 4 (1 read + read-ahead 8), READ 8 (hit,
        // read-ahead 12), READ 12 (hit, read-ahead 16).
        assert_eq!(bot.stats.read_ahead_hits, 2);
        assert_eq!(bot.dev.reads, 5);
        let reads = bot.dev.reads;
        let _ = serve(&mut bot, cbw(9, 2048, true, &rw10(0x28, 40, 4)), &[]);
        let _ = serve(&mut bot, cbw(9, 2048, true, &rw10(0x28, 20, 4)), &[]);
        assert_eq!(bot.dev.reads, reads + 2, "random reads read nothing ahead");
        // A WRITE drops a read-ahead (it may change those blocks).
        let _ = serve(&mut bot, cbw(10, 2048, true, &rw10(0x28, 24, 4)), &[]);
        let new = vec![0xEEu8; 2048];
        let (_, c) = serve(&mut bot, cbw(11, 2048, false, &rw10(0x2A, 28, 4)), &new);
        assert_eq!(c[12], 0);
        let (d, _) = serve(&mut bot, cbw(12, 2048, true, &rw10(0x28, 28, 4)), &[]);
        assert_eq!(d, new);
    }

    /// A WRITE of several halves: the device writes one half while the
    /// next is received.
    #[test]
    fn writes_overlap_the_next_half() {
        let mut buf = vec![0u8; 8 * 512];
        let (mut bot, log) = piped(64, &mut buf);
        let data: Vec<u8> = (0..12 * 512).map(|i| (i % 7) as u8).collect();
        let (d, c) = serve(&mut bot, cbw(1, 12 * 512, false, &rw10(0x2A, 10, 12)), &data);
        assert!(d.is_empty());
        assert_eq!((c[12], residue(&c)), (0, 0));
        assert_eq!(bot.dev.data[10 * 512..22 * 512], data[..]);
        let l = log.borrow();
        assert_eq!(&l[..4], ["recv 2048 start", "recv end", "write 10 start", "recv 2048 start"]);
        assert_eq!(bot.stats.bytes_written, 12 * 512);
    }

    /// A read error in the middle of a READ: the good pieces, zeros for
    /// the rest of the host's IN phase, CHECK CONDITION with the good byte
    /// count in the residue; the next command is understood.
    ///
    /// PERF-SD, 2026-11-27: a READ that failed went straight to the CSW;
    /// the host, still in its IN data phase, took the 13-byte CSW as data
    /// and then waited for a CSW (BOT §6.7 case 5).
    #[test]
    fn regress_failed_read_skipped_the_data_phase() {
        let mut buf = vec![0u8; 8 * 512];
        let (mut bot, _) = piped(64, &mut buf);
        bot.dev.bad = Some(9);
        let (d, c) = serve(&mut bot, cbw(1, 16 * 512, true, &rw10(0x28, 0, 16)), &[]);
        assert_eq!(d.len(), 16 * 512, "the whole IN phase");
        assert_eq!(d[..8 * 512], bot.dev.data[..8 * 512]);
        assert!(d[8 * 512..].iter().all(|&b| b == 0));
        assert_eq!((c[12], residue(&c)), (1, 8 * 512));
        let (s, _) = serve(&mut bot, cbw(2, 18, true, &[0x03, 0, 0, 0, 18, 0]), &[]);
        assert_eq!((s[2], s[12]), (0x03, 0x11));
        // Failed before any data (the first piece): all zeros.
        bot.dev.bad = Some(0);
        let (d, c) = serve(&mut bot, cbw(3, 4 * 512, true, &rw10(0x28, 0, 4)), &[]);
        assert_eq!((d.len(), c[12], residue(&c)), (4 * 512, 1, 4 * 512));
    }

    /// A refused WRITE (write-protected card reader, out of range): its
    /// data is taken and dropped, so the next CBW is read as a CBW.
    ///
    /// PERF-SD, 2026-11-27: a WRITE refused before its data phase (`usb
    /// msc ro`) answered at once and left the host's data in the pipe; the
    /// next "CBW" read was a data packet (phase error, the host's reset
    /// recovery).
    #[test]
    fn regress_refused_write_left_its_data_for_the_next_cbw() {
        let mut buf = vec![0u8; 8 * 512];
        let (mut bot, _) = piped(64, &mut buf);
        bot.unit.read_only = true;
        let before = bot.dev.data.clone();
        let data = vec![0x55u8; 6 * 512];
        let (_, c) = serve(&mut bot, cbw(1, 6 * 512, false, &rw10(0x2A, 0, 6)), &data);
        assert_eq!((c[12], residue(&c)), (1, 6 * 512));
        assert!(bot.ep_out.queue.is_empty(), "the refused data was taken");
        let (_, c) = serve(&mut bot, cbw(2, 0, false, &[0, 0, 0, 0, 0, 0]), &[]);
        assert_eq!((c[12], &c[4..8]), (0, &2u32.to_le_bytes()[..]));
        assert_eq!(bot.stats.phase_errors, 0);
        assert_eq!(bot.dev.data, before);
    }

    /// A write error in the middle: the rest of the host's data is taken,
    /// nothing after the failure is written, the residue counts the good
    /// bytes, and the sense says WRITE ERROR.
    #[test]
    fn write_error_mid_transfer_drains_and_reports() {
        let mut buf = vec![0u8; 8 * 512];
        let (mut bot, _) = piped(64, &mut buf);
        bot.dev.bad = Some(5);
        let before = bot.dev.data.clone();
        let data = vec![0x77u8; 12 * 512];
        let (_, c) = serve(&mut bot, cbw(1, 12 * 512, false, &rw10(0x2A, 0, 12)), &data);
        assert_eq!((c[12], residue(&c)), (1, 8 * 512));
        assert!(bot.ep_out.queue.is_empty());
        assert!(bot.dev.data[..4 * 512].iter().all(|&b| b == 0x77));
        assert_eq!(bot.dev.data[4 * 512..], before[4 * 512..], "nothing from the failure on");
        let (s, _) = serve(&mut bot, cbw(2, 18, true, &[0x03, 0, 0, 0, 18, 0]), &[]);
        assert_eq!((s[2], s[12]), (0x03, 0x0C));
    }

    /// The host's OUT data ends early (a short packet): phase error, and
    /// nothing after it is written.
    #[test]
    fn short_write_data_is_a_phase_error() {
        let mut buf = vec![0u8; 8 * 512];
        let (mut bot, _) = piped(64, &mut buf);
        let before = bot.dev.data.clone();
        let data = vec![0x11u8; 5 * 512 + 100];
        let (_, c) = serve(&mut bot, cbw(1, 8 * 512, false, &rw10(0x2A, 0, 8)), &data);
        assert_eq!(c[12], 2);
        assert_eq!(bot.dev.data[4 * 512..], before[4 * 512..]);
    }
}
