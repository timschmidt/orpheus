//! Fuzz-target bodies of `orpheus-usb` (VER-4), shared by the cargo-fuzz
//! targets in `fuzz_targets/` (nightly) and the stable replay of the
//! committed corpus (`tests/fuzz_corpus.rs`, every `cargo test`).
//!
//! * [`usb_scsi`] — a USB host's bulk-OUT traffic (CBWs, data, garbage)
//!   through the Bulk-Only Transport and the SCSI/MMC interpreter of a
//!   disk, read-only disk, absent medium or CD/DVD-ROM unit. Oracles: no
//!   panic; the block device is only accessed inside the medium and never
//!   written on a read-only unit or without a medium; every CSW carries the
//!   CBW's tag, a valid status and a residue within the transfer length;
//!   data sent never exceeds the transfer length; at the SCSI level a read
//!   or write plan stays inside the medium, a failure records sense data,
//!   standard commands respect their allocation length, and a padded reply
//!   is no shorter than its head.
//! * [`usb_devchan`] — developer-channel control requests (setup packet and
//!   data stage). Oracles: no panic; only vendor requests to the device are
//!   claimed; replies never exceed `wLength` or the buffer; PEEK reaches
//!   memory only in development builds; reboots happen only for REBOOT;
//!   key events are the data's pairs (at most 16); INFO round-trips.

#![forbid(unsafe_code)]

use std::collections::VecDeque;

use embassy_futures::block_on;
use embassy_usb::Handler;
use embassy_usb::control::{InResponse, OutResponse, Recipient, Request, RequestType};
use embassy_usb_driver::{
    Direction, Endpoint, EndpointAddress, EndpointError, EndpointIn, EndpointInfo, EndpointOut, EndpointType,
};
use orpheus_proto::devchan::{self as dc, Info};
use orpheus_usb::devchan::{DevChannel, Target};
use orpheus_usb::msc::BlockDevice;
use orpheus_usb::msc::bot::{Bot, CBW_SIG, CSW_SIG, Cbw};
use orpheus_usb::msc::scsi::{DeviceKind, Plan, Scsi, Sense, Unit};

/// Byte reader over the fuzz input.
pub struct Ops<'a>(pub &'a [u8]);

impl Ops<'_> {
    pub fn byte(&mut self) -> Option<u8> {
        let (&b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(b)
    }
    pub fn b(&mut self) -> u8 {
        self.byte().unwrap_or(0)
    }
    pub fn take(&mut self, n: usize) -> &[u8] {
        let n = n.min(self.0.len());
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        a
    }
}

fn info(dir: Direction, mps: u16) -> EndpointInfo {
    EndpointInfo {
        addr: EndpointAddress::from_parts(1, dir),
        ep_type: EndpointType::Bulk,
        max_packet_size: mps,
        interval_ms: 0,
    }
}

/// Bulk IN: records what the device sends.
pub struct In {
    info: EndpointInfo,
    pub sent: Vec<Vec<u8>>,
    /// Bytes taken so far: past [`HOST_PATIENCE`] the host gives up and
    /// resets the bus, as a real one does long before (Linux after 30 s);
    /// a CBW may ask for an IN phase of 4 GiB, which the transport pads
    /// with zeros (BOT §6.7) — 3.5 GB of packets exhausted the fuzzer's
    /// memory (`usb_scsi` OOM, 2026-11-28).
    taken: usize,
}

/// What the fake host takes before it resets the bus.
const HOST_PATIENCE: usize = 64 << 20;

impl Endpoint for In {
    fn info(&self) -> &EndpointInfo {
        &self.info
    }
    async fn wait_enabled(&mut self) {}
}

impl EndpointIn for In {
    async fn write(&mut self, buf: &[u8]) -> Result<(), EndpointError> {
        assert!(buf.len() <= usize::from(self.info.max_packet_size), "IN packet over the packet size");
        self.taken += buf.len();
        if self.taken > HOST_PATIENCE {
            return Err(EndpointError::Disabled);
        }
        self.sent.push(buf.to_vec());
        Ok(())
    }
}

