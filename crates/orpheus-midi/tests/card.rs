//! The player's card source (SYN-3): the parser's `Need`s answered from
//! a block cache, the way the firmware drives it (`audio::synth`), and the
//! player built in place.

mod common;

use common::{demo, hash, player, render};
use orpheus_midi::cache::{BLOCK, BlockCache, Miss};
use orpheus_midi::demo as songs;
use orpheus_midi::{MidiPlayer, Need, Options, Pull};

/// A file behind a block cache of `slots` blocks; counts the block reads.
struct Card<'d> {
    data: &'d [u8],
    cache: BlockCache,
    mem: Vec<u8>,
    reads: u32,
}

impl<'d> Card<'d> {
    fn new(data: &'d [u8], slots: usize) -> Self {
        let mut cache = <BlockCache as bytemuck::Zeroable>::zeroed();
        cache.reset(slots);
        Self { data, cache, mem: vec![0; slots * BLOCK], reads: 0 }
    }

    /// What the firmware does for a `Need`.
    fn supply(&mut self, p: &mut MidiPlayer, need: Need) {
        let mut out = [0u8; 64];
        loop {
            match self.cache.answer(need, &self.mem, &mut out) {
                Ok(n) => return p.supply(&out[..n]),
                Err(Miss { block }) => {
                    self.reads += 1;
                    let s = self.cache.slot(block, &mut self.mem).unwrap();
                    let at = block as usize * BLOCK;
                    let n = self.data.len().saturating_sub(at).min(BLOCK);
                    s[..n].copy_from_slice(&self.data[at..at + n]);
                    self.cache.filled(block, n);
                }
            }
        }
    }

    fn open(&mut self, p: &mut MidiPlayer, kar: bool) {
        p.open(self.data.len() as u32, kar);
        loop {
            match p.step(64).unwrap() {
                Pull::Need(n) => self.supply(p, n),
                Pull::Ready(true) => return,
                Pull::Ready(false) => {}
                Pull::End => panic!("ended while opening"),
            }
        }
    }

    fn render(&mut self, p: &mut MidiPlayer, frames: usize, block: usize) -> Vec<i16> {
        let mut out = vec![0i16; 2 * frames];
        let mut done = 0;
        while done < frames {
            let hi = (done + block).min(frames);
            match p.render(&mut out[2 * done..2 * hi]) {
                Pull::Ready(n) => done += n,
                Pull::Need(n) => self.supply(p, n),
                Pull::End => break,
            }
        }
        out.truncate(2 * done);
        out
    }
}

/// Through a cache of 2, 4 or 32 blocks the player renders the golden
/// songs bit for bit, in the firmware's 256-frame blocks.
#[test]
fn card_source_renders_the_goldens() {
    let table = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens.txt")).unwrap();
    let want = |name: &str| {
        table.lines().find(|l| l.starts_with(name)).unwrap().split(' ').nth(1).unwrap().to_owned()
    };
    type Song = fn(&mut [u8]) -> Option<usize>;
    let cases: [(&str, Song, bool, u32, u32); 2] =
        [("etude-44k", songs::etude, false, 44_100, 4), ("karaoke-44k", songs::karaoke, true, 44_100, 8)];
    for (name, f, kar, rate, secs) in cases {
        let d = demo(f);
        for slots in [2, 4, 32] {
            let mut p = Box::new(<MidiPlayer as bytemuck::Zeroable>::zeroed());
            p.init(Options::new(rate));
            let mut card = Card::new(&d, slots);
            card.open(&mut p, kar);
            let out = card.render(&mut p, (rate * secs) as usize, 256);
            assert_eq!(format!("{:016x}", hash(&out)), want(name), "{name} through {slots} blocks");
            if slots == 32 {
                // Every block read once at the open (the scan), once more
                // at the rewind to 0 at most, and nothing while playing.
                let blocks = d.len().div_ceil(BLOCK) as u32;
                assert!(card.reads <= blocks, "{name}: {} reads of {blocks} blocks", card.reads);
            }
        }
    }
}

