//! A synthesiser source beside the codecs (SYN-3, 04 §9): which source a
//! file is, how the playback loop starts it, and where the synthesiser's
//! memory goes.
//!
//! The synthesiser (a MIDI file's player, later the Sequencer and speech)
//! is "just another source": the card task plays either a codec over the
//! stream ring and the decoder arena, or a synth source that renders its
//! blocks from parameters and a few file bytes. The two are never live
//! together. A synth source takes the **pool**: the idle stream ring
//! (16 KiB) followed by the decoder arena, one contiguous region in the
//! firmware (`audio::POOL`), less the library's build state at the arena's
//! end. [`carve`] lays the synth's state, its file block cache and the
//! spare bytes for card work while it plays (player state saves, served
//! requests) out in that region; it is host-tested here and used as is by
//! the firmware.
//!
//! The start rules ([`plan_start`]):
//!
//! * a codec item handed over gapless (the stream ring holds its head)
//!   goes on as before;
//! * a synth item is never gapless: the previous track's PCM plays out,
//!   the ring's read-ahead is dropped (the ring's memory is lent to the
//!   synth), and the file opens fresh — even when the gapless read-ahead
//!   already read it;
//! * after a synth item the next codec item opens fresh too (nothing was
//!   read ahead: the synth never reports the reader's end).

use core::ops::Range;

/// What plays a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// A codec over the stream ring (everything that is not a synth file;
    /// the probe decides whether it plays).
    Codec,
    /// A Standard MIDI File through the synthesiser; `kar`: a karaoke file
    /// (`.kar`: text events are lyrics).
    Midi {
        /// Karaoke file.
        kar: bool,
    },
}

/// The source for a file with extension `ext` (without the dot, any case):
/// `mid`, `midi`, `rmi` and `kar` are MIDI files.
#[must_use]
pub fn source_of(ext: Option<&[u8]>) -> SourceKind {
    let Some(e) = ext else { return SourceKind::Codec };
    let is = |x: &[u8]| e.eq_ignore_ascii_case(x);
    if is(b"kar") {
        SourceKind::Midi { kar: true }
    } else if is(b"mid") || is(b"midi") || is(b"rmi") {
        SourceKind::Midi { kar: false }
    } else {
        SourceKind::Codec
    }
}

/// How the playback loop starts an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// The gapless hand-over goes on: the stream ring holds the file's
    /// head, the DAC keeps running.
    Gapless,
    /// Open the file and a codec fresh (the previous output played out).
    Codec,
    /// Play out the previous output, drop the read-ahead, lend the pool to
    /// the synthesiser and open the file there.
    Synth {
        /// Karaoke file.
        kar: bool,
    },
}

/// The start of an item of `kind`; `handed`: the gapless hand-over took it
/// (its head is in the stream ring).
#[must_use]
pub fn plan_start(handed: bool, kind: SourceKind) -> Start {
    match kind {
        SourceKind::Midi { kar } => Start::Synth { kar },
        SourceKind::Codec if handed => Start::Gapless,
        SourceKind::Codec => Start::Codec,
    }
}

/// Where the synthesiser's memory lies in the pool (offsets from its
/// start).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The synth's state (an engine, a MIDI player): one value.
    pub state: Range<usize>,
    /// The file block cache's blocks (`blocks` × `block` bytes).
    pub cache: Range<usize>,
    /// Bytes for card work while it plays (at least the asked minimum:
    /// everything that is left).
    pub spare: Range<usize>,
    /// Blocks in the cache.
    pub blocks: usize,
}

/// Lay out a state of `size` bytes aligned to `align` (a power of two) at
/// the start of a region of `len` bytes at address `addr`, then up to
/// `max_blocks` cache blocks of `block` bytes (aligned to 4), keeping at
/// least `spare_min` bytes at the end. `None`: the state and `spare_min`
/// do not fit with at least `min_blocks` blocks.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn carve(
    addr: usize,
    len: usize,
    size: usize,
    align: usize,
    block: usize,
    min_blocks: usize,
    max_blocks: usize,
    spare_min: usize,
) -> Option<Layout> {
    if !align.is_power_of_two() || block == 0 {
        return None;
    }
    let pad = addr.wrapping_neg() & (align - 1);
    let state_end = pad.checked_add(size)?;
    let cache_at = state_end.checked_add(state_end.wrapping_neg() & 3)?;
    let room = len.checked_sub(cache_at)?.checked_sub(spare_min)?;
    let blocks = (room / block).min(max_blocks);
    if blocks < min_blocks {
        return None;
    }
    let cache_end = cache_at + blocks * block;
    Some(Layout { state: pad..state_end, cache: cache_at..cache_end, spare: cache_end..len, blocks })
}

/// What a synth source was doing when the DAC ran dry: the `pcm: head`
/// line names it `ub=synth:<label>:<ms>` (the player, 2026-11-26: synth
/// underruns logged `ub=-`, the render was never noted). The load governor
/// itself is `orpheus_synth::load::Governor`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SynthOp {
    /// A render piece.
    Render = 0,
    /// A rest for thread mode (`orpheus_power::pace`).
    Rest = 1,
    /// A card block of the MIDI file read while playing.
    Read = 2,
}