impl orpheus_usb::msc::bot::BulkIn for In {}
impl orpheus_usb::msc::bot::BulkOut for Out {}

/// Bulk OUT: the host's packets; empty = the bus went away.
pub struct Out {
    info: EndpointInfo,
    pub queue: VecDeque<Vec<u8>>,
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

/// The medium: checks every access against the unit's size (`blocks`);
/// only the first blocks are stored, the rest read as zeros.
pub struct Ram {
    pub data: Vec<u8>,
    pub blocks: u64,
    pub block: usize,
    pub read_only: bool,
    pub present: bool,
    pub writes: u32,
}

impl BlockDevice for Ram {
    type Error = ();
    async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
        assert!(self.present, "read without a medium");
        assert!(buf.len() % self.block == 0, "partial block read");
        let n = (buf.len() / self.block) as u64;
        assert!(lba + n <= self.blocks, "read past the medium: lba {lba} + {n}");
        let o = usize::try_from(lba).expect("lba") * self.block;
        match self.data.get(o..o + buf.len()) {
            Some(src) => buf.copy_from_slice(src),
            None => buf.fill(0),
        }
        Ok(())
    }
    async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), ()> {
        assert!(self.present && !self.read_only, "write to a read-only or absent medium");
        assert!(buf.len() % self.block == 0, "partial block write");
        let n = (buf.len() / self.block) as u64;
        assert!(lba + n <= self.blocks, "write past the medium: lba {lba} + {n}");
        let o = usize::try_from(lba).expect("lba") * self.block;
        let dst = self.data.get_mut(o..o + buf.len()).expect("writable units are fully stored");
        dst.copy_from_slice(buf);
        self.writes += 1;
        Ok(())
    }
}

/// The unit byte 0 of the input describes.
#[must_use]
pub fn unit_of(sel: u8) -> Unit {
    let base = Unit {
        kind: DeviceKind::Disk,
        removable: sel & 0x10 != 0,
        block_size: 512,
        blocks: 64,
        read_only: false,
        present: true,
        vendor: *b"orpheus ",
        product: *b"fuzz unit       ",
        revision: *b"0001",
    };
    match sel % 5 {
        0 => base,
        1 => Unit { read_only: true, ..base },
        2 => Unit { present: false, ..base },
        3 => Unit::cdrom(23 * 2048 + u64::from(sel >> 5) * 700, base.vendor, base.product, base.revision),
        // A DVD-sized image (only its first blocks are stored).
        _ => Unit::cdrom(1 << 31, base.vendor, base.product, base.revision),
    }
}

/// Allocation length of the standard commands that have one, when the CDB
/// is long enough to hold it (a cut-off REQUEST SENSE gets its 18 bytes by
/// design; the transport cuts every reply to the transfer length).
fn alloc_len(cdb: &[u8]) -> Option<usize> {
    let g = |i: usize| usize::from(cdb[i]);
    Some(match cdb.first()? {
        0x03 if cdb.len() > 4 => g(4),
        0x12 if cdb.len() > 4 => g(3) << 8 | g(4),
        0x1A if cdb.len() > 4 => g(4),
        0x5A if cdb.len() > 8 => g(7) << 8 | g(8),
        0xA0 if cdb.len() > 9 => g(6) << 24 | g(7) << 16 | g(8) << 8 | g(9),
        _ => return None,
    })
}

