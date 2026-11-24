//! Reading helpers over a [`SeekSource`] for restartable parsers: a unit
//! (frame, chunk, item) is peeked whole — at most a small, fixed number of
//! bytes — processed, then skipped; when the bytes are not buffered yet the
//! parser answers `NeedData` without having consumed anything.

use orpheus_audio::{ByteSource, MAX_PEEK, SeekSource, SourceError, View};

/// Why a read could not complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pending {
    /// Call again when this many bytes are buffered.
    Need(usize),
    /// The stream ends first.
    Eof,
}

/// `n` bytes at the read position.
pub(crate) fn peek(src: &dyn ByteSource, n: usize) -> Result<View<'_>, Pending> {
    if let Some(v) = View::peek(src, n) {
        return Ok(v);
    }
    if src.at_end() || n > MAX_PEEK { Err(Pending::Eof) } else { Err(Pending::Need(n)) }
}

/// Skip `n` bytes: consume what is buffered, seek past the rest.
pub(crate) fn skip(src: &mut dyn SeekSource, n: u64) -> Result<(), SourceError> {
    let avail = src.available() as u64;
    if n <= avail {
        src.consume(n as usize);
        Ok(())
    } else {
        let to = src.position().saturating_add(n);
        src.seek(to)
    }
}

/// Move to absolute position `pos` (consuming when it is buffered ahead).
pub(crate) fn goto(src: &mut dyn SeekSource, pos: u64) -> Result<(), SourceError> {
    let here = src.position();
    if pos >= here { skip(src, pos - here) } else { src.seek(pos) }
}

/// Copy `n` logical bytes at the read position into `dst` without
/// consuming, removing ID3 unsynchronisation (`FF 00` → `FF`) when
/// `unsync` is set; `prev_ff` says whether the byte before the read
/// position was `FF`. Returns the raw byte count they occupy.
pub(crate) fn peek_logical(
    src: &dyn ByteSource,
    unsync: bool,
    prev_ff: bool,
    dst: &mut [u8],
) -> Result<usize, Pending> {
    let n = dst.len();
    if !unsync {
        let v = peek(src, n)?;
        v.copy_to(0, dst);
        return Ok(n);
    }
    let avail = src.available().min(2 * n + 2);
    let v = View::peek(src, avail).unwrap_or_default();
    let (mut ff, mut out, mut i) = (prev_ff, 0usize, 0usize);
    while out < n {
        let Some(b) = v.get(i) else {
            return Err(if src.at_end() || avail >= MAX_PEEK {
                Pending::Eof
            } else {
                Pending::Need(avail + 1)
            });
        };
        i += 1;
        if ff && b == 0 {
            ff = false;
            continue;
        }
        dst[out] = b;
        out += 1;
        ff = b == 0xFF;
    }
    Ok(i)
}

/// Skip up to `n` logical (unsynchronised) bytes from what is buffered.
/// Returns the logical bytes skipped; updates `prev_ff`.
pub(crate) fn skip_logical(src: &mut dyn ByteSource, n: u64, prev_ff: &mut bool) -> u64 {
    let v = View::of(src);
    let (mut done, mut i) = (0u64, 0usize);
    while done < n {
        let Some(b) = v.get(i) else { break };
        i += 1;
        if *prev_ff && b == 0 {
            *prev_ff = false;
            continue;
        }
        done += 1;
        *prev_ff = b == 0xFF;
    }
    src.consume(i);
    done
}

/// Remove unsynchronisation in place; returns the new length.
pub(crate) fn unsync_in_place(b: &mut [u8]) -> usize {
    let (mut w, mut ff) = (0usize, false);
    for r in 0..b.len() {
        let x = b[r];
        if ff && x == 0 {
            ff = false;
            continue;
        }
        b[w] = x;
        w += 1;
        ff = x == 0xFF;
    }
    w
}
