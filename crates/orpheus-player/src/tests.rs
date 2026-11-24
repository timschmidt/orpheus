//! The player against a simulated firmware: tracks of known length, a
//! reader that runs ahead of the decoder, a PCM sink that records which
//! (track, frame) ranges were played; and a store that can lose power in
//! the middle of a write.

use std::collections::BTreeMap;
use std::vec::Vec;

use super::*;

const RATE: u32 = 44100;

/// A store in memory; `cut` truncates the next write to that many bytes
/// (a power loss during the write).
#[derive(Default, Clone)]
struct Mem {
    kv: BTreeMap<u8, Vec<u8>>,
    cut: Option<usize>,
    writes: u32,
}

impl PlayerStore for Mem {
    fn write(&mut self, key: Key, data: &[u8]) {
        self.writes += 1;
        let n = self.cut.take().unwrap_or(data.len()).min(data.len());
        let e = self.kv.entry(key as u8).or_insert_with(|| std::vec![0; data.len()]);
        e.resize(data.len(), 0);
        e[..n].copy_from_slice(&data[..n]);
    }
    fn read(&mut self, key: Key, buf: &mut [u8]) -> usize {
        match self.kv.get(&(key as u8)) {
            Some(v) if v.len() == buf.len() => {
                buf.copy_from_slice(v);
                v.len()
            }
            _ => 0,
        }
    }
}

fn track(item: u32) -> TrackRef {
    TrackRef { serial: 0xCAFE, dir: 100, index: item * 3, name_hash: item ^ 0x5555 }
}

/// The firmware side.
struct Sim {
    lens: Vec<u64>,
    broken: Vec<u32>,
    /// Item open, frames decoded, frames read by the reader.
    cur: Option<(u32, u64, u64)>,
    prefetched: Option<u32>,
    reader_end_sent: bool,
    paused: bool,
    stopped: bool,
    /// Played ranges: (item, start, end), merged when contiguous.
    played: Vec<(u32, u64, u64)>,
    /// (item, gapless) of every open.
    opens: Vec<(u32, bool)>,
    drained: bool,
}

impl Sim {
    fn new(lens: &[u64]) -> Self {
        Self {
            lens: lens.to_vec(),
            broken: Vec::new(),
            cur: None,
            prefetched: None,
            reader_end_sent: false,
            paused: false,
            stopped: true,
            played: Vec::new(),
            opens: Vec::new(),
            drained: false,
        }
    }

    fn run(&mut self, p: &mut Player, st: &mut Mem, ev: Event) {
        let mut q = std::collections::VecDeque::from([ev]);
        while let Some(ev) = q.pop_front() {
            let mut out = Actions::default();
            p.handle(ev, st, &mut out);
            for a in out.iter() {
                if let Some(e) = self.act(a) {
                    q.push_back(e);
                }
            }
        }
    }

    fn act(&mut self, a: Action) -> Option<Event> {
        match a {
            Action::Open { item, start_frame, gapless, paused } => {
                self.opens.push((item, gapless));
                if gapless {
                    assert_eq!(self.prefetched, Some(item), "gapless open of an item not prefetched");
                }
                self.prefetched = None;
                self.reader_end_sent = false;
                self.paused = paused;
                self.stopped = false;
                if self.broken.contains(&item) || item as usize >= self.lens.len() {
                    self.cur = None;
                    return Some(Event::OpenFailed);
                }
                self.cur = Some((item, start_frame, start_frame));
                Some(Event::Opened { track: track(item), rate: RATE, frames: Some(self.lens[item as usize]) })
            }
            Action::OpenTrack { track: t, start_frame, paused } => {
                let item = t.index / 3;
                self.opens.push((item, false));
                self.prefetched = None;
                self.reader_end_sent = false;
                self.paused = paused;
                self.stopped = false;
                self.cur = Some((item, start_frame, start_frame));
                Some(Event::Opened { track: t, rate: RATE, frames: Some(self.lens[item as usize]) })
            }
            Action::Prefetch { item } => {
                self.prefetched = Some(item);
                None
            }
            Action::CancelPrefetch => {
                self.prefetched = None;
                // As the firmware does: report the reader's end again.
                self.reader_end_sent = false;
                None
            }
            Action::Seek { frame } => {
                if let Some(c) = &mut self.cur {
                    c.1 = frame;
                    c.2 = frame;
                    self.reader_end_sent = false;
                }
                None
            }
            Action::Pause => {
                self.paused = true;
                None
            }
            Action::Resume => {
                self.paused = false;
                None
            }
            Action::Stop => {
                self.stopped = true;
                self.cur = None;
                None
            }
        }
    }

