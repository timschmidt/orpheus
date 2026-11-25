//! USB-6 end to end on the host: image files on a FAT card (fragmented by
//! interleaved appends) are mapped with `orpheus-fs`, exported through the
//! Bulk-Only Transport as a CD-ROM or a disk, and read back by a scripted
//! initiator; the whole medium hashes like the file. A writable export of
//! a test image changes the file and nothing else on the card; writes past
//! it and to read-only exports are refused.

use std::collections::VecDeque;

use embassy_futures::block_on;
use embassy_usb_driver::{
    Direction, Endpoint, EndpointAddress, EndpointError, EndpointIn, EndpointInfo, EndpointOut, EndpointType,
};
use orpheus_fs::host::RamDisk;
use orpheus_fs::{CacheSlot, MountOptions, OpenMode, Volume};
use orpheus_usb::msc::BlockDevice;
use orpheus_usb::msc::bot::{Bot, CBW_SIG};
use orpheus_usb::msc::extent::{Extent, ExtentTable, ImageDevice, POOL_EXTENTS};
use orpheus_usb::msc::image::{self, Medium, Refusal, Request};

// ------------------------------------------------------------ the card

/// A FAT16 volume of `sectors` sectors, 1 KiB clusters (so interleaved
/// appends fragment quickly).
fn mkfs_fat16(sectors: u32) -> Vec<u8> {
    let mut img = vec![0u8; sectors as usize * 512];
    let (spc, reserved, root_entries, fat_sectors) = (2u8, 4u16, 512u16, 64u16);
    let b = &mut img[..512];
    b[0..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
    b[3..11].copy_from_slice(b"ORPHTEST");
    b[11..13].copy_from_slice(&512u16.to_le_bytes());
    b[13] = spc;
    b[14..16].copy_from_slice(&reserved.to_le_bytes());
    b[16] = 2;
    b[17..19].copy_from_slice(&root_entries.to_le_bytes());
    b[19..21].copy_from_slice(&u16::try_from(sectors).unwrap().to_le_bytes());
    b[21] = 0xF8;
    b[22..24].copy_from_slice(&fat_sectors.to_le_bytes());
    b[24..26].copy_from_slice(&32u16.to_le_bytes());
    b[26..28].copy_from_slice(&2u16.to_le_bytes());
    b[36] = 0x80;
    b[38] = 0x29;
    b[43..54].copy_from_slice(b"USB6 TEST  ");
    b[54..62].copy_from_slice(b"FAT16   ");
    b[510] = 0x55;
    b[511] = 0xAA;
    for f in 0..2usize {
        let at = (usize::from(reserved) + f * usize::from(fat_sectors)) * 512;
        img[at..at + 4].copy_from_slice(&[0xF8, 0xFF, 0xFF, 0xFF]);
    }
    img
}

fn pattern(n: usize, seed: u32) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

/// The card: `/Images/disk.img` and `/.orpheus/images/test.img` and
/// `/Images/live.iso`, each written in 1 KiB pieces in turn with a filler
/// file, so each has many runs.
struct Card {
    data: Vec<u8>,
    files: Vec<(&'static str, Vec<u8>)>,
}

fn card() -> Card {
    let mut cache = vec![CacheSlot::EMPTY; 16];
    let mut vol =
        block_on(Volume::mount(RamDisk::from_bytes(mkfs_fat16(16_000)), &mut cache, MountOptions::default()))
            .unwrap();
    for d in ["/Images", "/.orpheus", "/.orpheus/images", "/Other"] {
        block_on(vol.create_dir(d)).unwrap();
    }
    let files: Vec<(&'static str, Vec<u8>)> = vec![
        ("/Images/disk.img", pattern(150 * 1024 + 300, 1)),
        ("/.orpheus/images/test.img", pattern(64 * 1024, 2)),
        ("/Images/live.iso", pattern(90 * 1024 + 1000, 3)),
    ];
    let mut handles: Vec<_> =
        files.iter().map(|(p, _)| block_on(vol.open(p, OpenMode::CreateNew)).unwrap()).collect();
    let mut filler = block_on(vol.open("/Other/filler.bin", OpenMode::CreateNew)).unwrap();
    let mut at = 0;
    while files.iter().any(|(_, d)| at < d.len()) {
        for (i, (_, d)) in files.iter().enumerate() {
            if at < d.len() {
                let e = (at + 1024).min(d.len());
                block_on(vol.write(&mut handles[i], &d[at..e])).unwrap();
                block_on(vol.flush_file(&mut handles[i])).unwrap();
            }
        }
        block_on(vol.write(&mut filler, &[0xEE; 1024])).unwrap();
        block_on(vol.flush_file(&mut filler)).unwrap();
        at += 1024;
    }
    for h in handles {
        block_on(vol.close(h)).unwrap();
    }
    block_on(vol.close(filler)).unwrap();
    block_on(vol.flush()).unwrap();
    Card { data: vol.into_device().data, files }
}

/// Map `path` on the card into `storage`; the file's size and runs used.
fn map(card: &[u8], path: &str, storage: &mut [Extent]) -> Result<(u64, usize), Refusal> {
    let mut cache = vec![CacheSlot::EMPTY; 8];
    let opts = MountOptions { read_only: true, ..MountOptions::default() };
    let mut vol = block_on(Volume::mount(RamDisk::from_bytes(card.to_vec()), &mut cache, opts)).unwrap();
    let f = block_on(vol.open(path, OpenMode::Read)).map_err(|_| Refusal::NotFound)?;
    let mut t = ExtentTable::new(storage);
    block_on(image::map_file(&mut vol, &f, &mut t))?;
    Ok((f.size(), t.len()))
}

/// The card as the transport's block device.
struct Raw(Vec<u8>);
impl BlockDevice for Raw {
    type Error = ();
    async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
        let o = usize::try_from(lba).map_err(|_| ())? * 512;
        buf.copy_from_slice(self.0.get(o..o + buf.len()).ok_or(())?);
        Ok(())
    }
    async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), ()> {
        let o = usize::try_from(lba).map_err(|_| ())? * 512;
        self.0.get_mut(o..o + buf.len()).ok_or(())?.copy_from_slice(buf);
        Ok(())
    }
}

