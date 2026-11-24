//! The module's UART frame (docs/hw/bluetooth.md §3.1):
//!
//! ```text
//! AA | LEN_H LEN_L | ID | P0 .. Pn-1 | CHK
//! ```
//!
//! `LEN` (big-endian) counts ID and parameters; `CHK` makes the byte sum
//! from `LEN_H` through `CHK` zero. No escaping: the receiver hunts for
//! `AA`, rejects `LEN` above 44 and drops frames with a bad checksum. This
//! decoder additionally rescans the bytes of a rejected candidate, so an
//! `AA` inside garbage does not hide the frame that follows it.

/// Start-of-frame byte.
pub const SYNC: u8 = 0xAA;
/// Largest `LEN` the receiver accepts (ID + parameters).
pub const MAX_RX_LEN: usize = 44;
/// Most parameters in a received frame.
pub const MAX_RX_PARAMS: usize = MAX_RX_LEN - 1;
/// Most parameters in a command the crate sends (metadata step 2: sub-id
/// plus 72 text bytes).
pub const MAX_TX_PARAMS: usize = 73;
/// Largest command frame the crate sends.
pub const MAX_TX_FRAME: usize = 5 + MAX_TX_PARAMS;

/// Command IDs that only their owning work package may send (CLAUDE.md
/// rule 6): module upgrade 0x20–0x23, shutdown 0x24, baud change 0x28,
/// settings read 0x2C, settings write 0x7F. [`encode`] refuses them; see
/// [`crate::restricted`].
#[must_use]
pub const fn is_restricted(id: u8) -> bool {
    matches!(id, 0x20..=0x24 | 0x28 | 0x2C | 0x7F)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// Output buffer too small.
    TooLong,
    /// A restricted command ID (rule 6).
    Restricted,
}

/// Two's complement of the byte sum.
#[must_use]
pub fn checksum(bytes: &[u8]) -> u8 {
    0u8.wrapping_sub(bytes.iter().fold(0u8, |a, &b| a.wrapping_add(b)))
}

/// Encode without the restricted-ID check (crate-internal and
/// [`crate::restricted`]).
pub(crate) fn encode_unchecked(id: u8, params: &[u8], out: &mut [u8]) -> Result<usize, EncodeError> {
    // Slices by `get`: no panic paths (their location strings would cost
    // resident RAM in the firmware).
    const TOO_LONG: Result<usize, EncodeError> = Err(EncodeError::TooLong);
    let n = 5 + params.len();
    let Ok(len) = u16::try_from(params.len() + 1) else { return TOO_LONG };
    let Some((head, rest)) = out.get_mut(..n).and_then(|o| o.split_at_mut_checked(4)) else {
        return TOO_LONG;
    };
    let Some((body, [chk])) = rest.split_at_mut_checked(params.len()) else { return TOO_LONG };
    let [sync, len_h, len_l, cmd] = head else { return TOO_LONG };
    (*sync, *len_h, *len_l, *cmd) = (SYNC, (len >> 8) as u8, len as u8, id);
    body.copy_from_slice(params);
    *chk = checksum(&[*len_h, *len_l, *cmd]).wrapping_add(checksum(body));
    Ok(n)
}

/// Encode a command frame into `out`; returns its length. Refuses the
/// restricted IDs.
pub fn encode(id: u8, params: &[u8], out: &mut [u8]) -> Result<usize, EncodeError> {
    if is_restricted(id) {
        return Err(EncodeError::Restricted);
    }
    encode_unchecked(id, params, out)
}

/// A received frame.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub id: u8,
    len: u8,
    params: [u8; MAX_RX_PARAMS],
}

impl core::fmt::Debug for Frame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Frame").field("id", &self.id).field("params", &self.params()).finish()
    }
}

impl Frame {
    /// Build a frame value (tests, replay). `params` longer than
    /// [`MAX_RX_PARAMS`] are cut.
    #[must_use]
    pub fn new(id: u8, params: &[u8]) -> Self {
        let n = params.len().min(MAX_RX_PARAMS);
        let mut p = [0u8; MAX_RX_PARAMS];
        p[..n].copy_from_slice(&params[..n]);
        Self { id, len: n as u8, params: p }
    }

    #[must_use]
    pub fn params(&self) -> &[u8] {
        &self.params[..usize::from(self.len).min(MAX_RX_PARAMS)]
    }
}

/// Decoder counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecodeStats {
    pub frames: u32,
    /// Bytes dropped while hunting for `AA`.
    pub skipped: u32,
    pub bad_length: u32,
    pub bad_checksum: u32,
}

const CAND: usize = 4 + MAX_RX_LEN;

enum Step {
    More,
    Frame(Frame),
    Fail,
}

