//! Building a queue from a directory (04 §6.5: "a directory listing
//! materialised when playback starts"). The player itself only knows the
//! queue's length and identity; the firmware keeps the locators (in
//! `/.orpheus/queue.bin`, a window of it in RAM) and resolves an item index
//! when the player asks to open it.

use orpheus_fs::{BlockDevice, DirEntry, Volume};

use crate::shuffle::Shuffle;
use crate::store::TrackRef;

/// File extensions the built codecs play (case-insensitive).
pub const AUDIO_EXTENSIONS: [&str; 9] = ["mp3", "mp2", "flac", "wav", "aif", "aiff", "aifc", "au", "snd"];

/// Whether a file name looks playable.
#[must_use]
pub fn is_audio(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => AUDIO_EXTENSIONS.iter().any(|e| e.eq_ignore_ascii_case(ext)),
        _ => false,
    }
}

/// Hand every playable file of directory `path` (in directory order, not
/// recursive) to `sink`; returns how many there were. Sorting belongs to
/// the directory index (AU-12); `sink` typically appends to the queue file.
pub async fn queue_dir<D: BlockDevice>(
    vol: &mut Volume<'_, D>,
    path: &str,
    mut sink: impl FnMut(TrackRef),
) -> Result<u32, orpheus_fs::Error<D::Error>> {
    let serial = vol.serial();
    let mut cur = vol.open_dir(path).await?;
    let mut e = DirEntry::new();
    let mut n = 0u32;
    while vol.read_dir(&mut cur, &mut e).await? {
        if !e.is_dir() && is_audio(e.name()) {
            sink(TrackRef::new(serial, e.locator(), e.name()));
            n += 1;
        }
    }
    Ok(n)
}

/// The longest queue [`edit`] works on (the firmware's RAM queue).
pub const EDIT_MAX: usize = 256;

/// A change to the queue from Up next or a file's menu. Positions are play
/// positions (the order Up next shows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit<T> {
    /// Insert right after the current track.
    PlayNext(T),
    /// Append at the end.
    Add(T),
    /// Remove the track at this position (after the current one).
    Remove(u32),
    /// Swap with the track before it (both after the current one).
    MoveUp(u32),
    /// Swap with the track after it.
    MoveDown(u32),
}

/// Apply `e` to the queue `q[..len]` whose current track is at play
/// position `at`, played in `order` (shuffled) or in queue order. The queue
/// is rewritten in play order (a shuffle is written into it, so the player
/// goes on unshuffled: `Command::Requeue`). Returns the new length, or
/// `None` when nothing changes: a position that is not after the current
/// track, a move past either end, a full queue ([`EDIT_MAX`] or `q.len()`),
/// a queue longer than [`EDIT_MAX`]. The current track keeps position `at`.
pub fn edit<T: Copy>(q: &mut [T], len: usize, at: u32, order: Option<&Shuffle>, e: Edit<T>) -> Option<usize> {
    let cap = q.len().min(EDIT_MAX);
    if len > cap || at as usize >= len.max(1) {
        return None;
    }
    let after = |i: u32| i > at && (i as usize) < len;
    let ok = match e {
        Edit::PlayNext(_) | Edit::Add(_) => len < cap,
        Edit::Remove(i) => after(i),
        Edit::MoveUp(i) => after(i) && i > at + 1,
        Edit::MoveDown(i) => after(i) && (i as usize) + 1 < len,
    };
    if !ok {
        return None;
    }
    if let Some(s) = order {
        unshuffle(q.get_mut(..len)?, s);
    }
    let at = at as usize;
    // Element moves through `get`: no panic paths in the firmware.
    Some(match e {
        Edit::PlayNext(x) => {
            let p = (at + 1).min(len);
            for i in (p..len).rev() {
                copy(q, i, i + 1);
            }
            set(q, p, x);
            len + 1
        }
        Edit::Add(x) => {
            set(q, len, x);
            len + 1
        }
        Edit::Remove(i) => {
            for k in i as usize..len - 1 {
                copy(q, k + 1, k);
            }
            len - 1
        }
        Edit::MoveUp(i) => {
            swap(q, i as usize - 1, i as usize);
            len
        }
        Edit::MoveDown(i) => {
            swap(q, i as usize, i as usize + 1);
            len
        }
    })
}