// --------------------------------------------------------- the computer

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

fn cbw(tag: u32, len: u32, dir_in: bool, cdb: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; 31];
    b[..4].copy_from_slice(&CBW_SIG.to_le_bytes());
    b[4..8].copy_from_slice(&tag.to_le_bytes());
    b[8..12].copy_from_slice(&len.to_le_bytes());
    b[12] = if dir_in { 0x80 } else { 0 };
    b[14] = u8::try_from(cdb.len()).unwrap();
    b[15..15 + cdb.len()].copy_from_slice(cdb);
    b
}

fn read10(lba: u32, n: u16) -> [u8; 10] {
    let l = lba.to_be_bytes();
    let c = n.to_be_bytes();
    [0x28, 0, l[0], l[1], l[2], l[3], 0, c[0], c[1], 0]
}

fn write10(lba: u32, n: u16) -> [u8; 10] {
    let mut c = read10(lba, n);
    c[0] = 0x2A;
    c
}

type Dev<'t> = Bot<'t, In, Out, ImageDevice<'t, Raw>>;

/// One command: queue it (and its data), serve it; the bytes the device
/// sent (data then CSW) and the CSW status.
fn run(bot: &mut Dev<'_>, len: u32, dir_in: bool, cdb: &[u8], data: &[u8]) -> (Vec<u8>, u8) {
    bot.ep_out.queue.push_back(cbw(7, len, dir_in, cdb));
    for p in data.chunks(512) {
        bot.ep_out.queue.push_back(p.to_vec());
    }
    let from = bot.ep_in.sent.len();
    block_on(bot.serve_one()).unwrap();
    // Data of a refused write is not taken; the computer's recovery
    // discards it.
    bot.ep_out.queue.clear();
    let all: Vec<u8> = bot.ep_in.sent[from..].concat();
    let (body, csw) = all.split_at(all.len() - 13);
    (body.to_vec(), csw[12])
}

