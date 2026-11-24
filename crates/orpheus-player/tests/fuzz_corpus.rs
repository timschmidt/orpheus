//! The fuzz targets' committed corpora and regression inputs, replayed on
//! stable through the same bodies the cargo-fuzz targets run (VER-4); and
//! the seed inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-player
//! --test fuzz_corpus seeds` rewrites `tests/fuzz-corpus/<target>/seed-*`):
//! queue files and `player.bin` images the crate's own encoders write.

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;

use orpheus_fs::Locator;
use orpheus_player::image::{IMAGE_LEN, StoreImage, encode_queue, queue_file_len, queue_id};
use orpheus_player::{Actions, Command, Config, Event, Player, Repeat, TrackRef};

#[test]
fn player_queue_corpus() {
    replay::replay("player_queue", bodies::player_queue);
}

#[test]
fn player_resume_corpus() {
    replay::replay("player_resume", bodies::player_resume);
}

fn queue_seeds() -> Vec<(String, Vec<u8>)> {
    let mut v = Vec::new();
    for (sel, n) in [(0u8, 0usize), (0, 1), (0, 12), (3, 200), (1, 3), (2, 2)] {
        let locs: Vec<Locator> = (0..n as u32).map(|i| Locator { dir: 5 + i / 50, index: i * 3 }).collect();
        let mut f = vec![0u8; queue_file_len(n)];
        encode_queue(0x1234_5678, &locs, &mut f).unwrap();
        let mut d = vec![sel];
        d.extend_from_slice(&f);
        v.push((format!("queue-{n}-{sel}"), d));
    }
    v
}

fn t(k: u32) -> TrackRef {
    TrackRef { serial: 0x1234_5678, dir: 5, index: k * 3, name_hash: k }
}

/// `player.bin` after a session: a queue played, shuffled, bookmarked.
fn image(shuffle: bool, bookmarks: u32) -> Vec<u8> {
    let mut img = StoreImage::new();
    let cfg = Config::default();
    let mut p = Player::new(cfg, &mut img.load(&[]));
    let mut out = Actions::default();
    let locs: Vec<Locator> = (0..12).map(|i| Locator { dir: 5, index: i * 3 }).collect();
    let id = queue_id(0x1234_5678, &locs);
    let mut ev = |p: &mut Player, img: &mut StoreImage, e: Event| {
        out.clear();
        p.handle(e, img, &mut out);
    };
    ev(&mut p, &mut img, Event::Command(Command::PlayQueue { id, len: 12, start: 2 }));
    if shuffle {
        ev(&mut p, &mut img, Event::Command(Command::Shuffle(true)));
        ev(&mut p, &mut img, Event::Command(Command::Repeat(Repeat::All)));
    }
    for k in 0..bookmarks {
        ev(&mut p, &mut img, Event::Opened { track: t(k), rate: 44_100, frames: Some(10_000_000) });
        ev(&mut p, &mut img, Event::Position { frame: 441_000 * u64::from(k + 1) });
        ev(&mut p, &mut img, Event::Command(Command::AddBookmark));
        ev(&mut p, &mut img, Event::Command(Command::Next));
    }
    ev(&mut p, &mut img, Event::Opened { track: t(7), rate: 48_000, frames: None });
    ev(&mut p, &mut img, Event::Tick { ms: 70_000 });
    ev(&mut p, &mut img, Event::Shutdown);
    let mut bin = [0u8; IMAGE_LEN];
    img.image(p.bookmarks(), &mut bin);
    bin.to_vec()
}

fn resume_seeds() -> Vec<(String, Vec<u8>)> {
    let with = |ops: &[u8], img: &[u8]| {
        let mut d = vec![ops.len() as u8];
        d.extend_from_slice(ops);
        d.extend_from_slice(img);
        d
    };
    // Operations: resume as saved, then commands and events (see the
    // target's `event`/`command` encodings).
    let ops: &[u8] = &[
        0, 1, // resume as saved, playing
        3, 4, 0, 0, // Opened item 4, 44.1 kHz, unknown length
        5, 0x10, 0, 0, 0, 1, // Position
        0, 14, // AddBookmark
        1, 5, // Next
        6, 7, 9, // ReaderAtEnd, DecoderEnded, Drained
        2, 10, 1, // Shuffle on
        0, 7, 5, // JumpTo 5
        10, 40, // Tick
        1, 15, 0, // GotoBookmark 0
        0, 19, 1, 6, 2, // Requeue
        4, 8,  // OpenFailed, DecoderFailed
        11, // Shutdown
    ];
    let mut v = vec![("empty-ops".to_string(), with(ops, &[]))];
    for (name, img) in
        [("plain", image(false, 0)), ("shuffled", image(true, 2)), ("bookmarks", image(false, 18))]
    {
        v.push((name.to_string(), with(&[], &img)));
        v.push((format!("{name}-ops"), with(ops, &img)));
        v.push((format!("{name}-other-queue"), with(&[1, 0, 3, 3, 4, 0, 1], &img)));
    }
    // A cut-short write: the second slot half written.
    let mut torn = image(false, 1);
    torn[64 + 20..128].fill(0);
    v.push(("torn".to_string(), with(&[0, 0], &torn)));
    v
}

#[test]
fn seeds() {
    let q = queue_seeds();
    for (_, d) in &q {
        bodies::player_queue(d);
    }
    let r = resume_seeds();
    for (_, d) in &r {
        bodies::player_resume(d);
    }
    replay::write_seeds("player_queue", &q);
    replay::write_seeds("player_resume", &r);
}