fn set<T>(q: &mut [T], i: usize, v: T) {
    if let Some(d) = q.get_mut(i) {
        *d = v;
    }
}

fn copy<T: Copy>(q: &mut [T], from: usize, to: usize) {
    if let Some(&v) = q.get(from) {
        set(q, to, v);
    }
}

fn swap<T: Copy>(q: &mut [T], a: usize, b: usize) {
    if let (Some(&x), Some(&y)) = (q.get(a), q.get(b)) {
        set(q, a, y);
        set(q, b, x);
    }
}

/// Rewrite `q` (at most [`EDIT_MAX`] items) in the play order of `s`:
/// afterwards `q[pos]` is the item `s` played at `pos`. In place, following
/// each cycle of the permutation once.
pub fn unshuffle<T: Copy>(q: &mut [T], s: &Shuffle) {
    let n = q.len().min(EDIT_MAX).min(s.len() as usize);
    let mut done = [0u64; EDIT_MAX / 64];
    let mut mark = |p: usize| match done.get_mut(p / 64) {
        Some(w) if *w & 1 << (p % 64) == 0 => {
            *w |= 1 << (p % 64);
            true
        }
        _ => false,
    };
    for start in 0..n {
        let Some(&first) = q.get(start) else { break };
        if !mark(start) {
            continue;
        }
        let mut p = start;
        loop {
            let src = s.item(p as u32) as usize;
            if src == start || src >= n {
                set(q, p, first);
                break;
            }
            copy(q, src, p);
            mark(src);
            p = src;
        }
    }
}

/// A window of values for play positions (the Up next titles): `w` holds
/// those of positions `first..first + n` in its first `n` slots. Move the
/// ones the window at `to` keeps into their slots (slot `i` is position
/// `to + i`) and return the mask of slots that hold their value now (bit
/// `i`: slot `i`; at most 32 slots). The rest are to be filled. Valid only
/// while the queue and its order stay the same (the firmware's queue
/// generation): a title is resolved once per window position it keeps,
/// not again each time the window moves on by one track (AU-PERF,
/// 2026-11-25: each title is a directory lookup on the card).
pub fn shift_window<T: Copy>(w: &mut [T], first: u32, n: u32, to: u32) -> u32 {
    let cap = w.len().min(32);
    let n = (n as usize).min(cap);
    // (No panicking paths: the firmware keeps panic sites out of its
    // resident image, and their locations would cost RAM.)
    let ones = |k: usize| (1u64.checked_shl(k as u32).unwrap_or(0).wrapping_sub(1)) as u32;
    if to >= first {
        let d = (to - first) as usize;
        if d >= n {
            return 0;
        }
        for i in 0..n - d {
            if let Some(&v) = w.get(i + d)
                && let Some(s) = w.get_mut(i)
            {
                *s = v;
            }
        }
        ones(n - d)
    } else {
        let d = (first - to) as usize;
        if d >= cap {
            return 0;
        }
        let k = n.min(cap - d);
        for i in (0..k).rev() {
            if let Some(&v) = w.get(i)
                && let Some(s) = w.get_mut(i + d)
            {
                *s = v;
            }
        }
        ones(k).checked_shl(d as u32).unwrap_or(0)
    }
}

