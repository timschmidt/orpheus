//! Ogg page and packet layer (RFC 3533), shared by the Ogg codecs (Opus,
//! Speex, Vorbis, Ogg FLAC).
//!
//! * [`PageHeader`]: parse and check one page header; [`page_crc`] is the
//!   Ogg CRC-32 (polynomial `0x04C11DB7`, no reflection, initial 0).
//! * [`PacketReader`]: pulls pages from a [`ByteSource`] and reassembles the
//!   packets of one logical stream into a caller buffer. Restartable: a call
//!   that answers `NeedData` keeps what it has copied so far, so nothing it
//!   consumed is needed again. Pages up to [`MAX_PEEK`] bytes are CRC-checked
//!   before any of their bytes are used; larger pages (up to 64 KiB, more
//!   than the stream ring can show at once) are checked as they stream past
//!   and a mismatch is reported after their packets were delivered.
//! * Stream selection: until [`PacketReader::select`] is called only the
//!   first packets of BOS pages are delivered (the codec picks its stream by
//!   the identification header); afterwards only that serial's packets. A
//!   BOS page of another serial after the selected stream's EOS page (a
//!   chained file) is delivered so the codec can re-select.
//! * Seeking: [`PacketReader::seek_step`] bisects over a [`SeekSource`] for
//!   the last page whose granule position is at or before a target and
//!   [`PacketReader::restart`] resumes reading after the packets completed on
//!   that page, so the next packet starts exactly at that granule position.
//!
//! `no_std`, no allocation, no `unsafe`, no floating point.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

use yoton_audio::{ByteSource, MAX_PEEK, SeekSource, SourceError, View};

/// Fixed part of a page header.
pub const HEADER_MIN: usize = 27;
/// The largest possible page (header with 255 lacing values, 255 × 255
/// body bytes).
pub const MAX_PAGE: usize = 27 + 255 + 255 * 255;
pub const FLAG_CONTINUED: u8 = 1;
pub const FLAG_BOS: u8 = 2;
pub const FLAG_EOS: u8 = 4;
/// A page on which no packet ends carries this granule position.
pub const NO_GRANULE: u64 = u64::MAX;

const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u32) << 24;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static CRC: [u32; 256] = crc_table();

/// Continue the Ogg CRC-32 over `data`.
#[must_use]
pub fn crc_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc = (crc << 8) ^ CRC[usize::from((crc >> 24) as u8 ^ b)];
    }
    crc
}

/// CRC of a whole page in `v` (header with its CRC field taken as zero).
#[must_use]
pub fn page_crc(v: &View<'_>, total: usize) -> u32 {
    let page = v.take(total);
    let head = page.take(22);
    let mut crc = crc_update(crc_update(0, head.a), head.b);
    crc = crc_update(crc, &[0; 4]);
    let body = page.skip(26);
    crc_update(crc_update(crc, body.a), body.b)
}

/// A parsed page header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageHeader {
    pub flags: u8,
    /// Raw granule position ([`NO_GRANULE`] when no packet ends here).
    pub granule: u64,
    pub serial: u32,
    pub seq: u32,
    pub crc: u32,
    pub segments: u8,
    /// Header bytes including the lacing values.
    pub header_len: usize,
    pub body_len: usize,
}

/// Outcome of [`PageHeader::parse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parse {
    Page(PageHeader),
    /// The header needs this many bytes in the view.
    Need(usize),
    /// No valid page header at the start of the view.
    NotPage,
}

impl PageHeader {
    /// Parse the header at the start of `v` (the body need not be there).
    #[must_use]
    pub fn parse(v: &View<'_>) -> Parse {
        if v.len() < HEADER_MIN {
            return Parse::Need(HEADER_MIN);
        }
        if !v.starts_with_at(0, b"OggS") || v.byte(4) != 0 || v.byte(5) & !7 != 0 {
            return Parse::NotPage;
        }
        let segments = v.byte(26);
        let header_len = HEADER_MIN + usize::from(segments);
        if v.len() < header_len {
            return Parse::Need(header_len);
        }
        let body_len = (0..usize::from(segments)).map(|i| usize::from(v.byte(HEADER_MIN + i))).sum();
        Parse::Page(PageHeader {
            flags: v.byte(5),
            granule: v.u64_le(6).unwrap_or(NO_GRANULE),
            serial: v.u32_le(14).unwrap_or(0),
            seq: v.u32_le(18).unwrap_or(0),
            crc: v.u32_le(22).unwrap_or(0),
            segments,
            header_len,
            body_len,
        })
    }