    /// Play `frames` frames (in 1000-frame steps), feeding events.
    fn play(&mut self, p: &mut Player, st: &mut Mem, frames: u64) {
        let mut left = frames;
        while left > 0 {
            if self.paused || self.stopped {
                return;
            }
            let Some((item, dec, read)) = self.cur else {
                if p.state() == State::Draining && !self.drained {
                    self.drained = true;
                    self.run(p, st, Event::Drained);
                }
                return;
            };
            let len = self.lens[item as usize];
            let n = 1000.min(len - dec).min(left);
            if n > 0 {
                match self.played.last_mut() {
                    Some(l) if l.0 == item && l.2 == dec => l.2 += n,
                    _ => self.played.push((item, dec, dec + n)),
                }
            }
            let dec = dec + n;
            let read = (dec + 20_000).min(len).max(read);
            self.cur = Some((item, dec, read));
            left -= n;
            self.run(p, st, Event::Position { frame: dec });
            self.run(p, st, Event::Tick { ms: (n * 1000 / u64::from(RATE)) as u32 });
            if read == len && !self.reader_end_sent && self.cur.is_some_and(|c| c.0 == item) {
                self.reader_end_sent = true;
                self.run(p, st, Event::ReaderAtEnd);
            }
            if dec == len && self.cur.is_some_and(|c| c.0 == item && c.1 == len) {
                self.cur = None;
                self.run(p, st, Event::DecoderEnded);
            }
        }
    }
}

fn setup(lens: &[u64]) -> (Player, Mem, Sim) {
    let mut st = Mem::default();
    let p = Player::new(Config::default(), &mut st);
    (p, st, Sim::new(lens))
}

fn cmd(c: Command) -> Event {
    Event::Command(c)
}

#[test]
fn queue_plays_through_gapless() {
    let lens = [50_000u64, 7_777, 123_456, 1, 30_000];
    let (mut p, mut st, mut sim) = setup(&lens);
    sim.run(&mut p, &mut st, cmd(Command::PlayQueue { id: 1, len: 5, start: 0 }));
    sim.play(&mut p, &mut st, 10_000_000);
    assert_eq!(p.state(), State::Stopped);
    let want: Vec<(u32, u64, u64)> = lens.iter().enumerate().map(|(i, &l)| (i as u32, 0, l)).collect();
    assert_eq!(sim.played, want);
    // Every hand-over after the first track was gapless.
    assert_eq!(sim.opens, std::vec![(0, false), (1, true), (2, true), (3, true), (4, true)]);
}

#[test]
fn shuffle_repeat_all_and_repeat_one() {
    let lens = [3000u64; 7];
    let (mut p, mut st, mut sim) = setup(&lens);
    sim.run(&mut p, &mut st, cmd(Command::Shuffle(true)));
    sim.run(&mut p, &mut st, cmd(Command::Repeat(Repeat::All)));
    sim.run(&mut p, &mut st, cmd(Command::PlayQueue { id: 1, len: 7, start: 4 }));
    sim.play(&mut p, &mut st, 3000 * 14);
    let items: Vec<u32> = sim.played.iter().map(|r| r.0).collect();
    assert_eq!(items[0], 4, "starts with the chosen item");
    let mut first: Vec<u32> = items[..7].to_vec();
    first.sort_unstable();
    assert_eq!(first, (0..7).collect::<Vec<_>>(), "one pass is a permutation");
    assert_eq!(items.len(), 14);
    assert_eq!(&items[7..], &items[..7], "repeat-all repeats the same order");
    let (mut p, mut st, mut sim) = setup(&lens);
    sim.run(&mut p, &mut st, cmd(Command::Repeat(Repeat::One)));
    sim.run(&mut p, &mut st, cmd(Command::PlayQueue { id: 1, len: 7, start: 2 }));
    sim.play(&mut p, &mut st, 3000 * 3);
    assert_eq!(sim.played.iter().map(|r| r.0).collect::<Vec<_>>(), std::vec![2, 2, 2]);
    assert!(sim.opens[1..].iter().all(|&(i, g)| i == 2 && g));
}