/// The computer's view: wait for the medium (unit attention), read the
/// capacity, then the whole medium in 32-block requests.
fn read_all(bot: &mut Dev<'_>) -> (u32, Vec<u8>) {
    let (_, s) = run(bot, 0, false, &[0, 0, 0, 0, 0, 0], &[]);
    if s != 0 {
        let (_, s) = run(bot, 0, false, &[0, 0, 0, 0, 0, 0], &[]);
        assert_eq!(s, 0, "ready after the unit attention");
    }
    let (cap, s) = run(bot, 8, true, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], &[]);
    assert_eq!(s, 0);
    let last = u32::from_be_bytes(cap[..4].try_into().unwrap());
    let bs = u32::from_be_bytes(cap[4..8].try_into().unwrap());
    let mut out = Vec::new();
    let mut lba = 0;
    while lba <= last {
        let n = (last + 1 - lba).min(32);
        let (d, s) = run(bot, n * bs, true, &read10(lba, u16::try_from(n).unwrap()), &[]);
        assert_eq!(s, 0, "read at {lba}");
        out.extend(d);
        lba += n;
    }
    (bs, out)
}

fn export<'t>(card: &[u8], storage: &'t mut [Extent], path: &str, req: Request, test_mode: bool) -> Dev<'t> {
    let (bytes, runs) = map(card, path, storage).unwrap();
    let e = image::check(req, image::under_orpheus(path), test_mode, bytes, bytes).unwrap();
    let table = ExtentTable::from_raw(storage, runs).unwrap();
    let mut dev = ImageDevice::new(Raw(card.to_vec()), table, bytes, e.medium.block_size()).unwrap();
    dev.set_writable(!e.read_only);
    let buf: &'static mut [u8] = Box::leak(vec![0u8; 16 * 1024].into_boxed_slice());
    let mut bot = Bot::new(
        In { info: info(Direction::In), sent: Vec::new() },
        Out { info: info(Direction::Out), queue: VecDeque::new() },
        dev,
        e.unit(),
        buf,
    );
    bot.scsi.medium_changed();
    bot
}

fn hash(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3))
}

#[test]
fn fragmented_images_read_back_whole() {
    let c = card();
    for (path, file) in &c.files {
        let mut st = vec![Extent::default(); POOL_EXTENTS];
        let (_, runs) = map(&c.data, path, &mut st).unwrap();
        assert!(runs > 20, "{path}: {runs} runs, the test wants a fragmented file");
        let medium = Medium::for_name(path);
        let mut bot = export(&c.data, &mut st, path, Request { medium, write: false }, false);
        let (bs, got) = read_all(&mut bot);
        assert_eq!(bs, medium.block_size());
        if medium.is_cd() {
            // Padded to whole 2048-byte blocks with zeros.
            assert_eq!(got.len(), file.len().next_multiple_of(2048), "{path}");
            assert_eq!(&got[..file.len()], &file[..]);
            assert!(got[file.len()..].iter().all(|&b| b == 0));
        } else {
            // A disk's partial last sector is not addressable.
            assert_eq!(got.len(), file.len() / 512 * 512, "{path}");
            assert_eq!(hash(&got), hash(&file[..got.len()]), "{path}");
        }
        assert_eq!(bot.stats.bytes_read, got.len() as u64);
    }
}

#[test]
fn read_only_exports_refuse_writes() {
    let c = card();
    for (path, medium) in [
        ("/Images/disk.img", Medium::RemovableDisk),
        ("/.orpheus/images/test.img", Medium::RemovableDisk),
        ("/Images/live.iso", Medium::Cd),
    ] {
        let mut st = vec![Extent::default(); POOL_EXTENTS];
        let mut bot = export(&c.data, &mut st, path, Request { medium, write: false }, true);
        read_all(&mut bot);
        let bs = medium.block_size() as usize;
        let (_, s) = run(&mut bot, bs as u32, false, &write10(0, 1), &vec![0x55; bs]);
        assert_eq!(s, 1, "{path}: write refused");
        let (sense, _) = run(&mut bot, 18, true, &[0x03, 0, 0, 0, 18, 0], &[]);
        let want = if medium.is_cd() { (0x05, 0x20) } else { (0x07, 0x27) };
        assert_eq!((sense[2], sense[12]), want, "{path}: sense");
        assert_eq!(bot.dev.card.0, c.data, "{path}: card unchanged");
    }
}

