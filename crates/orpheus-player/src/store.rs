//! Track identity, the resume record and bookmarks, and how they are
//! persisted (04 §2.2 "Resume record", §6.3 "File locators").
//!
//! Records are fixed-size byte images with a magic, a sequence number and
//! a CRC-32, written alternately to two slots: a write cut short by a power
//! loss leaves the other slot intact, and loading picks the newest valid
//! one. The firmware implements [`PlayerStore`] on the settings store or on
//! a file under `/.orpheus/`; the calls are synchronous (the firmware may
//! queue the write and perform it later).

use orpheus_fs::Locator;
use orpheus_proto::hash::crc32;

/// A file reference that opens without a path walk (04 §6.3): volume
/// serial, parent directory first cluster, entry index, name hash. 16 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrackRef {
    pub serial: u32,
    pub dir: u32,
    pub index: u32,
    /// Hash of the file name, checked after `open_locator` (the entry may
    /// have been replaced since).
    pub name_hash: u32,
}

impl TrackRef {
    #[must_use]
    pub fn new(serial: u32, loc: Locator, name: &str) -> Self {
        Self { serial, dir: loc.dir, index: loc.index, name_hash: name_hash(name) }
    }

    #[must_use]
    pub fn locator(&self) -> Locator {
        Locator { dir: self.dir, index: self.index }
    }

    /// Whether `name` (of the entry `open_locator` found) is this track.
    #[must_use]
    pub fn matches(&self, name: &str) -> bool {
        name_hash(name) == self.name_hash
    }

    fn put(&self, b: &mut [u8]) {
        for (i, v) in [self.serial, self.dir, self.index, self.name_hash].into_iter().enumerate() {
            b[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
    }

    fn get(b: &[u8]) -> Self {
        let w = |i: usize| u32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
        Self { serial: w(0), dir: w(1), index: w(2), name_hash: w(3) }
    }
}

/// FNV-1a over the name's bytes (case as stored).
#[must_use]
pub fn name_hash(name: &str) -> u32 {
    name.bytes().fold(0x811C_9DC5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    /// Repeat the current track.
    One,
    /// Repeat the queue.
    All,
}

/// Everything needed to come back to where playback was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResumeRecord {
    /// The track (`None` before its open succeeded).
    pub track: Option<TrackRef>,
    /// Sample frame within the track (playable time line).
    pub frame: u64,
    /// Identity and length of the queue (chosen by the firmware, e.g. a
    /// hash of the queue file); resume uses `pos` only if they match.
    pub queue_id: u32,
    pub queue_len: u32,
    /// Play position and the queue item it maps to.
    pub pos: u32,
    pub item: u32,
    pub shuffle: bool,
    pub seed: u32,
    pub repeat: Repeat,
    /// The player was playing (not paused or stopped) when saved.
    pub playing: bool,
}

pub const RECORD_LEN: usize = 64;
const MAGIC: [u8; 4] = *b"YPL1";

impl ResumeRecord {
    /// The byte image, with sequence number `seq`.
    #[must_use]
    pub fn encode(&self, seq: u32) -> [u8; RECORD_LEN] {
        let mut b = [0u8; RECORD_LEN];
        b[0..4].copy_from_slice(&MAGIC);
        b[4..8].copy_from_slice(&seq.to_le_bytes());
        if let Some(t) = self.track {
            b[8] = 1;
            t.put(&mut b[12..28]);
        }
        b[9] = u8::from(self.shuffle);
        b[10] = self.repeat as u8;
        b[11] = u8::from(self.playing);
        b[28..36].copy_from_slice(&self.frame.to_le_bytes());
        for (i, v) in [self.queue_id, self.queue_len, self.pos, self.item, self.seed].into_iter().enumerate()
        {
            b[36 + 4 * i..40 + 4 * i].copy_from_slice(&v.to_le_bytes());
        }
        let crc = crc32(&b[..RECORD_LEN - 4]);
        b[RECORD_LEN - 4..].copy_from_slice(&crc.to_le_bytes());
        b
    }

    /// Decode a byte image: `(record, seq)` if it is valid.
    #[must_use]
    pub fn decode(b: &[u8; RECORD_LEN]) -> Option<(Self, u32)> {
        let crc = u32::from_le_bytes([b[60], b[61], b[62], b[63]]);
        if b[..4] != MAGIC || crc32(&b[..RECORD_LEN - 4]) != crc {
            return None;
        }
        let w = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let r = Self {
            track: (b[8] == 1).then(|| TrackRef::get(&b[12..28])),
            frame: u64::from_le_bytes([b[28], b[29], b[30], b[31], b[32], b[33], b[34], b[35]]),
            queue_id: w(36),
            queue_len: w(40),
            pos: w(44),
            item: w(48),
            seed: w(52),
            shuffle: b[9] == 1,
            repeat: match b[10] {
                1 => Repeat::One,
                2 => Repeat::All,
                _ => Repeat::Off,
            },
            playing: b[11] == 1,
        };
        Some((r, w(4)))
    }
}

/// A saved position in a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bookmark {
    pub track: TrackRef,
    pub frame: u64,
    /// Sample rate of the track (to show the time without opening it).
    pub rate: u32,
}

/// Bookmarks kept (oldest dropped first).
pub const MAX_BOOKMARKS: usize = 16;
pub const BOOKMARKS_LEN: usize = 8 + MAX_BOOKMARKS * BM_ENTRY + 4;
const BM_ENTRY: usize = 28;
const BM_MAGIC: [u8; 4] = *b"YBM2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bookmarks {
    items: [Bookmark; MAX_BOOKMARKS],
    n: u8,
}