/// The labels of [`SynthOp`] in order, NUL-separated (a paged `static`: a
/// plain literal would be resident on the player).
pub static SYNTH_LABELS: [u8; 16] = *b"render\0rest\0read";
/// The prefix of a synth label.
pub static SYNTH_PREFIX: [u8; 6] = *b"synth:";

/// The label of synth operation `i` ([`SynthOp`] as `u8`).
#[must_use]
pub fn synth_label(i: u8) -> Option<&'static str> {
    let b = SYNTH_LABELS.split(|&c| c == 0).nth(usize::from(i))?;
    core::str::from_utf8(b).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression (2026-11-26, the player, build `1fda608d`): underruns
    /// while a MIDI file played were logged `pcm: head … ub=-` — the synth
    /// noted nothing in the ledger, so the line could not say what kept
    /// the writer from the DAC. Every synth operation has a label.
    #[test]
    fn regress_synth_underruns_are_labelled() {
        assert_eq!(synth_label(SynthOp::Render as u8), Some("render"));
        assert_eq!(synth_label(SynthOp::Rest as u8), Some("rest"));
        assert_eq!(synth_label(SynthOp::Read as u8), Some("read"));
        assert_eq!(synth_label(3), None);
        assert_eq!(&SYNTH_PREFIX, b"synth:");
    }

    #[test]
    fn midi_extensions_in_any_case() {
        assert_eq!(source_of(Some(b"MID")), SourceKind::Midi { kar: false });
        assert_eq!(source_of(Some(b"midi")), SourceKind::Midi { kar: false });
        assert_eq!(source_of(Some(b"Rmi")), SourceKind::Midi { kar: false });
        assert_eq!(source_of(Some(b"kar")), SourceKind::Midi { kar: true });
        for e in [&b"mp3"[..], b"flac", b"mi", b"midx", b"", b"wav"] {
            assert_eq!(source_of(Some(e)), SourceKind::Codec, "{e:?}");
        }
        assert_eq!(source_of(None), SourceKind::Codec);
    }

    /// Every pair of (previous, next) source and hand-over: a synth item
    /// never continues a gapless hand-over; a codec item does only when it
    /// was handed over.
    #[test]
    fn switch_rules() {
        let kinds = [SourceKind::Codec, SourceKind::Midi { kar: false }, SourceKind::Midi { kar: true }];
        for k in kinds {
            for handed in [false, true] {
                let s = plan_start(handed, k);
                match k {
                    SourceKind::Codec => {
                        assert_eq!(s, if handed { Start::Gapless } else { Start::Codec });
                    }
                    SourceKind::Midi { kar } => assert_eq!(s, Start::Synth { kar }),
                }
            }
        }
    }

    #[test]
    fn carve_aligns_and_keeps_the_spare() {
        // The firmware's numbers: 16 KiB ring + 64 KiB arena less a 1.5 KiB
        // library state; a 70 KB player, 512-byte blocks, 2 KiB spare.
        let len = 16 * 1024 + 64 * 1024 - 1536;
        let l = carve(0x10_0004, len, 70_528, 8, 512, 4, 32, 2048).unwrap();
        assert_eq!(l.state, 4..4 + 70_528);
        assert_eq!((0x10_0004 + l.state.start) % 8, 0);
        assert_eq!(l.cache.start % 4, 0);
        assert!(l.cache.start >= l.state.end);
        assert_eq!(l.cache.len(), l.blocks * 512);
        assert!(l.spare.len() >= 2048);
        assert_eq!(l.spare.end, len);
        assert!(l.blocks >= 4 && l.blocks <= 32);
        // At most `max_blocks`: the rest is spare.
        let l = carve(0, 1 << 20, 100, 8, 512, 1, 16, 0).unwrap();
        assert_eq!(l.blocks, 16);
        assert_eq!(l.spare.len(), (1 << 20) - 100 - 16 * 512);
    }

    #[test]
    fn carve_refuses_what_does_not_fit() {
        assert_eq!(carve(0, 1000, 1001, 8, 512, 0, 8, 0), None);
        // The state fits, the spare does not.
        assert_eq!(carve(0, 1000, 900, 8, 512, 0, 8, 200), None);
        // Too few blocks.
        assert_eq!(carve(0, 4096, 1000, 8, 512, 6, 8, 512), None);
        assert!(carve(0, 4096, 1000, 8, 512, 5, 8, 512).is_some());
        // Alignment padding counts.
        assert_eq!(carve(1, 1000, 999, 8, 512, 0, 8, 0), None);
        assert_eq!(carve(0, 1000, 10, 3, 512, 0, 8, 0), None);
        assert_eq!(carve(0, usize::MAX, usize::MAX, 8, 512, 0, 8, 0), None);
    }
}