/// A seek through the card source lands where the slice source's does and
/// plays the same samples.
#[test]
fn card_source_seeks_like_the_slice_source() {
    let d = demo(songs::etude);
    let o = Options::new(44_100);
    let mut a = player(&d, o, false);
    let at = a.seek_from(&mut orpheus_midi::SliceSource::new(&d), 44_100 * 3).unwrap();
    let ra = render(&mut a, &d, 44_100, 1152);
    let mut b = Box::new(MidiPlayer::new(o));
    let mut card = Card::new(&d, 4);
    card.open(&mut b, false);
    b.seek(44_100 * 3).unwrap();
    loop {
        match b.step(64).unwrap() {
            Pull::Need(n) => card.supply(&mut b, n),
            Pull::Ready(true) => break,
            _ => {}
        }
    }
    assert_eq!(b.position(), at);
    let rb = card.render(&mut b, 44_100, 256);
    assert_eq!(ra, rb);
}

/// `MidiPlayer::init` on a player that has played leaves what `new` builds.
#[test]
fn init_in_place_equals_new() {
    let d = demo(songs::tour);
    let mut p = player(&d, Options::new(48_000), false);
    render(&mut p, &d, 20_000, 1152);
    for rate in [44_100, 48_000] {
        let o = Options { gain: 0.4, ..Options::new(rate) };
        p.init(o);
        assert_eq!(format!("{p:?}"), format!("{:?}", MidiPlayer::new(o)));
    }
}

/// The player's `play PATH` reply for the suite fixtures
/// (`/.orpheus/test/suite/midi/`, `xtask dev suite --setup`) with `dsp off`
/// and `crc on`: `frames=` the frames the synthesiser rendered at 44.1 kHz
/// (the song to its end + 2 s), `crc=` the CRC-32 of every frame written to
/// the PCM ring — the render's samples, then the play-out silence (the
/// current DAC half padded, two more halves of 1536 frames). The firmware
/// renders through this cache in 256-frame blocks with its voice limit at
/// 16; a lowered limit (`status` `syn: over=`) changes the CRC.
/// `tests/goldens-play.txt` (rewritten with `ORPHEUS_WRITE_GOLDENS=1`).
#[test]
fn play_crc_of_the_fixtures() {
    const HALF: usize = 1536;
    let mut table = String::new();
    for (name, f) in songs::SONGS {
        let kar = orpheus_midi::extension(name) == Some(true);
        let d = demo(f);
        let mut p = Box::new(MidiPlayer::new(Options { kar, ..Options::new(44_100) }));
        let mut card = Card::new(&d, orpheus_midi::cache::SLOTS);
        card.open(&mut p, kar);
        let total = p.duration() as usize + 4096;
        let out = card.render(&mut p, total, 256);
        let frames = out.len() / 2;
        assert_eq!(frames as u64, p.duration(), "{name}: rendered to the end");
        let mut crc = orpheus_proto::hash::crc32_update(0xFFFF_FFFF, &bytes(&out));
        let pad = (HALF - frames % HALF) % HALF + 2 * HALF;
        crc = orpheus_proto::hash::crc32_update(crc, &vec![0u8; 4 * pad]);
        table.push_str(&format!("{name} frames={frames} crc={}\n", !crc));
    }
    let file = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens-play.txt");
    if std::env::var_os("ORPHEUS_WRITE_GOLDENS").is_some() {
        std::fs::write(file, &table).unwrap();
        return;
    }
    let want = std::fs::read_to_string(file).expect("tests/goldens-play.txt: ORPHEUS_WRITE_GOLDENS=1");
    assert_eq!(table, want);
}

fn bytes(s: &[i16]) -> Vec<u8> {
    s.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// The demo songs fit the firmware's room after the player (its bench
/// renders them from memory).
#[test]
fn demo_songs_are_small() {
    for f in [songs::etude, songs::karaoke, songs::tour] {
        assert!(demo(f).len() <= 6 * 1024, "{}", demo(f).len());
    }
}