/// Slots `0..` of a window [`shift_window`] mask filled without a gap
/// (what may be shown), at most `cap`.
#[must_use]
pub fn window_prefix(mask: u32, cap: u32) -> u32 {
    (!mask).trailing_zeros().min(cap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// Every kept slot holds the value of its position, every other slot
    /// is reported missing, for moves forward, back, by one (a track
    /// ended) and out of range.
    #[test]
    fn shift_window_keeps_the_shared_positions() {
        for n in [0u32, 1, 5, 8] {
            for first in [0u32, 3, 10, 40] {
                for to in [0u32, 1, 2, 3, 4, 9, 10, 11, 13, 17, 18, 40, 41, 47, 48, 60, u32::MAX] {
                    let mut w = [u32::MAX; 8];
                    for (i, s) in w.iter_mut().take(n as usize).enumerate() {
                        *s = first + i as u32;
                    }
                    let m = shift_window(&mut w, first, n, to);
                    for (i, &v) in w.iter().enumerate() {
                        let pos = u64::from(to) + i as u64;
                        let kept = pos >= u64::from(first) && pos < u64::from(first) + u64::from(n);
                        assert_eq!(m & 1 << i != 0, kept, "n {n} first {first} to {to} slot {i}");
                        if kept {
                            assert_eq!(u64::from(v), pos, "n {n} first {first} to {to} slot {i}");
                        }
                    }
                }
            }
        }
        assert_eq!(window_prefix(0b0111, 8), 3);
        assert_eq!(window_prefix(0b1110, 8), 0);
        assert_eq!(window_prefix(u32::MAX, 8), 8);
        assert_eq!(window_prefix(0b1111, 2), 2);
    }

    #[test]
    fn unshuffle_writes_the_play_order() {
        for n in [1u32, 2, 3, 17, 100, 256] {
            for seed in [1u32, 99, 0xBEEF] {
                let s = Shuffle::new(n, seed);
                let mut q: Vec<u32> = (0..n).map(|i| i * 10).collect();
                unshuffle(&mut q, &s);
                let want: Vec<u32> = (0..n).map(|p| s.item(p) * 10).collect();
                assert_eq!(q, want, "n {n} seed {seed}");
            }
        }
    }

    #[test]
    fn edits() {
        let mut q = [0u32; 8];
        let base = [10, 11, 12, 13, 14];
        let run = |at: u32, e: Edit<u32>| {
            let mut q = q;
            q[..5].copy_from_slice(&base);
            edit(&mut q, 5, at, None, e).map(|n| q[..n].to_vec())
        };
        assert_eq!(run(1, Edit::PlayNext(99)), Some(std::vec![10, 11, 99, 12, 13, 14]));
        assert_eq!(run(4, Edit::PlayNext(99)), Some(std::vec![10, 11, 12, 13, 14, 99]));
        assert_eq!(run(1, Edit::Add(99)), Some(std::vec![10, 11, 12, 13, 14, 99]));
        assert_eq!(run(1, Edit::Remove(3)), Some(std::vec![10, 11, 12, 14]));
        assert_eq!(run(1, Edit::Remove(4)), Some(std::vec![10, 11, 12, 13]));
        assert_eq!(run(1, Edit::Remove(1)), None, "not the current track");
        assert_eq!(run(1, Edit::Remove(0)), None, "nor one before it");
        assert_eq!(run(1, Edit::Remove(5)), None);
        assert_eq!(run(1, Edit::MoveUp(3)), Some(std::vec![10, 11, 13, 12, 14]));
        assert_eq!(run(1, Edit::MoveUp(2)), None, "not above the current track");
        assert_eq!(run(1, Edit::MoveDown(3)), Some(std::vec![10, 11, 12, 14, 13]));
        assert_eq!(run(1, Edit::MoveDown(4)), None, "not past the end");
        assert_eq!(run(5, Edit::Add(1)), None, "current beyond the queue");
        // Full.
        q = [7; 8];
        assert_eq!(edit(&mut q, 8, 0, None, Edit::Add(1)), None);
        assert_eq!(edit(&mut q, 8, 0, None, Edit::PlayNext(1)), None);
        let mut big = std::vec![0u32; 300];
        assert_eq!(edit(&mut big, 256, 0, None, Edit::Add(1)), None);
        assert_eq!(edit(&mut big, 255, 0, None, Edit::Add(1)), Some(256));
    }

    #[test]
    fn edit_of_a_shuffled_queue() {
        let s = Shuffle::new(6, 1234);
        let mut q: Vec<u32> = (0..7).collect();
        let order: Vec<u32> = (0..6).map(|p| s.item(p)).collect();
        // Current at position 2; Play next 100.
        assert_eq!(edit(&mut q, 6, 2, Some(&s), Edit::PlayNext(100)), Some(7));
        let mut want = order.clone();
        want.insert(3, 100);
        assert_eq!(q, want);
        // An edit that does not apply leaves the order (and the queue) alone.
        let mut q2: Vec<u32> = (0..6).collect();
        assert_eq!(edit(&mut q2, 6, 2, Some(&s), Edit::Remove(1)), None);
        assert_eq!(q2, (0..6).collect::<Vec<_>>());
    }

    #[test]
    fn extensions() {
        assert!(is_audio("a.MP3") && is_audio("x.flac") && is_audio("b.Wav"));
        assert!(!is_audio("cover.jpg") && !is_audio(".mp3") && !is_audio("mp3"));
    }
}