#[test]
fn next_prev_seek() {
    let lens = [441_000u64; 4];
    let (mut p, mut st, mut sim) = setup(&lens);
    sim.run(&mut p, &mut st, cmd(Command::PlayQueue { id: 9, len: 4, start: 1 }));
    sim.play(&mut p, &mut st, 44_100);
    sim.run(&mut p, &mut st, cmd(Command::Next));
    assert_eq!(p.current().unwrap().item, 2);
    sim.play(&mut p, &mut st, 1000);
    // Early in the track: Prev goes back a track.
    sim.run(&mut p, &mut st, cmd(Command::Prev));
    assert_eq!(p.current().unwrap().item, 1);
    sim.play(&mut p, &mut st, 5 * 44_100);
    // Later: Prev restarts the track.
    sim.run(&mut p, &mut st, cmd(Command::Prev));
    assert_eq!(p.current().unwrap().item, 1);
    assert_eq!(sim.cur.unwrap().1, 0);
    sim.run(&mut p, &mut st, cmd(Command::SeekMs(2500)));
    assert_eq!(sim.cur.unwrap().1, 110_250);
    sim.run(&mut p, &mut st, cmd(Command::SeekByMs(-1000)));
    assert_eq!(sim.cur.unwrap().1, 66_150);
    sim.run(&mut p, &mut st, cmd(Command::SeekMs(1_000_000)));
    assert_eq!(sim.cur.unwrap().1, 440_999);
    sim.run(&mut p, &mut st, cmd(Command::JumpTo(3)));
    sim.run(&mut p, &mut st, cmd(Command::Next));
    assert_eq!(p.state(), State::Stopped);
}

#[test]
fn broken_tracks_are_skipped_and_all_broken_stops() {
    let lens = [2000u64; 5];
    let (mut p, mut st, mut sim) = setup(&lens);
    sim.broken = std::vec![1, 2];
    sim.run(&mut p, &mut st, cmd(Command::PlayQueue { id: 1, len: 5, start: 0 }));
    sim.play(&mut p, &mut st, 100_000);
    assert_eq!(sim.played.iter().map(|r| r.0).collect::<Vec<_>>(), std::vec![0, 3, 4]);
    let (mut p, mut st, mut sim) = setup(&lens);
    sim.broken = std::vec![0, 1, 2, 3, 4];
    sim.run(&mut p, &mut st, cmd(Command::Repeat(Repeat::All)));
    sim.run(&mut p, &mut st, cmd(Command::PlayQueue { id: 1, len: 5, start: 0 }));
    assert_eq!(p.state(), State::Stopped);
    assert!(sim.opens.len() <= 5);
}

#[test]
fn resume_after_pause_and_power_loss_is_exact() {
    let lens = [1_000_000u64; 6];
    let (mut p, mut st, mut sim) = setup(&lens);
    sim.run(&mut p, &mut st, cmd(Command::Shuffle(true)));
    sim.run(&mut p, &mut st, cmd(Command::PlayQueue { id: 77, len: 6, start: 3 }));
    sim.play(&mut p, &mut st, 1_000_000 + 123_000);
    sim.run(&mut p, &mut st, cmd(Command::Pause));
    let at = p.current().unwrap();
    // Power loss: the player and the simulation vanish; the store stays.
    let _ = p;
    let mut p = Player::new(Config::default(), &mut st);
    let mut sim = Sim::new(&lens);
    let mut out = Actions::default();
    assert!(p.resume(77, 6, false, &mut out));
    assert_eq!(
        out.iter().next(),
        Some(Action::Open { item: at.item, start_frame: 123_000, gapless: false, paused: true })
    );
    assert!(p.shuffled());
    for a in out.iter() {
        if let Some(e) = sim.act(a) {
            sim.run(&mut p, &mut st, e);
        }
    }
    assert_eq!(p.state(), State::Paused);
    // The same shuffled order continues.
    sim.run(&mut p, &mut st, cmd(Command::Play));
    sim.play(&mut p, &mut st, 1_000_000);
    assert_eq!(sim.played[0], (at.item, 123_000, 1_000_000));
}