/// One CDB through the interpreter alone, with the plan-level oracles.
pub fn check_plan(s: &mut Scsi, u: &Unit, cdb: &[u8]) {
    let p = s.command(u, cdb);
    match p {
        Plan::Read { lba, count } | Plan::Write { lba, count } => {
            assert!(u.present, "medium access without a medium: {cdb:02X?}");
            assert!(count > 0, "empty transfer planned: {cdb:02X?}");
            assert!(lba + u64::from(count) <= u.blocks, "{p:?} past {} blocks: {cdb:02X?}", u.blocks);
            if matches!(p, Plan::Write { .. }) {
                assert!(!u.read_only, "write planned on a read-only unit: {cdb:02X?}");
            }
        }
        Plan::DataIn(d) => {
            assert!(d.len() <= 128);
            if let Some(a) = alloc_len(cdb) {
                assert!(d.len() <= a, "{} bytes for allocation length {a}: {cdb:02X?}", d.len());
            }
        }
        Plan::DataInPadded { head, len } => {
            assert!(head.len() <= len, "padded reply shorter than its head");
            if let Some(a) = alloc_len(cdb) {
                assert!(len <= a);
            }
        }
        Plan::Fail => assert_ne!(s.sense(), Sense::NONE, "a failure without sense data: {cdb:02X?}"),
        Plan::Done => {}
    }
}

fn cbw_bytes(tag: u32, len: u32, dir_in: bool, lun: u8, cdb: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; 31];
    b[..4].copy_from_slice(&CBW_SIG.to_le_bytes());
    b[4..8].copy_from_slice(&tag.to_le_bytes());
    b[8..12].copy_from_slice(&len.to_le_bytes());
    b[12] = if dir_in { 0x80 } else { 0 };
    b[13] = lun;
    b[14] = cdb.len() as u8;
    b[15..15 + cdb.len()].copy_from_slice(cdb);
    b
}

/// Bulk-Only Transport + SCSI (see the crate docs). Input: unit selector,
/// then packets: `k < 0xC0`: a CBW (`k` = direction, CDB length), its
/// transfer length (2 bytes, × 512 when `k & 0x40`), the CDB, then for
/// OUT transfers the data packets (a count, then per packet a length byte:
/// 0 or not the last = a full packet); `k >= 0xC0`: a raw packet of the next
/// byte's length.
pub fn usb_scsi(data: &[u8]) {
    let mut ops = Ops(data);
    let sel = ops.b();
    let unit = unit_of(sel);
    let mps: u16 = if sel & 0x80 != 0 { 64 } else { 512 };
    let block = unit.block_size as usize;
    let size = usize::try_from(unit.blocks).unwrap_or(0).min(64) * block;
    let pattern: Vec<u8> = (0..size).map(|i| (i * 7 % 251) as u8).collect();
    let mut queue = VecDeque::new();
    let mut plan = Scsi::new();
    let mut tag = 0u32;
    while let Some(k) = ops.byte() {
        if queue.len() > 400 {
            break;
        }
        if k >= 0xC0 {
            let n = usize::from(ops.b() % 64);
            queue.push_back(ops.take(n).to_vec());
            continue;
        }
        let dir_in = k & 0x80 != 0;
        let lo = u32::from(ops.b()) | u32::from(ops.b()) << 8;
        let len = if k & 0x40 != 0 { lo * 512 } else { lo };
        let cdb_len = 1 + usize::from(k & 0x0F);
        let cdb = ops.take(cdb_len).to_vec();
        if cdb.is_empty() {
            break;
        }
        tag = tag.wrapping_add(0x0101_0101);
        check_plan(&mut plan, &unit, &cdb);
        queue.push_back(cbw_bytes(tag, len, dir_in, (k >> 4) & 1, &cdb));
        if !dir_in {
            let packets = usize::from(ops.b() % 24);
            for i in 0..packets {
                let last = ops.b();
                let n = match (i + 1 == packets, last) {
                    (true, 1..) => usize::from(last) % usize::from(mps),
                    _ => usize::from(mps),
                };
                queue.push_back(vec![(i as u8).wrapping_mul(13); n]);
            }
        }
    }
    let ram = Ram {
        data: pattern.clone(),
        blocks: unit.blocks,
        block,
        read_only: unit.read_only,
        present: unit.present,
        writes: 0,
    };
    let mut buf = vec![0u8; 4096];
    let mut bot = Bot::new(
        In { info: info(Direction::In, mps), sent: Vec::new(), taken: 0 },
        Out { info: info(Direction::Out, mps), queue },
        ram,
        unit,
        &mut buf,
    );
    for _ in 0..1000 {
        // The transport reads a CBW into 31 bytes (a longer packet is cut).
        let head = bot.ep_out.queue.front().map(|h| h[..h.len().min(31)].to_vec());
        let cmds = bot.stats.commands;
        let sent = bot.ep_in.sent.len();
        if block_on(bot.serve_one()).is_err() {
            break;
        }
        let new = &bot.ep_in.sent[sent..];
        if bot.stats.commands == cmds {
            assert!(new.is_empty(), "a reply to an invalid CBW");
            continue;
        }
        let cbw = head.as_deref().and_then(Cbw::parse).expect("a command was served from a valid CBW");
        let (csw, data_out) = new.split_last().expect("every command ends in a CSW");
        assert_eq!(csw.len(), 13, "CSW length");
        assert_eq!(&csw[..4], &CSW_SIG.to_le_bytes());
        assert_eq!(&csw[4..8], &cbw.tag.to_le_bytes(), "CSW tag");
        assert!(csw[12] <= 2, "CSW status {}", csw[12]);
        let residue = u32::from_le_bytes([csw[8], csw[9], csw[10], csw[11]]);
        assert!(residue <= cbw.len, "residue {residue} over the transfer length {}", cbw.len);
        let sent_bytes: usize = data_out.iter().map(Vec::len).sum();
        assert!(sent_bytes <= cbw.len as usize, "{sent_bytes} bytes sent for a {} byte transfer", cbw.len);
        if !cbw.dir_in {
            assert_eq!(sent_bytes, 0, "data sent for an OUT transfer");
        }
        if csw[12] == 0 && cbw.dir_in {
            // The data, then (BOT §6.7, Hi > Di) zeros up to the transfer
            // length when the data ended on a packet boundary.
            let data = cbw.len as usize - residue as usize;
            if sent_bytes != data {
                assert_eq!(sent_bytes, cbw.len as usize, "padding stops short of the transfer length");
                assert_eq!(data % usize::from(mps), 0, "padding after a short packet");
                let all: Vec<u8> = data_out.concat();
                assert!(all[data..].iter().all(|&b| b == 0), "padding that is not zeros");
            }
        }
        if csw[12] == 1 && cbw.dir_in && sent_bytes > 0 {
            // A failed IN phase is completed with zeros too.
            assert!(sent_bytes == cbw.len as usize || sent_bytes % usize::from(mps) != 0);
        }
    }
    if bot.unit.read_only || !bot.unit.present {
        assert_eq!(bot.dev.writes, 0);
        assert_eq!(bot.dev.data, pattern, "a read-only medium changed");
    }
}

