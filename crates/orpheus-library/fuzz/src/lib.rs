//! Fuzz-target body of `orpheus-library` (VER-4), shared by the cargo-fuzz
//! target in `fuzz_targets/` (nightly) and the stable replay of the
//! committed corpus (`tests/fuzz_corpus.rs`, every `cargo test`).
//!
//! [`library_open`] — a library file (`lib-a.ydb` / `lib-b.ydb`) as found
//! on the card — damaged, of another version, or hostile — opened and
//! browsed the way the Music screens do: every top list, a window of rows,
//! the lists that open from them, track details, lookups by locator,
//! letters and letter jumps, play queues and Shuffle all. Byte 0 of the
//! input can repair the header CRC and the file length, so the fuzzer
//! reaches the queries with arbitrary section tables. Oracles: no panic, no
//! hang; queries answer within their lists (row windows of the asked size,
//! letter jumps inside the list, queues no longer than asked); names are
//! text; on a file that passes `verify`, rows and lookups agree with each
//! other (an item's position is its row, a track is found by its locator).

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::convert::Infallible;

use orpheus_library::{FileName, Item, ItemKind, Library, List, Storage};

/// Library files in memory.
#[derive(Default)]
pub struct Mem {
    pub files: HashMap<FileName, Vec<u8>>,
}

impl Storage for Mem {
    type Error = Infallible;
    fn len(&mut self, f: FileName) -> Result<Option<u32>, Infallible> {
        Ok(self.files.get(&f).map(|v| v.len() as u32))
    }
    fn read_at(&mut self, f: FileName, off: u32, buf: &mut [u8]) -> Result<usize, Infallible> {
        let Some(v) = self.files.get(&f) else { return Ok(0) };
        let o = (off as usize).min(v.len());
        let n = buf.len().min(v.len() - o);
        buf[..n].copy_from_slice(&v[o..o + n]);
        Ok(n)
    }
    fn write_at(&mut self, _f: FileName, _off: u32, _data: &[u8]) -> Result<(), Infallible> {
        panic!("queries never write");
    }
    fn truncate(&mut self, _f: FileName, _len: u32) -> Result<(), Infallible> {
        panic!("queries never write");
    }
}

fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

const HEADER: usize = 512;
const SECTIONS: usize = 20;
/// Largest file the target builds when it repairs the length.
const MAX_FILE: u64 = 1 << 20;

/// The file length the header's section table implies.
fn implied_len(b: &[u8]) -> u64 {
    (0..SECTIONS)
        .map(|i| u64::from(rd32(b, 48 + 12 * i)) + u64::from(rd32(b, 52 + 12 * i)))
        .max()
        .unwrap_or(0)
        .max(HEADER as u64)
}

/// Lists whose rows the target walks, with how many rows to look at.
const TOP: [List; 5] = [List::Artists, List::Albums, List::Songs, List::Genres, List::Recent];

fn name(l: &Library, s: &mut Mem, it: &Item) {
    let mut b = [0u8; 128];
    for r in [it.name, it.sub] {
        if let Ok(t) = l.text(s, r, &mut b) {
            assert!(t.len() <= 128);
        }
    }
}

/// Rows `first..first + k` of `list`, checked against the list length.
fn window(l: &Library, s: &mut Mem, list: List, n: u32, first: u32, k: usize) -> Vec<Item> {
    let mut out = vec![Item::default(); k];
    match l.rows(s, list, first, &mut out) {
        Ok(got) => {
            assert_eq!(got, (n.saturating_sub(first) as usize).min(k), "{list:?} rows from {first} of {n}");
            out.truncate(got);
            out
        }
        Err(_) => Vec::new(),
    }
}