    #[must_use]
    pub fn total(&self) -> usize {
        self.header_len + self.body_len
    }
    #[must_use]
    pub fn continued(&self) -> bool {
        self.flags & FLAG_CONTINUED != 0
    }
    #[must_use]
    pub fn bos(&self) -> bool {
        self.flags & FLAG_BOS != 0
    }
    #[must_use]
    pub fn eos(&self) -> bool {
        self.flags & FLAG_EOS != 0
    }
    /// The granule position, unless no packet ends on this page.
    #[must_use]
    pub fn granule_pos(&self) -> Option<u64> {
        (self.granule != NO_GRANULE).then_some(self.granule)
    }
}

/// One reassembled packet (its bytes are in the caller's buffer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet {
    /// Bytes stored in the buffer.
    pub len: usize,
    /// The packet was longer than the buffer; the rest was dropped.
    pub truncated: bool,
    /// Granule position of the page, for the last packet that ends on it.
    pub granule: Option<u64>,
    /// First packet of a beginning-of-stream page.
    pub bos: bool,
    /// Last packet of an end-of-stream page.
    pub eos: bool,
    pub serial: u32,
    /// Data of this stream was lost since the previous packet (a CRC
    /// failure, a gap in the page sequence, junk between pages).
    pub after_gap: bool,
}

/// Answer of [`PacketReader::next_packet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    Packet(Packet),
    /// Call again once `available() >= n` or the source is at its end.
    NeedData(usize),
    /// No further complete page.
    End,
    /// Bytes that are not a valid page were skipped (at least one byte
    /// consumed), or a large page failed its CRC after its packets were
    /// delivered.
    Corrupt,
}

/// Answer of [`PacketReader::seek_step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekStep {
    NeedData(usize),
    /// The last page of the selected stream with a granule position at or
    /// before the target: `(page offset, granule)`; `None` if the target is
    /// before every page's granule position.
    Done(Option<(u64, u64)>),
}

/// Bisection state for [`PacketReader::seek_step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bisect {
    target: u64,
    lo: u64,
    hi: u64,
    best: Option<(u64, u64)>,
    probe: u64,
    phase: Phase,
    steps: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Start,
    Scan,
    Linear,
}

/// Below this span the bisection walks pages linearly.
const LINEAR_SPAN: u64 = 48 * 1024;

impl Bisect {
    /// Search pages starting in `[start, end)` for the target granule.
    #[must_use]
    pub fn new(start: u64, end: u64, target: u64) -> Self {
        Self {
            target,
            lo: start,
            hi: end.max(start),
            best: None,
            probe: start,
            phase: Phase::Start,
            steps: 0,
        }
    }
    #[must_use]
    pub fn target(&self) -> u64 {
        self.target
    }
}

#[derive(Debug, Clone, Copy)]
struct Cur {
    hdr: PageHeader,
    lacing: [u8; 255],
    /// Next segment and bytes of it consumed.
    seg: usize,
    seg_done: usize,
    /// Index of the last segment that ends a packet (`segments` if none).
    last_end: usize,
    /// Running CRC (pages larger than `MAX_PEEK`).
    crc: u32,
    checked: bool,
    /// Packets ended on this page so far.
    ended: usize,
    /// Discard every packet that ends on this page (resuming after a seek).
    skip_completed: bool,
}

