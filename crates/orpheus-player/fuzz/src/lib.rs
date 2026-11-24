//! Fuzz-target bodies of `orpheus-player` (VER-4), shared by the cargo-fuzz
//! targets in `fuzz_targets/` (nightly) and the stable replay of the
//! committed corpus (`tests/fuzz_corpus.rs`, every `cargo test`).
//!
//! * [`player_queue`] — `/.orpheus/queue.bin` as found on the card. Oracles:
//!   no panic; a file decodes only into a queue that fits, with its
//!   identity matching its locators, and re-encodes to the same bytes;
//!   locator lists encode and decode back, and no cut-short encoding
//!   decodes.
//! * [`player_resume`] — `/.orpheus/player.bin` (resume slots, bookmarks)
//!   as found on the card, then the player driven by arbitrary commands and
//!   decoder/reader events. Oracles: no panic; every record that decodes
//!   re-encodes to one that decodes the same; every queue item the player
//!   asks to open or read ahead exists; the current position is inside the
//!   queue and the shuffle order is a permutation of it; at most 16
//!   bookmarks; what the player saves survives a write of `player.bin` and
//!   a restart (same newest resume record, same bookmarks).

#![forbid(unsafe_code)]

use orpheus_fs::Locator;
use orpheus_player::image::{IMAGE_LEN, StoreImage, decode_queue, encode_queue, queue_file_len, queue_id};
use orpheus_player::store::{BOOKMARKS_LEN, RECORD_LEN};
use orpheus_player::{
    Action, Actions, Bookmarks, Command, Config, Event, Player, Repeat, ResumeRecord, TrackRef, load_resume,
};

/// The firmware's queue capacity and other sizes the decoder is given.
const CAPS: [usize; 4] = [256, 0, 1, 4096];

/// See the crate docs. Byte 0 picks the capacity; the rest is the file.
pub fn player_queue(data: &[u8]) {
    let Some((&sel, file)) = data.split_first() else { return };
    let cap = CAPS[usize::from(sel & 3)];
    let mut out = vec![Locator { dir: 0, index: 0 }; cap];
    if let Some(h) = decode_queue(file, &mut out) {
        let n = h.len as usize;
        assert!(n <= cap, "{n} items decoded into {cap}");
        assert_ne!(h.id, 0);
        assert_eq!(h.id, queue_id(h.serial, &out[..n]));
        let mut enc = vec![0u8; queue_file_len(n)];
        assert_eq!(encode_queue(h.serial, &out[..n], &mut enc), Some(enc.len()));
        assert_eq!(&enc[..], &file[..enc.len()], "re-encoded queue file");
    }
    // The other way: the input's bytes as locators.
    let locs: Vec<Locator> = file
        .chunks_exact(8)
        .take(300)
        .map(|c| Locator {
            dir: u32::from_le_bytes([c[0], c[1], c[2], c[3]]),
            index: u32::from_le_bytes([c[4], c[5], c[6], c[7]]),
        })
        .collect();
    let serial = u32::from(sel) * 0x0101_0101;
    let len = queue_file_len(locs.len());
    let mut enc = vec![0u8; len];
    assert_eq!(encode_queue(serial, &locs, &mut enc[..len - 1]), None, "encoded into a short buffer");
    assert_eq!(encode_queue(serial, &locs, &mut enc), Some(len));
    let mut back = vec![Locator { dir: 0, index: 0 }; locs.len().max(1)];
    let h = decode_queue(&enc, &mut back).expect("own encoding decodes");
    assert_eq!((h.len as usize, h.serial, h.id), (locs.len(), serial, queue_id(serial, &locs)));
    assert_eq!(&back[..locs.len()], &locs[..]);
    for cut in [0, 3, 4, 15, 16, len / 2, len - 1] {
        assert!(
            decode_queue(&enc[..cut.min(len)], &mut back).is_none() || cut >= len,
            "cut at {cut} decodes"
        );
    }
    if !locs.is_empty() {
        assert!(decode_queue(&enc, &mut back[..locs.len() - 1]).is_none(), "decoded into a short queue");
    }
}

/// Byte reader over the fuzz input.
struct Ops<'a>(&'a [u8]);

impl Ops<'_> {
    fn byte(&mut self) -> Option<u8> {
        let (&b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(b)
    }
    fn b(&mut self) -> u8 {
        self.byte().unwrap_or(0)
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes([self.b(), self.b(), self.b(), self.b()])
    }
    /// A small number most of the time, any number sometimes.
    fn n(&mut self) -> u32 {
        match self.b() {
            x @ 0..=0xEF => u32::from(x % 24),
            0xF0..=0xF7 => self.u32(),
            x => u32::MAX - u32::from(x & 7),
        }
    }
}

fn track(k: u32) -> TrackRef {
    TrackRef { serial: 0xCAFE, dir: 2 + k / 100, index: k % 100, name_hash: k.wrapping_mul(0x9E37_79B9) }
}