/// What the developer channel did.
#[derive(Default)]
pub struct Fake {
    pub development: bool,
    pub reboots: Vec<u16>,
    pub keys: Vec<(u8, bool)>,
    pub peeks: u32,
    pub put: Vec<u8>,
    pub test: Vec<u8>,
    pub time: Option<[u8; 7]>,
}

impl Target for Fake {
    fn info(&mut self) -> Info {
        Info { version: dc::VERSION, uptime_ms: 42, battery_mv: 3900, build_id: [7; 16], ..Info::default() }
    }
    fn read_mem(&mut self, addr: u32, buf: &mut [u8]) -> bool {
        assert!(self.development, "PEEK outside a development build");
        self.peeks += 1;
        for (i, b) in buf.iter_mut().enumerate() {
            *b = addr.wrapping_add(i as u32) as u8;
        }
        addr % 3 != 0
    }
    fn read_frame(&mut self, offset: u32, buf: &mut [u8]) -> usize {
        let n = (128 * 128 * 2usize).saturating_sub(offset as usize).min(buf.len());
        buf[..n].fill(0x5A);
        n
    }
    fn read_log(&mut self, offset: u32, buf: &mut [u8]) -> usize {
        let n = 2048usize.saturating_sub(offset as usize).min(buf.len());
        buf[..n].fill(0xA5);
        n
    }
    fn stats(&mut self, group: u16, out: &mut [u32]) -> usize {
        let n = usize::from(group % 70).min(out.len());
        for (i, w) in out[..n].iter_mut().enumerate() {
            *w = i as u32;
        }
        n
    }
    fn keys(&mut self, events: &[(u8, bool)]) {
        self.keys.extend_from_slice(events);
    }
    fn request_reboot(&mut self, target: u16) {
        self.reboots.push(target);
    }
    fn development(&self) -> bool {
        self.development
    }
    fn put(&mut self, _seq: u16, data: &[u8]) -> bool {
        self.put.extend_from_slice(data);
        self.put.len() < 4096
    }
    fn set_time(&mut self, t: [u8; 7]) -> bool {
        self.time = Some(t);
        t[2] <= 12
    }
    fn time(&mut self) -> Option<[u8; 7]> {
        self.time
    }
    fn test_command(&mut self, data: &[u8]) -> bool {
        self.test = data.to_vec();
        !data.is_empty()
    }
    fn test_reply(&mut self, buf: &mut [u8]) -> usize {
        let n = self.test.len().min(buf.len());
        buf[..n].copy_from_slice(&self.test[..n]);
        n
    }
}