#[test]
fn writable_test_image_changes_only_itself() {
    let c = card();
    let path = "/.orpheus/images/test.img";
    let mut st = vec![Extent::default(); POOL_EXTENTS];
    let req = Request { medium: Medium::RemovableDisk, write: true };
    // Outside /.orpheus/, or outside test mode: refused before anything.
    let mut other = vec![Extent::default(); POOL_EXTENTS];
    let (bytes, _) = map(&c.data, "/Images/disk.img", &mut other).unwrap();
    assert_eq!(image::check(req, false, true, bytes, bytes), Err(Refusal::WriteOutsideOrpheus));
    assert_eq!(image::check(req, true, false, bytes, bytes), Err(Refusal::WriteNeedsTestMode));
    let mut bot = export(&c.data, &mut st, path, req, true);
    let (_, _) = read_all(&mut bot);
    let blocks = 64 * 2;
    // The computer writes the whole disk with a new pattern.
    let new = pattern(64 * 1024, 9);
    for (i, chunk) in new.chunks(16 * 512).enumerate() {
        let lba = u32::try_from(i * 16).unwrap();
        let (_, s) = run(&mut bot, 16 * 512, false, &write10(lba, 16), chunk);
        assert_eq!(s, 0);
    }
    // Past the end: refused with LBA OUT OF RANGE, nothing written.
    let before = bot.dev.card.0.clone();
    for (lba, n) in [(blocks, 1u16), (blocks - 1, 2), (u32::MAX, 1)] {
        let (_, s) =
            run(&mut bot, u32::from(n) * 512, false, &write10(lba, n), &vec![0xAB; usize::from(n) * 512]);
        assert_eq!(s, 1, "write at {lba}+{n}");
        let (sense, _) = run(&mut bot, 18, true, &[0x03, 0, 0, 0, 18, 0], &[]);
        assert_eq!((sense[2], sense[12]), (0x05, 0x21));
    }
    assert_eq!(bot.dev.card.0, before);
    assert_eq!(bot.stats.bytes_written, new.len() as u64);
    // Through the file system: the image has the new contents, every
    // other file and all metadata are as they were.
    let after = bot.dev.card.0.clone();
    let mut cache = vec![CacheSlot::EMPTY; 8];
    let opts = MountOptions { read_only: true, ..MountOptions::default() };
    let mut vol = block_on(Volume::mount(RamDisk::from_bytes(after.clone()), &mut cache, opts)).unwrap();
    for (p, old) in &c.files {
        let mut f = block_on(vol.open(p, OpenMode::Read)).unwrap();
        let mut b = vec![0u8; old.len()];
        assert_eq!(block_on(vol.read_exact_or_eof(&mut f, &mut b)).unwrap(), old.len());
        if *p == path {
            assert_eq!(b, new);
        } else {
            assert_eq!(&b, old, "{p} unchanged");
        }
    }
    // Byte for byte: only sectors of the image's runs differ.
    let st2: Vec<Extent> = st.clone();
    let mut owned = vec![false; after.len() / 512];
    for e in st2.iter().filter(|e| e.count > 0) {
        for s in e.lba..e.lba + e.count {
            owned[s as usize] = true;
        }
    }
    for (s, own) in owned.iter().enumerate() {
        if !own {
            assert_eq!(after[s * 512..(s + 1) * 512], c.data[s * 512..(s + 1) * 512], "sector {s}");
        }
    }
}

#[test]
fn too_fragmented_is_reported() {
    let c = card();
    let mut st = vec![Extent::default(); 8];
    assert_eq!(map(&c.data, "/Images/disk.img", &mut st), Err(Refusal::TooFragmented));
    let mut st = vec![Extent::default(); 8];
    assert_eq!(map(&c.data, "/Images/none.img", &mut st), Err(Refusal::NotFound));
}
