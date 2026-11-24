//! Narrowing a byte source to a picture embedded in another file (album art
//! in ID3 `APIC`, FLAC `PICTURE`, Vorbis `METADATA_BLOCK_PICTURE` after
//! base64 is not handled here), so embedded pictures go through the same
//! decoders as files on the card.
//!
//! `yoton-meta` reports a picture as a location: absolute offset, logical
//! length, logical bytes to skip, and whether the bytes are ID3
//! unsynchronised. The caller seeks its source to the offset and wraps it:
//!
//! * plain bytes: [`ArtSource::plain`] limits the stream to `len` bytes
//!   (no copy);
//! * unsynchronised bytes: [`ArtSource::unsync`] copies through a staging
//!   buffer (≥ [`crate::MAX_PEEK`] bytes, from the arena) removing the
//!   `00` after every `FF`; call [`ArtSource::pump`] before every decoder
//!   step.

use yoton_audio::ByteSource;

pub struct ArtSource<'s, 'b> {
    inner: &'s mut dyn ByteSource,
    /// Logical bytes not yet handed out (plain: not yet consumed).
    left: u64,
    stage: Option<Stage<'b>>,
    start: u64,
    consumed: u64,
}

struct Stage<'b> {
    buf: &'b mut [u8],
    head: usize,
    tail: usize,
    skip: u32,
    prev_ff: bool,
}

impl<'s, 'b> ArtSource<'s, 'b> {
    /// The next `len` bytes of `inner`.
    pub fn plain(inner: &'s mut dyn ByteSource, len: u64) -> Self {
        let start = inner.position();
        Self { inner, left: len, stage: None, start, consumed: 0 }
    }

    /// `len` logical bytes of an unsynchronised stream, after discarding
    /// `skip` logical bytes; `buf` is the staging buffer.
    pub fn unsync(inner: &'s mut dyn ByteSource, len: u64, skip: u32, buf: &'b mut [u8]) -> Self {
        let start = inner.position();
        let stage = Stage { buf, head: 0, tail: 0, skip, prev_ff: false };
        let mut s = Self { inner, left: len, stage: Some(stage), start, consumed: 0 };
        s.pump();
        s
    }

    /// Move bytes from the inner source into the staging buffer (no-op for
    /// plain sources). Returns whether anything moved.
    pub fn pump(&mut self) -> bool {
        let Some(st) = self.stage.as_mut() else { return false };
        if st.head > 0 {
            st.buf.copy_within(st.head..st.tail, 0);
            st.tail -= st.head;
            st.head = 0;
        }
        let mut moved = false;
        loop {
            let staged = (st.tail - st.head) as u64;
            if st.tail >= st.buf.len() || (st.skip == 0 && staged >= self.left) {
                break;
            }
            let Some((a, b)) = self.inner.peek(self.inner.available()) else { break };
            if a.is_empty() && b.is_empty() {
                break;
            }
            let mut raw = 0;
            for &byte in a.iter().chain(b.iter()) {
                if st.tail >= st.buf.len() || (st.skip == 0 && (st.tail - st.head) as u64 >= self.left) {
                    break;
                }
                raw += 1;
                let drop = st.prev_ff && byte == 0;
                st.prev_ff = byte == 0xFF;
                if drop {
                    continue;
                }
                if st.skip > 0 {
                    st.skip -= 1;
                    continue;
                }
                st.buf[st.tail] = byte;
                st.tail += 1;
            }
            if raw == 0 {
                break;
            }
            self.inner.consume(raw);
            moved = true;
        }
        moved
    }
}

impl ByteSource for ArtSource<'_, '_> {
    fn peek(&self, n: usize) -> Option<(&[u8], &[u8])> {
        if n > self.available() {
            return None;
        }
        match &self.stage {
            Some(st) => Some((&st.buf[st.head..st.head + n], &[])),
            None => self.inner.peek(n),
        }
    }

    fn available(&self) -> usize {
        match &self.stage {
            Some(st) => (st.tail - st.head).min(usize::try_from(self.left).unwrap_or(usize::MAX)),
            None => self.inner.available().min(usize::try_from(self.left).unwrap_or(usize::MAX)),
        }
    }

    fn consume(&mut self, n: usize) {
        let n = n.min(self.available());
        match &mut self.stage {
            Some(st) => st.head += n,
            None => self.inner.consume(n),
        }
        self.left -= n as u64;
        self.consumed += n as u64;
    }

    fn position(&self) -> u64 {
        match &self.stage {
            Some(_) => self.consumed,
            None => self.inner.position() - self.start,
        }
    }

    fn len(&self) -> Option<u64> {
        Some(self.consumed + self.left)
    }

    fn at_end(&self) -> bool {
        let staged = self.available() as u64;
        staged >= self.left
            || match &self.stage {
                Some(_) => self.inner.at_end() && self.inner.available() == 0,
                None => self.inner.at_end(),
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yoton_audio::SliceSource;

    #[test]
    fn plain_limits() {
        let data: std::vec::Vec<u8> = (0..100u8).collect();
        let mut s = SliceSource::new(&data);
        s.consume(10);
        let mut a = ArtSource::plain(&mut s, 20);
        assert_eq!(a.available(), 20);
        assert!(a.at_end());
        assert!(a.peek(21).is_none());
        assert_eq!(a.peek(2).unwrap().0, &[10, 11]);
        a.consume(5);
        assert_eq!(a.position(), 5);
        assert_eq!(a.available(), 15);
    }

    #[test]
    fn unsync_removes_zero_after_ff() {
        let data = [9u8, 9, 0xFF, 0x00, 0xD8, 0xFF, 0x00, 0x00, 1, 2, 3, 4, 5];
        let mut s = SliceSource::new(&data);
        let mut buf = [0u8; 4];
        let mut a = ArtSource::unsync(&mut s, 6, 2, &mut buf);
        let mut out = std::vec::Vec::new();
        for _ in 0..10 {
            let n = a.available();
            out.extend_from_slice(a.peek(n).unwrap().0);
            a.consume(n);
            if a.at_end() && a.available() == 0 {
                break;
            }
            a.pump();
        }
        assert_eq!(&out[..], &[0xFF, 0xD8, 0xFF, 0x00, 1, 2]);
        assert_eq!(a.position(), 6);
    }
}