/// Packet reassembly over one logical stream. Small (about 300 bytes); kept
/// in the decoder value, not the arena.
#[derive(Debug, Clone)]
pub struct PacketReader {
    selected: Option<u32>,
    eos_seen: bool,
    cur: Option<Cur>,
    /// Bytes still to skip (a page of another stream).
    skip: u64,
    /// Packet in progress.
    len: usize,
    started: bool,
    truncated: bool,
    /// Drop the rest of the packet in progress (its start was lost).
    discard: bool,
    gap: bool,
    next_seq: Option<u32>,
    pending_corrupt: bool,
    skip_completed_next: bool,
}

impl Default for PacketReader {
    fn default() -> Self {
        Self::new()
    }
}

enum Sync {
    Page(PageHeader, bool),
    Need(usize),
    End,
}

/// Find the next page at or after the read position, consuming junk before
/// it. Pages up to `MAX_PEEK` bytes are returned only when complete and
/// with a good CRC. The flag tells whether junk was skipped.
fn sync_page(src: &mut dyn ByteSource) -> Sync {
    let mut junk = false;
    loop {
        let v = View::of(src);
        let end = src.at_end();
        match PageHeader::parse(&v) {
            Parse::Need(n) => {
                if v.len() >= 4 && !v.starts_with_at(0, b"OggS") {
                    // Not a page start: fall through to the search below.
                } else if end {
                    src.consume(v.len());
                    return Sync::End;
                } else {
                    return Sync::Need(n.min(MAX_PEEK));
                }
            }
            Parse::Page(h) => {
                let total = h.total();
                if total > MAX_PEEK {
                    return Sync::Page(h, junk);
                }
                if v.len() < total {
                    if end {
                        // A page cut short by the end of the file.
                        src.consume(v.len());
                        return Sync::End;
                    }
                    return Sync::Need(total);
                }
                if page_crc(&v, total) == h.crc {
                    return Sync::Page(h, junk);
                }
            }
            Parse::NotPage => {}
        }
        // Skip at least one byte, up to the next capture pattern.
        junk = true;
        match v.find(1, b"OggS") {
            Some(k) => src.consume(k),
            None => {
                let k = v.len().saturating_sub(3).max(1).min(v.len());
                src.consume(k);
                if src.available() < HEADER_MIN {
                    if src.at_end() {
                        src.consume(src.available());
                        return Sync::End;
                    }
                    return Sync::Need(HEADER_MIN);
                }
            }
        }
    }
}

