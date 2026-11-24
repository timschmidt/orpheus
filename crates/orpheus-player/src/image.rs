//! The firmware's persistent images (04 §2.2, §6.5): the player store as
//! one fixed-size byte image (`/.orpheus/player.bin`) and the play queue file
//! (`/.orpheus/queue.bin`).
//!
//! `player.bin` (588 bytes) is the three [`PlayerStore`] values at fixed
//! offsets, each carrying its own magic, sequence number (resume slots) and
//! CRC-32:
//!
//! | Offset | Bytes | Value |
//! |---|---|---|
//! | 0 | 64 | resume slot A ([`ResumeRecord`], magic `YPL1`) |
//! | 64 | 64 | resume slot B |
//! | 128 | 460 | bookmarks (magic `YBM2`: count, then per bookmark the
//! track locator, frame and sample rate) |
//!
//! The firmware keeps the resume slots in RAM ([`StoreImage`]; the
//! bookmarks live in the player) and writes only the changed span back in
//! place, so a write cut short by a power loss damages at most the slot
//! being written (the other slot still loads).
//!
//! `queue.bin` (little-endian):
//!
//! | Offset | Bytes | Value |
//! |---|---|---|
//! | 0 | 4 | magic `YQU1` |
//! | 4 | 4 | item count `n` |
//! | 8 | 4 | queue identity ([`queue_id`]: CRC-32 of serial and locators) |
//! | 12 | 4 | volume serial |
//! | 16 | 8·n | locators: parent directory first cluster, entry index |
//! | 16 + 8·n | 4 | CRC-32 of everything before |
//!
//! [`ResumeRecord`]: crate::ResumeRecord

use core::ops::Range;

use orpheus_fs::Locator;
use orpheus_proto::hash::{crc32, crc32_update};

use crate::store::{BOOKMARKS_LEN, Bookmark, Bookmarks, Key, PlayerStore, RECORD_LEN};

/// Size of `player.bin`.
pub const IMAGE_LEN: usize = 2 * RECORD_LEN + BOOKMARKS_LEN;

/// Where `key` lives in the image.
#[must_use]
pub fn key_range(key: Key) -> Range<usize> {
    match key {
        Key::ResumeA => 0..RECORD_LEN,
        Key::ResumeB => RECORD_LEN..2 * RECORD_LEN,
        Key::Bookmarks => 2 * RECORD_LEN..IMAGE_LEN,
    }
}

fn bit(key: Key) -> u8 {
    match key {
        Key::ResumeA => 1,
        Key::ResumeB => 2,
        Key::Bookmarks => 4,
    }
}

/// The player store in RAM: the two resume slots of `player.bin` and which
/// values changed since the last [`StoreImage::clear_dirty`]. The bookmarks
/// are not kept a second time (the player holds them): [`StoreImage::load`]
/// hands the file's bookmarks to `Player::new`, [`StoreImage::image`]
/// encodes the player's when the file is written.
pub struct StoreImage {
    resume: [u8; 2 * RECORD_LEN],
    dirty: u8,
}

impl StoreImage {
    #[must_use]
    pub const fn new() -> Self {
        Self { resume: [0; 2 * RECORD_LEN], dirty: 0 }
    }

    /// Take the resume slots from `file` (the bytes of `player.bin`; a
    /// shorter file leaves the rest empty, so the values there do not
    /// decode). Returns the store to create the player with: it also reads
    /// the file's bookmarks.
    pub fn load<'a>(&'a mut self, file: &'a [u8]) -> Loading<'a> {
        let n = file.len().min(2 * RECORD_LEN);
        self.resume = [0; 2 * RECORD_LEN];
        self.resume[..n].copy_from_slice(&file[..n]);
        self.dirty = 0;
        Loading { image: self, file }
    }

    /// The whole file: the resume slots and `bookmarks` (the player's).
    pub fn image(&self, bookmarks: &[Bookmark], out: &mut [u8; IMAGE_LEN]) {
        if let Some((head, bm)) = out.split_last_chunk_mut::<BOOKMARKS_LEN>() {
            head.copy_from_slice(&self.resume);
            Bookmarks::encode_into(bookmarks, bm);
        }
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty != 0
    }

    /// The smallest span of the file covering every changed value.
    #[must_use]
    pub fn dirty_range(&self) -> Option<Range<usize>> {
        let mut r: Option<Range<usize>> = None;
        for key in [Key::ResumeA, Key::ResumeB, Key::Bookmarks] {
            if self.dirty & bit(key) != 0 {
                let k = key_range(key);
                r = Some(match r {
                    Some(p) => p.start.min(k.start)..p.end.max(k.end),
                    None => k,
                });
            }
        }
        r
    }

    pub fn clear_dirty(&mut self) {
        self.dirty = 0;
    }