#[test]
fn power_loss_while_playing_loses_at_most_the_save_interval() {
    let lens = [100 * 44_100u64; 2];
    let (mut p, mut st, mut sim) = setup(&lens);
    sim.run(&mut p, &mut st, cmd(Command::PlayQueue { id: 5, len: 2, start: 0 }));
    sim.play(&mut p, &mut st, 75 * 44_100);
    let _ = p;
    let mut p = Player::new(Config::default(), &mut st);
    let r = p.saved().unwrap();
    assert!(r.playing && r.item == 0);
    assert!(75 * 44_100 - r.frame <= 60 * 44_100 && r.frame >= 60 * 44_100);
    let mut out = Actions::default();
    assert!(p.resume(5, 2, true, &mut out));
    // A queue that changed meanwhile: resume opens the track directly.
    let mut p = Player::new(Config::default(), &mut st);
    let mut out = Actions::default();
    assert!(p.resume(6, 2, true, &mut out));
    assert!(
        matches!(out.iter().next(), Some(Action::OpenTrack { track, .. }) if track == super::tests::track(0))
    );
}

#[test]
fn torn_write_keeps_the_previous_record() {
    let lens = [10_000_000u64; 3];
    let (mut p, mut st, mut sim) = setup(&lens);
    sim.run(&mut p, &mut st, cmd(Command::PlayQueue { id: 1, len: 3, start: 1 }));
    sim.play(&mut p, &mut st, 50_000);
    sim.run(&mut p, &mut st, cmd(Command::Pause));
    let good = p.current().unwrap().frame;
    sim.run(&mut p, &mut st, cmd(Command::Play));
    sim.play(&mut p, &mut st, 10_000);
    for cut in [0usize, 1, 20, 40, 63] {
        let mut s2 = st.clone();
        s2.cut = Some(cut);
        let mut p2 = Player::new(Config::default(), &mut s2);
        p2.handle(Event::Shutdown, &mut s2, &mut Actions::default());
        let r = Player::new(Config::default(), &mut s2).saved().unwrap();
        assert_eq!(r.frame, good, "cut at {cut}");
    }
    p.handle(Event::Shutdown, &mut st, &mut Actions::default());
    assert_eq!(Player::new(Config::default(), &mut st).saved().unwrap().frame, 60_000);
}

#[test]
fn bookmarks_ab_and_sleep() {
    let lens = [10 * 44_100u64; 3];
    let (mut p, mut st, mut sim) = setup(&lens);
    sim.run(&mut p, &mut st, cmd(Command::PlayQueue { id: 1, len: 3, start: 0 }));
    sim.play(&mut p, &mut st, 44_100);
    sim.run(&mut p, &mut st, cmd(Command::AddBookmark));
    sim.run(&mut p, &mut st, cmd(Command::Next));
    sim.play(&mut p, &mut st, 2000);
    sim.run(&mut p, &mut st, cmd(Command::GotoBookmark(0)));
    assert_eq!(sim.cur.unwrap().0, 0);
    assert_eq!(sim.cur.unwrap().1, 44_100);
    // Bookmarks survive a restart.
    assert_eq!(Player::new(Config::default(), &mut st).bookmarks().len(), 1);
    // A–B: from 1 s to 2 s.
    sim.run(&mut p, &mut st, cmd(Command::SeekMs(1000)));
    sim.run(&mut p, &mut st, Event::Position { frame: 44_100 });
    sim.run(&mut p, &mut st, cmd(Command::AbPoint));
    sim.play(&mut p, &mut st, 44_100);
    sim.run(&mut p, &mut st, cmd(Command::AbPoint));
    assert_eq!(p.ab(), (Some(44_100), Some(2 * 44_100)));
    sim.play(&mut p, &mut st, 10_000);
    assert!(sim.cur.unwrap().1 < 2 * 44_100 && sim.cur.unwrap().1 >= 44_100);
    sim.run(&mut p, &mut st, cmd(Command::AbClear));
    assert_eq!(p.ab(), (None, None));
    // Sleep timer pauses.
    sim.run(&mut p, &mut st, cmd(Command::SleepAfterMs(500)));
    assert_eq!(p.sleep_timer(), (Some(500), false));
    sim.play(&mut p, &mut st, 44_100);
    assert_eq!(p.state(), State::Paused);
    assert_eq!(p.sleep_timer(), (None, false));
    // A time cancels "after this track", 0 cancels everything.
    sim.run(&mut p, &mut st, cmd(Command::SleepAfterTrack));
    sim.run(&mut p, &mut st, cmd(Command::SleepAfterMs(0)));
    assert_eq!(p.sleep_timer(), (None, false));
    // Sleep after track: stops at the end of the current one.
    sim.run(&mut p, &mut st, cmd(Command::Play));
    sim.run(&mut p, &mut st, cmd(Command::SleepAfterTrack));
    assert_eq!(p.sleep_timer(), (None, true));
    sim.play(&mut p, &mut st, 20 * 44_100);
    assert_eq!(p.state(), State::Stopped);
    assert_eq!(sim.played.last().unwrap().0, 0);
}