impl PacketReader {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            selected: None,
            eos_seen: false,
            cur: None,
            skip: 0,
            len: 0,
            started: false,
            truncated: false,
            discard: false,
            gap: false,
            next_seq: None,
            pending_corrupt: false,
            skip_completed_next: false,
        }
    }

    /// Follow the logical stream `serial` from now on.
    pub fn select(&mut self, serial: u32) {
        if self.selected != Some(serial) {
            self.selected = Some(serial);
            self.eos_seen = false;
            self.next_seq = None;
        }
    }

    #[must_use]
    pub fn selected(&self) -> Option<u32> {
        self.selected
    }

    /// Whether the selected stream's EOS page has been read.
    #[must_use]
    pub fn eos_seen(&self) -> bool {
        self.eos_seen
    }

    /// Forget the page and packet in progress (after the source was moved).
    /// With `skip_completed`, the packets that end on the first page read
    /// next are dropped, so the first packet delivered starts at that page's
    /// granule position.
    pub fn restart(&mut self, skip_completed: bool) {
        self.cur = None;
        self.skip = 0;
        self.reset_packet();
        self.discard = false;
        self.gap = false;
        self.next_seq = None;
        self.eos_seen = false;
        self.pending_corrupt = false;
        self.skip_completed_next = skip_completed;
    }

    fn reset_packet(&mut self) {
        self.len = 0;
        self.started = false;
        self.truncated = false;
    }

    fn relevant(&self, h: &PageHeader) -> bool {
        match self.selected {
            None => h.bos(),
            Some(s) if s == h.serial => true,
            Some(_) => h.bos() && self.eos_seen,
        }
    }

    /// Start reading the page whose header is at the read position.
    fn begin_page(&mut self, src: &mut dyn ByteSource, h: PageHeader) {
        let v = View::of(src);
        let mut lacing = [0u8; 255];
        let n = usize::from(h.segments);
        v.copy_to(HEADER_MIN, &mut lacing[..n]);
        let last_end = (0..n).rev().find(|&i| lacing[i] < 255).unwrap_or(n);
        let checked = h.total() <= MAX_PEEK;
        let crc = if checked {
            0
        } else {
            let head = v.take(h.header_len);
            let mut c = crc_update(crc_update(0, head.take(22).a), head.take(22).b);
            c = crc_update(c, &[0; 4]);
            let rest = head.skip(26);
            crc_update(crc_update(c, rest.a), rest.b)
        };
        src.consume(h.header_len);
        let own = self.selected == Some(h.serial);
        if own {
            if let Some(s) = self.next_seq
                && s != h.seq
            {
                self.gap = true;
                if self.started {
                    self.discard = true;
                }
            }
            self.next_seq = Some(h.seq.wrapping_add(1));
        }
        // A continuation with nothing in progress: drop its first packet.
        if h.continued() && !self.started {
            self.discard = true;
        }
        // A fresh packet where one was in progress: the old one is lost.
        if !h.continued() && self.started {
            self.reset_packet();
            self.discard = false;
            self.gap = true;
        }
        if !own {
            // A BOS page of a new stream: nothing in progress belongs to it.
            self.reset_packet();
            self.discard = false;
        }
        let skip_completed = core::mem::take(&mut self.skip_completed_next);
        self.cur = Some(Cur {
            hdr: h,
            lacing,
            seg: 0,
            seg_done: 0,
            last_end,
            crc,
            checked,
            ended: 0,
            skip_completed,
        });
    }

    /// Reassemble the next packet of the selected stream into `buf`.
    pub fn next_packet(&mut self, src: &mut dyn ByteSource, buf: &mut [u8]) -> Next {
        loop {
            if self.skip > 0 {
                let k = self.skip.min(src.available() as u64);
                src.consume(k as usize);
                self.skip -= k;
                if self.skip > 0 {
                    if src.at_end() {
                        self.skip = 0;
                        return Next::End;
                    }
                    return Next::NeedData(self.skip.min(MAX_PEEK as u64) as usize);
                }
            }
            let Some(cur) = self.cur.as_mut() else {
                if core::mem::take(&mut self.pending_corrupt) {
                    return Next::Corrupt;
                }
                match sync_page(src) {
                    Sync::Need(n) => return Next::NeedData(n),
                    Sync::End => return Next::End,
                    Sync::Page(h, junk) => {
                        if junk {
                            self.gap = true;
                            if self.started {
                                self.discard = true;
                            }
                        }
                        if self.relevant(&h) {
                            self.begin_page(src, h);
                        } else {
                            self.skip = h.total() as u64;
                        }
                        if junk {
                            return Next::Corrupt;
                        }
                        continue;
                    }
                }
            };
            let n = usize::from(cur.hdr.segments);
            if cur.seg >= n {
                let h = cur.hdr;
                let bad = !cur.checked && cur.crc != h.crc;
                self.cur = None;
                if self.selected == Some(h.serial) && h.eos() {
                    self.eos_seen = true;
                }
                if bad {
                    self.gap = true;
                    return Next::Corrupt;
                }
                continue;
            }
            // While no stream is selected only the first packet of a BOS page
            // is of interest.
            let lace = usize::from(cur.lacing[cur.seg]);
            let want = lace - cur.seg_done;
            let k = want.min(src.available());
            if want > 0 && k == 0 {
                if src.at_end() {
                    self.cur = None;
                    return Next::End;
                }
                return Next::NeedData(want.min(MAX_PEEK));
            }
            let dropping = self.discard || cur.skip_completed && cur.seg <= cur.last_end;
            if let Some(v) = View::peek(src, k) {
                if !cur.checked {
                    cur.crc = crc_update(crc_update(cur.crc, v.a), v.b);
                }
                if !dropping {
                    let room = buf.len().saturating_sub(self.len);
                    let c = k.min(room);
                    if c > 0
                        && let Some(dst) = buf.get_mut(self.len..self.len + c)
                    {
                        v.copy_to(0, dst);
                    }
                    self.len += c;
                    if c < k {
                        self.truncated = true;
                    }
                }
            }
            src.consume(k);
            if !dropping {
                self.started = true;
            }
            cur.seg_done += k;
            if cur.seg_done < lace {
                continue;
            }
            let seg = cur.seg;
            cur.seg += 1;
            cur.seg_done = 0;
            if lace == 255 {
                continue;
            }
            // A packet ends with this segment.
            cur.ended += 1;
            let first = cur.ended == 1;
            let h = cur.hdr;
            let last = seg == cur.last_end;
            if dropping {
                self.discard = false;
                self.reset_packet();
                continue;
            }
            let p = Packet {
                len: self.len,
                truncated: self.truncated,
                granule: if last { h.granule_pos() } else { None },
                bos: h.bos() && first,
                eos: h.eos() && last,
                serial: h.serial,
                after_gap: core::mem::take(&mut self.gap),
            };
            let rest: usize = cur.lacing[cur.seg..n].iter().map(|&l| usize::from(l)).sum::<usize>();
            self.reset_packet();
            if self.selected.is_none() || self.selected != Some(h.serial) {
                // Identification packets only: skip the rest of this page.
                self.cur = None;
                self.skip = rest as u64;
            }
            return Next::Packet(p);
        }
    }

    /// One step of the granule bisection. Call again with the same `b`
    /// after `NeedData` once the source has the bytes (or is at its end).
    /// Afterwards position the source with `seek` + [`Self::restart`].
    ///
    /// # Errors
    /// The source failed to seek.
    pub fn seek_step(&mut self, src: &mut dyn SeekSource, b: &mut Bisect) -> Result<SeekStep, SourceError> {
        let Some(serial) = self.selected else { return Ok(SeekStep::Done(None)) };
        loop {
            if b.phase == Phase::Start {
                b.steps += 1;
                if b.hi <= b.lo || b.hi - b.lo <= LINEAR_SPAN || b.steps > 64 {
                    b.phase = Phase::Linear;
                    b.probe = b.lo;
                } else {
                    b.phase = Phase::Scan;
                    b.probe = b.lo + (b.hi - b.lo) / 2;
                }
                src.seek(b.probe)?;
            }
            let (h, pos) = match sync_page(src) {
                Sync::Need(n) => return Ok(SeekStep::NeedData(n)),
                Sync::End => {
                    if b.phase == Phase::Linear {
                        return Ok(SeekStep::Done(b.best));
                    }
                    b.hi = b.probe;
                    b.phase = Phase::Start;
                    continue;
                }
                Sync::Page(h, _) => (h, src.position()),
            };
            if pos >= b.hi {
                if b.phase == Phase::Linear {
                    return Ok(SeekStep::Done(b.best));
                }
                b.hi = b.probe;
                b.phase = Phase::Start;
                continue;
            }
            if h.serial == serial
                && let Some(g) = h.granule_pos()
            {
                if g <= b.target {
                    b.best = Some((pos, g));
                    if b.phase == Phase::Scan {
                        b.lo = pos + h.total() as u64;
                        b.phase = Phase::Start;
                        continue;
                    }
                } else {
                    if b.phase == Phase::Linear {
                        return Ok(SeekStep::Done(b.best));
                    }
                    b.hi = b.probe;
                    b.phase = Phase::Start;
                    continue;
                }
            }
            // Step over this page.
            let total = h.total();
            if src.available() >= total {
                src.consume(total);
            } else {
                let to = pos + total as u64;
                if src.len().is_some_and(|l| to > l) {
                    if b.phase == Phase::Linear {
                        return Ok(SeekStep::Done(b.best));
                    }
                    b.hi = b.probe;
                    b.phase = Phase::Start;
                    continue;
                }
                src.seek(to)?;
            }
        }
    }
}

#[cfg(test)]
mod tests;