impl Bookmarks {
    #[must_use]
    pub fn as_slice(&self) -> &[Bookmark] {
        &self.items[..usize::from(self.n)]
    }

    pub fn push(&mut self, b: Bookmark) {
        if usize::from(self.n) == MAX_BOOKMARKS {
            self.items.copy_within(1.., 0);
            self.n -= 1;
        }
        self.items[usize::from(self.n)] = b;
        self.n += 1;
    }

    pub fn remove(&mut self, i: usize) -> bool {
        let n = usize::from(self.n);
        if i >= n {
            return false;
        }
        self.items.copy_within(i + 1..n, i);
        self.items[n - 1] = Bookmark::default();
        self.n -= 1;
        true
    }

    #[must_use]
    pub fn encode(&self) -> [u8; BOOKMARKS_LEN] {
        let mut b = [0u8; BOOKMARKS_LEN];
        Self::encode_into(self.as_slice(), &mut b);
        b
    }

    /// The byte image of `items` (at most [`MAX_BOOKMARKS`], the oldest
    /// first) written into `b`.
    pub fn encode_into(items: &[Bookmark], b: &mut [u8; BOOKMARKS_LEN]) {
        let items = &items[items.len().saturating_sub(MAX_BOOKMARKS)..];
        b.fill(0);
        b[..4].copy_from_slice(&BM_MAGIC);
        b[4] = items.len() as u8;
        for (i, m) in items.iter().enumerate() {
            let o = 8 + BM_ENTRY * i;
            m.track.put(&mut b[o..o + 16]);
            b[o + 16..o + 24].copy_from_slice(&m.frame.to_le_bytes());
            b[o + 24..o + 28].copy_from_slice(&m.rate.to_le_bytes());
        }
        let crc = crc32(&b[..BOOKMARKS_LEN - 4]);
        b[BOOKMARKS_LEN - 4..].copy_from_slice(&crc.to_le_bytes());
    }

    #[must_use]
    pub fn decode(b: &[u8; BOOKMARKS_LEN]) -> Option<Self> {
        let crc = u32::from_le_bytes([
            b[BOOKMARKS_LEN - 4],
            b[BOOKMARKS_LEN - 3],
            b[BOOKMARKS_LEN - 2],
            b[BOOKMARKS_LEN - 1],
        ]);
        if b[..4] != BM_MAGIC || crc32(&b[..BOOKMARKS_LEN - 4]) != crc || usize::from(b[4]) > MAX_BOOKMARKS {
            return None;
        }
        let mut m = Self { n: b[4], ..Self::default() };
        for i in 0..usize::from(m.n) {
            let o = 8 + BM_ENTRY * i;
            let mut f = [0u8; 8];
            f.copy_from_slice(&b[o + 16..o + 24]);
            let rate = u32::from_le_bytes([b[o + 24], b[o + 25], b[o + 26], b[o + 27]]);
            m.items[i] = Bookmark { track: TrackRef::get(&b[o..o + 16]), frame: u64::from_le_bytes(f), rate };
        }
        Some(m)
    }
}

/// What the player persists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// The two alternating resume slots.
    ResumeA,
    ResumeB,
    Bookmarks,
}

/// Persistence for the player. `write` stores `data` under `key` (the
/// whole value; a power loss may cut it short — the CRC catches that);
/// `read` copies the stored value into `buf` and returns its length (0 if
/// none).
pub trait PlayerStore {
    fn write(&mut self, key: Key, data: &[u8]);
    fn read(&mut self, key: Key, buf: &mut [u8]) -> usize;
}

/// The newest valid resume record in `store`, with its sequence number.
pub fn load_resume(store: &mut dyn PlayerStore) -> Option<(ResumeRecord, u32)> {
    let mut best: Option<(ResumeRecord, u32)> = None;
    for key in [Key::ResumeA, Key::ResumeB] {
        let mut b = [0u8; RECORD_LEN];
        if store.read(key, &mut b) != RECORD_LEN {
            continue;
        }
        if let Some((r, seq)) = ResumeRecord::decode(&b)
            && best.is_none_or(|(_, s)| seq.wrapping_sub(s) as i32 > 0)
        {
            best = Some((r, seq));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_and_reject_damage() {
        let r = ResumeRecord {
            track: Some(TrackRef { serial: 1, dir: 2, index: 3, name_hash: 4 }),
            frame: 123_456_789,
            queue_id: 7,
            queue_len: 99,
            pos: 5,
            item: 42,
            shuffle: true,
            seed: 0xABCD,
            repeat: Repeat::All,
            playing: true,
        };
        let b = r.encode(9);
        assert_eq!(ResumeRecord::decode(&b), Some((r, 9)));
        for i in 0..RECORD_LEN {
            let mut d = b;
            d[i] ^= 0x10;
            assert_eq!(ResumeRecord::decode(&d), None, "byte {i}");
        }
        let mut m = Bookmarks::default();
        for f in 0..20u64 {
            m.push(Bookmark { track: TrackRef::default(), frame: f, rate: 44_100 });
        }
        assert_eq!(m.as_slice().len(), MAX_BOOKMARKS);
        assert_eq!(m.as_slice()[0].frame, 4);
        assert!(m.remove(0));
        assert_eq!(Bookmarks::decode(&m.encode()), Some(m));
    }
}