/// Developer-channel requests (see the crate docs). Input: byte 0 = flags
/// (bit 0 development build), then per request an 8-byte setup packet, a
/// data length byte and the data (OUT) or the IN buffer size (byte × 4).
pub fn usb_devchan(data: &[u8]) {
    let mut ops = Ops(data);
    let flags = ops.b();
    let mut ch = DevChannel::new(Fake { development: flags & 1 != 0, ..Fake::default() });
    let b = Info { version: 3, layout: 2, uptime_ms: 77, ..Info::default() };
    assert_eq!(Info::from_bytes(&b.to_bytes()), Some(b));
    let mut n = 0;
    while ops.0.len() >= 9 {
        n += 1;
        if n > 200 {
            break;
        }
        let setup: [u8; 8] = ops.take(8).try_into().expect("8 bytes");
        let req = Request::parse(&setup);
        let len = ops.b();
        let mine = req.request_type == RequestType::Vendor && req.recipient == Recipient::Device;
        let before = (ch.target.reboots.len(), ch.target.keys.len(), ch.target.peeks, ch.requests);
        if setup[0] & 0x80 == 0 {
            let d = ops.take(usize::from(len)).to_vec();
            let r = ch.control_out(req, &d);
            assert_eq!(r.is_some(), mine, "OUT {req:?} claimed: {r:?}");
            let rebooted = ch.target.reboots.len() - before.0;
            assert_eq!(rebooted, usize::from(mine && req.request == dc::REQ_REBOOT));
            let keys = ch.target.keys.len() - before.1;
            if mine && req.request == dc::REQ_KEYS {
                assert_eq!(keys, (d.len() / 2).min(16));
                let got = &ch.target.keys[before.1..];
                for (e, p) in got.iter().zip(d.chunks_exact(2)) {
                    assert_eq!(*e, (p[0], p[1] != 0));
                }
            } else {
                assert_eq!(keys, 0);
            }
            if mine && req.request == dc::REQ_TIME && d.len() != 7 {
                assert_eq!(r, Some(OutResponse::Rejected));
            }
        } else {
            let mut buf = vec![0u8; usize::from(len) * 4];
            let cap = buf.len();
            let r = ch.control_in(req, &mut buf);
            assert_eq!(r.is_some(), mine, "IN {req:?} claimed");
            if let Some(InResponse::Accepted(d)) = r {
                assert!(d.len() <= usize::from(req.length) && d.len() <= cap, "reply {} bytes", d.len());
                if req.request == dc::REQ_INFO && d.len() == Info::SIZE {
                    assert!(Info::from_bytes(d).is_some_and(|i| i.build_id == [7; 16]));
                }
            }
            let peeked = ch.target.peeks - before.2;
            if peeked > 0 {
                assert!(ch.target.development && req.request == dc::REQ_PEEK);
            }
            assert_eq!(ch.target.reboots.len(), before.0);
        }
        assert_eq!(ch.requests - before.3, u32::from(mine));
    }
}