fn walk(l: &Library, s: &mut Mem, list: List, depth: u32, verified: bool) {
    let Ok(n) = l.len(s, list) else { return };
    for first in [0, n / 2, n.saturating_sub(1), n] {
        let rows = window(l, s, list, n, first, 12);
        for (k, it) in rows.iter().enumerate() {
            name(l, s, it);
            if verified {
                let p = l.position_of(s, list, it.id).expect("position in a verified file");
                assert!(p.is_some_and(|p| p < n), "{list:?}: item {} not found", it.id);
                assert_eq!(l.row(s, list, p.unwrap()).unwrap().map(|x| x.id), Some(it.id));
            }
            if it.kind == ItemKind::Track {
                if let Ok(t) = l.track(s, it.id) {
                    for r in [t.title, t.artist, t.album, t.album_artist, t.genre] {
                        let mut b = [0u8; 128];
                        let _ = l.text(s, r, &mut b);
                    }
                    let found = l.find(s, t.r.locator());
                    if verified {
                        let id = found.unwrap().expect("a track is found by its locator");
                        assert_eq!(l.track(s, id).unwrap().r.locator(), t.r.locator());
                    }
                }
            } else if depth < 2 {
                if let Some(child) = list.open(it) {
                    walk(l, s, child, depth + 1, verified);
                }
                if let Some(tracks) = List::tracks_of(it) {
                    queue(l, s, tracks);
                }
            }
            if k >= 3 {
                break;
            }
        }
    }
    // Letters.
    for i in [0, 1, n / 2, n.saturating_sub(1), n] {
        let _ = l.letter(s, list, i);
        for fwd in [true, false] {
            if let Ok(Some((j, c))) = l.seek_letter(s, list, i, fwd) {
                assert!(j < n, "{list:?}: letter jump to {j} of {n}");
                if verified {
                    assert_eq!(l.letter(s, list, j), Ok(Some(c)));
                }
            }
        }
    }
    let mut runs = [(0u32, None); 16];
    if let Ok(k) = l.letter_runs(s, list, 0, &mut runs) {
        assert!(k <= runs.len());
        for &(f, _) in &runs[..k] {
            if verified {
                assert!(f < n.max(1), "{list:?}: letter run at {f} of {n}");
            }
        }
    }
    if list.is_tracks() {
        queue(l, s, list);
    }
}

fn queue(l: &Library, s: &mut Mem, list: List) {
    for (first, max) in [(0, 40), (3, 5), (u32::MAX, 10)] {
        let mut got = 0u32;
        if let Ok(n) = l.queue(s, list, first, max, |_| {
            got += 1;
            got < 1000
        }) {
            assert!(n <= max && got <= max, "{list:?}: queue of {n}/{got} for {max}");
        }
    }
}

/// See the crate docs. Byte 0: bit 0 repairs the header CRC, bit 1 the
/// file length, bit 2 puts the file in slot B; bit 3 also puts the file
/// with the next generation into the other slot.
pub fn library_open(data: &[u8]) {
    let Some((&sel, file)) = data.split_first() else { return };
    let mut file = file.to_vec();
    if sel & 2 != 0 && file.len() >= HEADER {
        let len = implied_len(&file);
        if len <= MAX_FILE {
            file.resize(len as usize, 0);
        }
    }
    let crc = |f: &mut Vec<u8>| {
        if f.len() >= HEADER {
            let c = orpheus_index::crc32_update(0, &f[..508]);
            f[508..512].copy_from_slice(&c.to_le_bytes());
        }
    };
    if sel & 1 != 0 {
        crc(&mut file);
    }
    let slots = [FileName::new("lib-a.ydb").unwrap(), FileName::new("lib-b.ydb").unwrap()];
    let slot = usize::from(sel >> 2 & 1);
    let mut s = Mem::default();
    if sel & 8 != 0 && file.len() >= HEADER {
        let mut next = file.clone();
        let g = rd32(&next, 8).wrapping_add(1);
        next[8..12].copy_from_slice(&g.to_le_bytes());
        crc(&mut next);
        s.files.insert(slots[1 - slot], next);
    }
    s.files.insert(slots[slot], file);
    let Some(l) = Library::open(&mut s).expect("memory storage does not fail") else { return };
    assert!(l.file_len() >= HEADER as u64);
    let mut buf = [0u8; 1000];
    // "Verified": the section CRCs match and the header was not repaired by
    // the target (bits 0 and 3: a repaired header may carry any counts).
    let verified = sel & 9 == 0 && l.verify(&mut s, &mut buf).unwrap_or(false);
    for k in [
        orpheus_library::Kind::Artist,
        orpheus_library::Kind::Album,
        orpheus_library::Kind::Genre,
        orpheus_library::Kind::Title,
    ] {
        let _ = l.strings(k);
    }
    let _ = (l.tracks(), l.artists(), l.albums(), l.genres(), l.folders(), l.generation(), l.stamp());
    for list in TOP {
        walk(&l, &mut s, list, 0, verified);
    }
    for (seed, n) in [(1, 20), (sel.into(), 3), (7, 0)] {
        let mut got = 0u32;
        if let Ok(k) = l.sample(&mut s, seed, n, |_| {
            got += 1;
            true
        }) {
            assert!(k <= n.min(l.tracks()) && got == k, "sample of {k}/{got} for {n} of {}", l.tracks());
        }
    }
}