/// Streaming decoder. Feed it any chunking of the byte stream.
pub struct Decoder {
    buf: [u8; CAND],
    n: usize,
    pub stats: DecodeStats,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buf: [0; CAND],
            n: 0,
            stats: DecodeStats { frames: 0, skipped: 0, bad_length: 0, bad_checksum: 0 },
        }
    }

    /// Drop any partial frame (after a module power cycle).
    pub fn reset(&mut self) {
        self.n = 0;
    }

    fn step(&mut self, c: u8) -> Step {
        if self.n == 0 {
            if c == SYNC {
                self.buf[0] = c;
                self.n = 1;
            } else {
                self.stats.skipped += 1;
            }
            return Step::More;
        }
        let Some(slot) = self.buf.get_mut(self.n) else {
            // Cannot happen (a candidate ends at 4 + MAX_RX_LEN bytes).
            self.n = 0;
            return Step::More;
        };
        *slot = c;
        self.n += 1;
        if self.n < 3 {
            return Step::More;
        }
        let len = usize::from(u16::from_be_bytes([self.buf[1], self.buf[2]]));
        if len == 0 || len > MAX_RX_LEN {
            self.stats.bad_length += 1;
            return Step::Fail;
        }
        if self.n < 4 + len {
            return Step::More;
        }
        let total = self.n;
        self.n = 0;
        let cand = self.buf.get(..total).unwrap_or(&[]);
        if cand.len() < 5 || checksum(&cand[1..]) != 0 {
            // `checksum` of a range that includes CHK is 0 when valid.
            self.n = total;
            self.stats.bad_checksum += 1;
            return Step::Fail;
        }
        self.stats.frames += 1;
        Step::Frame(Frame::new(cand[3], &cand[4..cand.len() - 1]))
    }

    /// Feed one byte; `out` receives every completed frame (a rejected
    /// candidate can release more than one when rescanned).
    pub fn push(&mut self, b: u8, out: &mut impl FnMut(Frame)) {
        let mut pending = [0u8; 2 * CAND];
        pending[0] = b;
        let (mut i, mut np) = (0usize, 1usize);
        while i < np {
            let Some(&c) = pending.get(i) else { break };
            i += 1;
            match self.step(c) {
                Step::More => {}
                Step::Frame(f) => out(f),
                Step::Fail => {
                    // Replay everything after the rejected AA, ahead of the
                    // bytes not yet processed.
                    // `held` ≤ CAND and `np - i` ≤ CAND: `next` holds both
                    // (written with `get` to keep panic paths out).
                    let held = self.n.min(CAND);
                    self.n = 0;
                    let mut next = [0u8; 2 * CAND];
                    let mut t = 0;
                    for &c in
                        self.buf.get(1..held).unwrap_or(&[]).iter().chain(pending.get(i..np).unwrap_or(&[]))
                    {
                        if let Some(d) = next.get_mut(t) {
                            *d = c;
                            t += 1;
                        }
                    }
                    pending = next;
                    i = 0;
                    np = t;
                }
            }
        }
    }

    /// Feed a chunk.
    pub fn feed(&mut self, bytes: &[u8], mut out: impl FnMut(Frame)) {
        for &b in bytes {
            self.push(b, &mut out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn enc(id: u8, p: &[u8]) -> Vec<u8> {
        let mut b = [0u8; MAX_TX_FRAME];
        let n = encode_unchecked(id, p, &mut b).unwrap();
        b[..n].to_vec()
    }

    fn decode_all(bytes: &[u8]) -> (Vec<Frame>, DecodeStats) {
        let mut d = Decoder::new();
        let mut v = Vec::new();
        d.feed(bytes, |f| v.push(f));
        (v, d.stats)
    }

    /// The byte-exact examples of bluetooth.md §3.4 and the frames seen in
    /// the RAM dump (§3.1, §4.1).
    #[test]
    fn documented_examples() {
        let cases: [(u8, &[u8], &[u8]); 18] = [
            (0x06, &[0x02], &[0xAA, 0x00, 0x02, 0x06, 0x02, 0xF6]),
            (0x2C, &[], &[0xAA, 0x00, 0x01, 0x2C, 0xD3]),
            (0x30, &[0x01, 0x01], &[0xAA, 0x00, 0x03, 0x30, 0x01, 0x01, 0xCB]),
            (0x2A, &[0xFF], &[0xAA, 0x00, 0x02, 0x2A, 0xFF, 0xD5]),
            (0x00, &[0x0F, 0x0C, 0x03], &[0xAA, 0x00, 0x04, 0x00, 0x0F, 0x0C, 0x03, 0xDE]),
            (0x01, &[0x00], &[0xAA, 0x00, 0x02, 0x01, 0x00, 0xFD]),
            (0x05, &[0x14, 0x00], &[0xAA, 0x00, 0x03, 0x05, 0x14, 0x00, 0xE4]),
            (0x05, &[0x15, 0x00], &[0xAA, 0x00, 0x03, 0x05, 0x15, 0x00, 0xE3]),
            (0x26, &[0x40], &[0xAA, 0x00, 0x02, 0x26, 0x40, 0x98]),
            (0x25, &[], &[0xAA, 0x00, 0x01, 0x25, 0xDA]),
            (0x04, &[0x00], &[0xAA, 0x00, 0x02, 0x04, 0x00, 0xFA]),
            (0x29, &[], &[0xAA, 0x00, 0x01, 0x29, 0xD6]),
            (0x24, &[], &[0xAA, 0x00, 0x01, 0x24, 0xDB]),
            (0x23, &[], &[0xAA, 0x00, 0x01, 0x23, 0xDC]),
            (0x28, &[0x02], &[0xAA, 0x00, 0x02, 0x28, 0x02, 0xD4]),
            (0x7F, &[0x06, 0x07], &[0xAA, 0x00, 0x03, 0x7F, 0x06, 0x07, 0x71]),
            // Received: ack of 0x06, settings event.
            (0x00, &[0x06], &[0xAA, 0x00, 0x02, 0x00, 0x06, 0xF8]),
            (0x2C, &[0x06, 0x07], &[0xAA, 0x00, 0x03, 0x2C, 0x06, 0x07, 0xC4]),
        ];
        for (id, p, want) in cases {
            assert_eq!(enc(id, p), want, "id {id:#04x}");
            let (f, _) = decode_all(want);
            assert_eq!(f, [Frame::new(id, p)]);
        }
        let id = b"\xAA\x00\x09\x06ATS2851\x00\x39";
        assert_eq!(decode_all(id).0, [Frame::new(0x06, b"ATS2851\0")]);
    }

    #[test]
    fn restricted_ids_refused() {
        let mut b = [0u8; 16];
        for id in 0..=255u8 {
            let r = encode(id, &[], &mut b);
            assert_eq!(r.is_err(), is_restricted(id), "id {id:#04x}");
        }
        assert_eq!(encode(0x24, &[], &mut b), Err(EncodeError::Restricted));
        assert_eq!(encode(0x06, &[0; 20], &mut b), Err(EncodeError::TooLong));
    }

    #[test]
    fn resync_cases() {
        let ready = [0xAA, 0x00, 0x01, 0x23, 0xDC];
        // Stray 0x00 at module power-up.
        let mut s = std::vec![0x00];
        s.extend_from_slice(&ready);
        let (f, st) = decode_all(&s);
        assert_eq!(f.len(), 1);
        assert_eq!(st.skipped, 1);
        // An AA with an impossible length right before a frame.
        let mut s = std::vec![0xAA, 0x7F];
        s.extend_from_slice(&ready);
        assert_eq!(decode_all(&s).0.len(), 1);
        // A truncated frame whose tail contains a whole frame: the bad
        // checksum candidate is rescanned and the inner frame recovered.
        let mut s = std::vec![0xAA, 0x00, 0x06, 0x01];
        s.extend_from_slice(&ready);
        s.extend_from_slice(&ready);
        let (f, st) = decode_all(&s);
        assert_eq!(f.len(), 2, "{st:?}");
        // AA AA: the second is the start.
        let mut s = std::vec![0xAA];
        s.extend_from_slice(&ready);
        assert_eq!(decode_all(&s).0.len(), 1);
        // Byte at a time equals all at once.
        let mut d = Decoder::new();
        let mut n = 0;
        for b in [0x55, 0xAA, 0x00, 0x02, 0x00, 0x06, 0xF8, 0xAA, 0x00, 0x01, 0x23, 0xDC] {
            d.push(b, &mut |_| n += 1);
        }
        assert_eq!(n, 2);
    }

    /// Fuzz: random noise with valid frames spliced in. Never panics; every
    /// frame surrounded by noise that contains no AA is found; every
    /// decoded frame has a valid checksum by construction.
    #[test]
    fn fuzz_decoder() {
        let mut x: u64 = 0x1234_5678_9ABC_DEF1;
        let mut rnd = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for round in 0..3000 {
            let mut stream = Vec::new();
            let mut planted = 0;
            for _ in 0..(rnd() % 8) {
                let noise = rnd() % 20;
                for _ in 0..noise {
                    let mut b = rnd() as u8;
                    if round % 2 == 0 && b == SYNC {
                        b = 0x55;
                    }
                    stream.push(b);
                }
                let n = (rnd() % 20) as usize;
                let p: Vec<u8> = (0..n).map(|_| rnd() as u8).collect();
                stream.extend_from_slice(&enc(rnd() as u8, &p));
                planted += 1;
            }
            let mut d = Decoder::new();
            let mut got = 0;
            for chunk in stream.chunks(1 + (rnd() % 7) as usize) {
                d.feed(chunk, |f| {
                    assert!(f.params().len() <= MAX_RX_PARAMS);
                    got += 1;
                });
            }
            if round % 2 == 0 {
                assert!(got >= planted, "round {round}: {got} < {planted}");
            }
        }
        // Pure noise.
        let mut d = Decoder::new();
        for _ in 0..200_000 {
            d.push(rnd() as u8, &mut |f| assert!(f.params().len() <= MAX_RX_PARAMS));
        }
    }
}