#[test]
fn player_size() {
    std::eprintln!("Player {} B", core::mem::size_of::<Player>());
    assert!(core::mem::size_of::<Player>() <= 768);
}

#[test]
fn shuffled_queue_from_an_item_plays_every_item_once() {
    for len in [1u32, 2, 7, 33, 256] {
        for start in [0, len / 2, len - 1] {
            let lens = std::vec![1000u64; len as usize];
            let (mut p, mut st, mut sim) = setup(&lens);
            sim.run(&mut p, &mut st, cmd(Command::Shuffle(true)));
            sim.run(&mut p, &mut st, cmd(Command::PlayQueue { id: 1, len, start }));
            sim.play(&mut p, &mut st, 1000 * u64::from(len) + 5000);
            let mut items: Vec<u32> = sim.played.iter().map(|r| r.0).collect();
            assert_eq!(items[0], start, "len {len} start {start}");
            items.sort_unstable();
            assert_eq!(items, (0..len).collect::<Vec<_>>(), "len {len} start {start}");
            assert_eq!(p.state(), State::Stopped);
        }
    }
}

#[test]
fn requeue_keeps_playing_and_follows_the_new_order() {
    let lens = [5000u64, 6000, 7000, 8000];
    let (mut p, mut st, mut sim) = setup(&lens);
    sim.run(&mut p, &mut st, cmd(Command::PlayQueue { id: 1, len: 4, start: 0 }));
    sim.play(&mut p, &mut st, 4000);
    assert_eq!(sim.prefetched, Some(1));
    // Play next: a 999-frame track goes in after the current one.
    sim.lens = std::vec![5000, 999, 6000, 7000, 8000];
    sim.run(&mut p, &mut st, cmd(Command::Requeue { id: 2, len: 5, pos: 0 }));
    assert_eq!(sim.prefetched, None);
    assert_eq!((p.queue_id(), p.len(), p.current().unwrap().item), (2, 5, 0));
    sim.play(&mut p, &mut st, 1_000_000);
    let got: Vec<(u32, u64, u64)> = sim.played.clone();
    let want: Vec<(u32, u64, u64)> =
        std::vec![(0, 0, 5000), (1, 0, 999), (2, 0, 6000), (3, 0, 7000), (4, 0, 8000)];
    assert_eq!(got, want);
    // Every hand-over still gapless (the new next track was read ahead).
    assert!(sim.opens[1..].iter().all(|&(_, g)| g), "{:?}", sim.opens);
}

#[test]
fn requeue_of_a_shuffled_queue_keeps_the_order() {
    let lens: Vec<u64> = (1..=6).map(|i| i * 1000).collect();
    let (mut p, mut st, mut sim) = setup(&lens);
    sim.run(&mut p, &mut st, cmd(Command::Shuffle(true)));
    sim.run(&mut p, &mut st, cmd(Command::PlayQueue { id: 1, len: 6, start: 2 }));
    sim.play(&mut p, &mut st, 3000 + 4000 + 100);
    let order: Vec<u32> = (0..6).map(|pos| p.item_at(pos)).collect();
    let c = p.current().unwrap();
    // The firmware writes the play order into the queue.
    sim.lens = order.iter().map(|&i| lens[i as usize]).collect();
    let (item, dec, read) = sim.cur.unwrap();
    assert_eq!(item, c.item);
    sim.cur = Some((c.pos, dec, read));
    sim.played.clear();
    sim.run(&mut p, &mut st, cmd(Command::Requeue { id: 3, len: 6, pos: c.pos }));
    assert!(!p.shuffled());
    assert_eq!(p.current().unwrap().item, c.pos);
    sim.play(&mut p, &mut st, 1_000_000);
    let played: Vec<u64> = sim.played.iter().map(|r| sim.lens[r.0 as usize]).collect();
    let want: Vec<u64> = order[c.pos as usize..].iter().map(|&i| lens[i as usize]).collect();
    assert_eq!(played, want);
    // The resume record follows: new identity, unshuffled.
    p.handle(Event::Shutdown, &mut st, &mut Actions::default());
    let r = Player::new(Config::default(), &mut st).saved().unwrap();
    assert_eq!((r.queue_id, r.queue_len, r.shuffle), (3, 6, false));
}