    /// Invalidate both resume slots ("stop and clear the resume point").
    pub fn forget_resume(&mut self) {
        self.resume = [0; 2 * RECORD_LEN];
        self.dirty |= bit(Key::ResumeA) | bit(Key::ResumeB);
    }
}

impl Default for StoreImage {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayerStore for StoreImage {
    fn write(&mut self, key: Key, data: &[u8]) {
        if key != Key::Bookmarks {
            let r = key_range(key);
            let n = data.len().min(r.len());
            self.resume[r.start..r.start + n].copy_from_slice(&data[..n]);
        }
        self.dirty |= bit(key);
    }

    fn read(&mut self, key: Key, buf: &mut [u8]) -> usize {
        let r = key_range(key);
        if key == Key::Bookmarks || buf.len() != r.len() {
            return 0;
        }
        buf.copy_from_slice(&self.resume[r]);
        buf.len()
    }
}

/// The store while the player is created (see [`StoreImage::load`]).
pub struct Loading<'a> {
    image: &'a mut StoreImage,
    file: &'a [u8],
}

impl PlayerStore for Loading<'_> {
    fn write(&mut self, key: Key, data: &[u8]) {
        self.image.write(key, data);
    }

    fn read(&mut self, key: Key, buf: &mut [u8]) -> usize {
        if key != Key::Bookmarks {
            return self.image.read(key, buf);
        }
        match self.file.get(key_range(key)) {
            Some(b) if b.len() == buf.len() => {
                buf.copy_from_slice(b);
                buf.len()
            }
            _ => 0,
        }
    }
}

// ------------------------------------------------------------ queue file

pub const QUEUE_MAGIC: [u8; 4] = *b"YQU1";
/// Header bytes before the locators.
pub const QUEUE_HEAD: usize = 16;

/// Bytes of a queue file with `n` items.
#[must_use]
pub const fn queue_file_len(n: usize) -> usize {
    QUEUE_HEAD + 8 * n + 4
}

/// What a queue file says about its queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueHeader {
    pub len: u32,
    pub id: u32,
    pub serial: u32,
}

fn loc_bytes(l: Locator) -> [u8; 8] {
    let mut b = [0u8; 8];
    b[..4].copy_from_slice(&l.dir.to_le_bytes());
    b[4..].copy_from_slice(&l.index.to_le_bytes());
    b
}

/// The queue's identity (what the resume record compares): CRC-32 of the
/// volume serial and the locators, never 0.
#[must_use]
pub fn queue_id(serial: u32, locs: &[Locator]) -> u32 {
    let mut c = crc32_update(0xFFFF_FFFF, &serial.to_le_bytes());
    for &l in locs {
        c = crc32_update(c, &loc_bytes(l));
    }
    (c ^ 0xFFFF_FFFF).max(1)
}

/// Write the queue file for `locs` into `out`; returns its length, or
/// `None` if `out` is too small.
#[must_use]
pub fn encode_queue(serial: u32, locs: &[Locator], out: &mut [u8]) -> Option<usize> {
    let len = queue_file_len(locs.len());
    let out = out.get_mut(..len)?;
    let n = u32::try_from(locs.len()).ok()?;
    out[..4].copy_from_slice(&QUEUE_MAGIC);
    out[4..8].copy_from_slice(&n.to_le_bytes());
    out[8..12].copy_from_slice(&queue_id(serial, locs).to_le_bytes());
    out[12..16].copy_from_slice(&serial.to_le_bytes());
    for (i, &l) in locs.iter().enumerate() {
        let o = QUEUE_HEAD + 8 * i;
        out[o..o + 8].copy_from_slice(&loc_bytes(l));
    }
    let crc = crc32(&out[..len - 4]);
    out[len - 4..].copy_from_slice(&crc.to_le_bytes());
    Some(len)
}