fn command(o: &mut Ops<'_>) -> Command {
    match o.b() % 21 {
        0 => Command::PlayQueue { id: o.n().max(1), len: o.n(), start: o.n() },
        1 => Command::Play,
        2 => Command::Pause,
        3 => Command::Toggle,
        4 => Command::Stop,
        5 => Command::Next,
        6 => Command::Prev,
        7 => Command::JumpTo(o.n()),
        8 => Command::SeekMs(o.u32()),
        9 => Command::SeekByMs(o.u32() as i32),
        10 => Command::Shuffle(o.b() & 1 != 0),
        11 => Command::Repeat([Repeat::Off, Repeat::One, Repeat::All][usize::from(o.b() % 3)]),
        12 => Command::AbPoint,
        13 => Command::AbClear,
        14 => Command::AddBookmark,
        15 => Command::GotoBookmark(o.b() % 20),
        16 => Command::DeleteBookmark(o.b() % 20),
        17 => Command::SleepAfterMs(o.u32() % 100_000),
        18 => Command::SleepAfterTrack,
        // As the firmware's queue edit sends it: the current track keeps
        // its position, which is inside the new queue (`queue::edit` only
        // removes tracks after it, so the queue is never empty).
        _ => {
            let (id, len) = (o.n().max(1), o.n().max(1));
            Command::Requeue { id, len, pos: o.n() % len }
        }
    }
}

fn event(o: &mut Ops<'_>) -> Event {
    match o.b() % 12 {
        0..=2 => Event::Command(command(o)),
        3 => {
            let k = o.n();
            let rate = [44_100, 48_000, 8_000, 0, u32::MAX][usize::from(o.b() % 5)];
            let frames = match o.b() % 3 {
                0 => None,
                1 => Some(u64::from(o.u32())),
                _ => Some(u64::MAX - u64::from(o.b())),
            };
            Event::Opened { track: track(k), rate, frames }
        }
        4 => Event::OpenFailed,
        5 => Event::Position { frame: u64::from(o.u32()) * u64::from(o.b() % 4) },
        6 => Event::ReaderAtEnd,
        7 => Event::DecoderEnded,
        8 => Event::DecoderFailed,
        9 => Event::Drained,
        10 => Event::Tick { ms: u32::from(o.b()) * 250 + 1 },
        _ => Event::Shutdown,
    }
}

fn check(p: &Player, out: &Actions) {
    let len = p.len();
    for a in out.iter() {
        match a {
            Action::Open { item, .. } | Action::Prefetch { item } => {
                assert!(item < len, "{a:?} in a queue of {len}");
            }
            _ => {}
        }
    }
    if let Some(c) = p.current()
        && len > 0
        && c.item != u32::MAX
    {
        assert!(c.pos < len && c.item < len, "current {c:?} in a queue of {len}");
    }
    if let Some(s) = p.order() {
        assert_eq!(s.len(), len, "shuffle order of another length");
        if len <= 64 {
            let mut seen = vec![false; len as usize];
            for pos in 0..len {
                let i = s.item(pos);
                assert!(i < len && !seen[i as usize], "shuffle order is not a permutation");
                seen[i as usize] = true;
                assert_eq!(s.pos(i), pos);
            }
        }
    }
    assert!(p.bookmarks().len() <= 16);
}

/// See the crate docs. Input: `[n][n operation bytes][player.bin…]`.
pub fn player_resume(data: &[u8]) {
    let Some((&n, rest)) = data.split_first() else { return };
    let (ops, file) = rest.split_at(usize::from(n).min(rest.len()));
    let file = &file[..file.len().min(IMAGE_LEN)];

    // Every value in the file that decodes re-encodes to the same value.
    for k in 0..2 {
        if let Some(b) = file.get(k * RECORD_LEN..(k + 1) * RECORD_LEN) {
            let b: &[u8; RECORD_LEN] = b.try_into().unwrap();
            if let Some((r, seq)) = ResumeRecord::decode(b) {
                assert_eq!(ResumeRecord::decode(&r.encode(seq)), Some((r, seq)));
            }
        }
    }
    if let Some(b) = file.get(2 * RECORD_LEN..IMAGE_LEN) {
        let b: &[u8; BOOKMARKS_LEN] = b.try_into().unwrap();
        if let Some(m) = Bookmarks::decode(b) {
            assert!(m.as_slice().len() <= 16);
            assert_eq!(Bookmarks::decode(&m.encode()), Some(m));
        }
    }

    let cfg = Config { restart_ms: 3000, save_every_ms: 2000, max_failures: 4 };
    let mut img = StoreImage::new();
    let mut p = Player::new(cfg, &mut img.load(file));
    let mut o = Ops(ops);
    let mut out = Actions::default();
    if let Some(r) = p.saved() {
        // Resume into the queue the record names, or another one.
        let (id, len) = if o.b() & 1 == 0 { (r.queue_id, r.queue_len) } else { (o.n(), o.n()) };
        p.resume(id, len, o.b() & 1 != 0, &mut out);
        check(&p, &out);
    }
    let mut steps = 0;
    while !o.0.is_empty() && steps < 300 {
        steps += 1;
        out.clear();
        let ev = event(&mut o);
        p.handle(ev, &mut img, &mut out);
        check(&p, &out);
    }

    // Write player.bin and start again: the same newest record and the same
    // bookmarks come back.
    out.clear();
    p.handle(Event::Shutdown, &mut img, &mut out);
    let mut bin = [0u8; IMAGE_LEN];
    img.image(p.bookmarks(), &mut bin);
    let before = load_resume(&mut img);
    let mut img2 = StoreImage::new();
    let p2 = Player::new(cfg, &mut img2.load(&bin));
    assert_eq!(p2.bookmarks(), p.bookmarks(), "bookmarks after a restart");
    assert_eq!(load_resume(&mut img2), before, "resume record after a restart");
    assert_eq!(p2.saved(), before.map(|(r, _)| r));
}