/// Read a queue file back into `out`. `None` if it is damaged, or holds
/// more items than `out` (the firmware's queue length).
#[must_use]
pub fn decode_queue(b: &[u8], out: &mut [Locator]) -> Option<QueueHeader> {
    let w = |o: usize| -> Option<u32> { Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?)) };
    if b.get(..4)? != QUEUE_MAGIC {
        return None;
    }
    let n = usize::try_from(w(4)?).ok()?;
    if n > out.len() {
        return None;
    }
    let len = queue_file_len(n);
    if b.len() < len || crc32(&b[..len - 4]) != w(len - 4)? {
        return None;
    }
    for (i, slot) in out[..n].iter_mut().enumerate() {
        let o = QUEUE_HEAD + 8 * i;
        *slot = Locator { dir: w(o)?, index: w(o + 4)? };
    }
    let h = QueueHeader { len: w(4)?, id: w(8)?, serial: w(12)? };
    // The stored identity must be the one these locators give.
    (h.id == queue_id(h.serial, &out[..n])).then_some(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{ResumeRecord, TrackRef, load_resume};
    use crate::{Actions, Command, Config, Event, Player};

    #[test]
    fn store_image_round_trips_through_bytes() {
        let mut img = StoreImage::new();
        assert!(load_resume(&mut img).is_none());
        let mut p = Player::new(Config::default(), &mut img.load(&[]));
        let mut out = Actions::default();
        p.handle(Event::Command(Command::PlayQueue { id: 5, len: 3, start: 1 }), &mut img, &mut out);
        let t = TrackRef { serial: 1, dir: 2, index: 3, name_hash: 4 };
        p.handle(Event::Opened { track: t, rate: 44_100, frames: Some(1_000_000) }, &mut img, &mut out);
        p.handle(Event::Position { frame: 441_000 }, &mut img, &mut out);
        p.handle(Event::Command(Command::AddBookmark), &mut img, &mut out);
        p.handle(Event::Command(Command::Pause), &mut img, &mut out);
        assert!(img.is_dirty());
        assert_eq!(img.dirty_range(), Some(0..IMAGE_LEN));
        // The file as written, read back after a reboot.
        let mut file = [0u8; IMAGE_LEN];
        img.image(p.bookmarks(), &mut file);
        let mut back = StoreImage::new();
        let p2 = Player::new(Config::default(), &mut back.load(&file));
        assert!(!back.is_dirty());
        let r = p2.saved().unwrap();
        assert_eq!(
            (r.track, r.frame, r.queue_id, r.queue_len, r.pos, r.playing),
            (Some(t), 441_000, 5, 3, 1, false)
        );
        assert_eq!(p2.bookmarks(), &[Bookmark { track: t, frame: 441_000, rate: 44_100 }]);
        // The image again, from the restarted player: the same bytes.
        let mut again = [0u8; IMAGE_LEN];
        back.image(p2.bookmarks(), &mut again);
        assert_eq!(again, file);
    }

    #[test]
    fn only_the_written_slot_is_dirty_and_a_torn_slot_keeps_the_other() {
        let mut img = StoreImage::new();
        let r = ResumeRecord { frame: 10, queue_len: 2, ..ResumeRecord::default() };
        img.write(Key::ResumeB, &r.encode(2));
        assert_eq!(img.dirty_range(), Some(key_range(Key::ResumeB)));
        img.clear_dirty();
        img.write(Key::Bookmarks, &Bookmarks::default().encode());
        assert_eq!(img.dirty_range(), Some(key_range(Key::Bookmarks)));
        // Slot A torn mid-write: half a newer record.
        let newer = ResumeRecord { frame: 99, ..r }.encode(3);
        let mut file = [0u8; IMAGE_LEN];
        img.image(&[], &mut file);
        file[..RECORD_LEN / 2].copy_from_slice(&newer[..RECORD_LEN / 2]);
        let mut back = StoreImage::new();
        let _ = back.load(&file);
        assert_eq!(load_resume(&mut back), Some((r, 2)));
        // Forgetting clears both slots.
        back.forget_resume();
        assert_eq!(load_resume(&mut back), None);
        assert_eq!(back.dirty_range(), Some(0..2 * RECORD_LEN));
        // A short (new) file loads as empty, bookmarks included.
        let p = Player::new(Config::default(), &mut back.load(&file[..10]));
        assert_eq!(load_resume(&mut back), None);
        assert!(p.bookmarks().is_empty());
    }

    #[test]
    fn queue_file_round_trip_and_damage() {
        let locs: [Locator; 5] =
            core::array::from_fn(|i| Locator { dir: 100 + i as u32, index: 3 * i as u32 });
        let mut buf = [0u8; 128];
        let n = encode_queue(0xCAFE, &locs, &mut buf).unwrap();
        assert_eq!(n, queue_file_len(5));
        let mut out = [Locator { dir: 0, index: 0 }; 8];
        let h = decode_queue(&buf[..n], &mut out).unwrap();
        assert_eq!(h, QueueHeader { len: 5, id: queue_id(0xCAFE, &locs), serial: 0xCAFE });
        assert_eq!(&out[..5], &locs);
        assert_ne!(queue_id(0xCAFE, &locs), queue_id(0xCAFF, &locs));
        assert_ne!(queue_id(0xCAFE, &locs), queue_id(0xCAFE, &locs[..4]));
        for i in 0..n {
            let mut d = buf;
            d[i] ^= 0x40;
            assert_eq!(decode_queue(&d[..n], &mut out), None, "byte {i}");
        }
        // Truncated, too long for the RAM queue, buffer too small.
        assert_eq!(decode_queue(&buf[..n - 1], &mut out), None);
        assert_eq!(decode_queue(&buf[..n], &mut out[..4]), None);
        assert_eq!(encode_queue(1, &locs, &mut buf[..n - 1]), None);
        // An empty queue is valid.
        let n0 = encode_queue(7, &[], &mut buf).unwrap();
        assert_eq!(decode_queue(&buf[..n0], &mut out).map(|h| h.len), Some(0));
    }
}
