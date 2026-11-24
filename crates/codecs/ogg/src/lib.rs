//! Ogg (RFC 3533) over the stream ring: packets are returned as up to
//! [`MAX_RANGES`] byte ranges of the ring view (one per page they span) —
//! nothing is copied.
//!
//! Pages may be larger than the ring (ffmpeg writes one-second pages, 40 KB
//! and more at high bitrates): the reader keeps the current page's header
//! (lacing table included) and consumes its body packet by packet, so only
//! a packet, not a page, has to fit in the ring. A page's CRC is checked
//! when the whole page is buffered when first seen (always, for pages up to
//! [`yoton_audio::MAX_PEEK`] bytes); larger pages are taken unchecked.
//!
//! One logical stream is followed (the first one seen); pages of other
//! streams are skipped; a new stream after ours (chaining) ends it.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

use yoton_audio::{ByteSource, MAX_PEEK, View};

/// Pages one packet may span.
pub const MAX_RANGES: usize = 32;
/// Granule position "none" (−1).
pub const NO_GRANULE: u64 = u64::MAX;

const fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut r = (i as u32) << 24;
        let mut k = 0;
        while k < 8 {
            r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04C1_1DB7 } else { r << 1 };
            k += 1;
        }
        t[i] = r;
        i += 1;
    }
    t
}

static CRC: [u32; 256] = crc_table();

fn crc_update(c: u32, b: u8) -> u32 {
    (c << 8) ^ CRC[((c >> 24) as u8 ^ b) as usize]
}

/// A page header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    /// Offset of `OggS` in the view it was found in.
    pub at: usize,
    pub nseg: usize,
    pub lacing: [u8; 255],
    pub body_len: usize,
    pub flags: u8,
    pub granule: u64,
    pub serial: u32,
    pub seq: u32,
}

impl Default for Page {
    fn default() -> Self {
        Self {
            at: 0,
            nseg: 0,
            lacing: [0; 255],
            body_len: 0,
            flags: 0,
            granule: NO_GRANULE,
            serial: 0,
            seq: 0,
        }
    }
}

impl Page {
    #[must_use]
    pub fn header_len(&self) -> usize {
        27 + self.nseg
    }
    #[must_use]
    pub fn len(&self) -> usize {
        self.header_len() + self.body_len
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.body_len == 0
    }
    #[must_use]
    pub fn continued(&self) -> bool {
        self.flags & 1 != 0
    }
    #[must_use]
    pub fn bos(&self) -> bool {
        self.flags & 2 != 0
    }
    #[must_use]
    pub fn eos(&self) -> bool {
        self.flags & 4 != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageErr {
    /// This many bytes from the view start are needed.
    Need(usize),
    /// No valid page here.
    Bad,
}

/// The page header at `at`; the CRC is checked when the whole page is in
/// the view. With `whole`, a page not entirely buffered is `Need` unless it
/// is larger than [`MAX_PEEK`] or the stream ends (`at_end`).
pub fn page_at(v: &View<'_>, at: usize) -> Result<Page, PageErr> {
    header(v, at, true, false)
}

fn header(v: &View<'_>, at: usize, whole: bool, at_end: bool) -> Result<Page, PageErr> {
    if v.len() < at + 27 {
        return Err(PageErr::Need(at + 27));
    }
    if !v.starts_with_at(at, b"OggS") || v.byte(at + 4) != 0 {
        return Err(PageErr::Bad);
    }
    let nseg = usize::from(v.byte(at + 26));
    if v.len() < at + 27 + nseg {
        return Err(PageErr::Need(at + 27 + nseg));
    }
    let mut p = Page {
        at,
        nseg,
        flags: v.byte(at + 5),
        granule: v.u64_le(at + 6).unwrap_or(NO_GRANULE),
        serial: v.u32_le(at + 14).unwrap_or(0),
        seq: v.u32_le(at + 18).unwrap_or(0),
        ..Page::default()
    };
    v.copy_to(at + 27, &mut p.lacing[..nseg]);
    p.body_len = p.lacing[..nseg].iter().map(|&l| usize::from(l)).sum();
    let len = p.len();
    if v.len() >= at + len {
        let r = v.range(at, len).ok_or(PageErr::Bad)?;
        let mut c = 0u32;
        for (i, b) in r.a.iter().chain(r.b).enumerate() {
            c = crc_update(c, if (22..26).contains(&i) { 0 } else { *b });
        }
        if Some(c) != v.u32_le(at + 22) {
            return Err(PageErr::Bad);
        }
    } else if whole && len <= MAX_PEEK && !at_end {
        return Err(PageErr::Need(at + len));
    }
    Ok(p)
}

/// The next page at or after `from`: checked if it is buffered whole,
/// else taken from its header (pages larger than the ring, or the end).
pub fn find_page(v: &View<'_>, from: usize, at_end: bool) -> Result<Page, PageErr> {
    let mut i = from;
    while let Some(off) = v.find(i, b"OggS") {
        match header(v, off, true, at_end) {
            Ok(p) => return Ok(p),
            Err(PageErr::Need(n)) => return Err(PageErr::Need(n)),
            Err(PageErr::Bad) => i = off + 1,
        }
    }
    Err(PageErr::Need(v.len() + 1))
}

/// One packet: its bytes as ranges of the view, and where it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet {
    pub ranges: [(u32, u32); MAX_RANGES],
    pub n: usize,
    pub len: usize,
    /// The granule position of the page it ends on, when it is the last
    /// packet completing there (else [`NO_GRANULE`]).
    pub granule: u64,
    /// It is the last packet of the stream's last page.
    pub eos: bool,
    /// View offset of the header of the page it ends on, if not the
    /// current page.
    end_hdr: Option<usize>,
    end_seg: usize,
    end_left: usize,
    /// View bytes up to the end of its data.
    consumed: usize,
}

impl Default for Packet {
    fn default() -> Self {
        Self {
            ranges: [(0, 0); MAX_RANGES],
            n: 0,
            len: 0,
            granule: NO_GRANULE,
            eos: false,
            end_hdr: None,
            end_seg: 0,
            end_left: 0,
            consumed: 0,
        }
    }
}

impl Packet {
    /// Byte `i` of the packet (0 beyond the end).
    #[must_use]
    pub fn byte(&self, v: &View<'_>, mut i: usize) -> u8 {
        for &(at, len) in &self.ranges[..self.n] {
            if i < len as usize {
                return v.byte(at as usize + i);
            }
            i -= len as usize;
        }
        0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // no heap: the packet travels by value
pub enum Next {
    Packet(Packet),
    /// Call again with this many bytes buffered.
    Need(usize),
    /// The logical stream ended.
    End,
    /// Damaged data was skipped (something was consumed).
    Corrupt,
}

/// The page being read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cur {
    page: Page,
    seg: usize,
    /// Body bytes of the page not consumed yet.
    left: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Reader {
    serial: Option<u32>,
    cur: Option<Cur>,
    /// Bytes still to drop (a foreign page, a packet tail) before reading on.
    skip: usize,
    /// Skip a continued packet's tail (after a seek or damage).
    fresh: bool,
    ended: bool,
}

#[allow(clippy::large_enum_variant)]
enum Asm {
    Packet(Packet),
    Need(usize),
    /// Drop `consume` bytes and resynchronise.
    Fail {
        consume: usize,
        end: bool,
    },
    /// The packet does not fit in the ring.
    TooLarge,
}

/// Assemble the packet starting at view offset `off` in page `c`.
fn assemble(v: &View<'_>, c: Cur, mut off: usize, serial: Option<u32>, at_end: bool) -> Asm {
    let mut pk = Packet::default();
    let (mut pg, mut s, mut left) = (c.page, c.seg, c.left);
    let mut end_hdr = None;
    loop {
        let mut len = 0usize;
        let mut done = false;
        while s < pg.nseg {
            let l = usize::from(pg.lacing[s]);
            len += l;
            s += 1;
            if l < 255 {
                done = true;
                break;
            }
        }
        if len > left {
            return Asm::Fail { consume: off.max(1), end: false };
        }
        if len > 0 {
            if pk.n == MAX_RANGES {
                return Asm::Fail { consume: off.max(1), end: false };
            }
            pk.ranges[pk.n] = (off as u32, len as u32);
            pk.n += 1;
            pk.len += len;
        }
        off += len;
        left -= len;
        if off > v.len() {
            return if off > MAX_PEEK {
                Asm::TooLarge
            } else if at_end {
                Asm::Fail { consume: v.len(), end: true }
            } else {
                Asm::Need(off)
            };
        }
        if done {
            if pg.lacing[s..pg.nseg].iter().all(|&l| l == 255) {
                pk.granule = pg.granule;
                pk.eos = pg.eos();
            }
            pk.end_hdr = end_hdr;
            pk.end_seg = s;
            pk.end_left = left;
            pk.consumed = off;
            return Asm::Packet(pk);
        }
        // Continued on the next page of this stream (the rest of this
        // body, `left`, is empty for a well-formed page).
        let mut at = off + left;
        let np = loop {
            match header(v, at, false, at_end) {
                Ok(p) if Some(p.serial) == serial => break p,
                Ok(p) => at += p.len(),
                Err(PageErr::Need(n)) if n <= MAX_PEEK && !at_end => return Asm::Need(n),
                Err(PageErr::Need(_)) if !at_end => return Asm::TooLarge,
                Err(PageErr::Need(_)) => return Asm::Fail { consume: v.len().min(at).max(1), end: true },
                Err(PageErr::Bad) => return Asm::Fail { consume: at.max(1), end: false },
            }
        };
        if !np.continued() || pg.eos() {
            return Asm::Fail { consume: at, end: false };
        }
        end_hdr = Some(at);
        off = at + np.header_len();
        left = np.body_len;
        pg = np;
        s = 0;
    }
}

fn need(n: usize, src: &dyn ByteSource) -> Next {
    if src.at_end() { Next::End } else { Next::Need(n.min(MAX_PEEK)) }
}

impl Reader {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn serial(&self) -> Option<u32> {
        self.serial
    }

    /// The source was repositioned (anywhere, not necessarily on a page):
    /// resynchronise and drop the tail of any packet cut by the jump.
    pub fn reset(&mut self) {
        self.cur = None;
        self.skip = 0;
        self.fresh = true;
        self.ended = false;
    }

    fn resync(&mut self, src: &mut dyn ByteSource) -> Next {
        let v = View::of(src);
        let to = v.find(1, b"OggS").unwrap_or(v.len().saturating_sub(3).max(1));
        src.consume(to);
        self.reset();
        Next::Corrupt
    }

    /// The next packet (not consumed: call [`Reader::commit`] after using
    /// it). Junk, exhausted pages and other streams' pages are consumed.
    pub fn next(&mut self, src: &mut dyn ByteSource) -> Next {
        if self.ended {
            return Next::End;
        }
        loop {
            if self.skip > 0 {
                let k = self.skip.min(src.available());
                src.consume(k);
                self.skip -= k;
                if self.skip > 0 {
                    return need(self.skip.min(4096).max(src.available() + 1), src);
                }
            }
            let Some(c) = self.cur else {
                let v = View::of(src);
                let page = match header(&v, 0, true, src.at_end()) {
                    Ok(p) => p,
                    Err(PageErr::Need(n)) => {
                        if src.at_end() {
                            let a = src.available();
                            src.consume(a);
                            self.ended = true;
                            return Next::End;
                        }
                        return Next::Need(n);
                    }
                    Err(PageErr::Bad) => return self.resync(src),
                };
                match self.serial {
                    None => self.serial = Some(page.serial),
                    Some(s) if s != page.serial => {
                        if page.bos() {
                            // A chained stream: ours is over.
                            self.ended = true;
                            return Next::End;
                        }
                        src.consume(page.header_len());
                        self.skip = page.body_len;
                        continue;
                    }
                    Some(_) => {}
                }
                src.consume(page.header_len());
                let mut cur = Cur { page, seg: 0, left: page.body_len };
                if self.fresh && page.continued() {
                    // Drop the tail of a packet whose start was lost; it may
                    // run over several pages.
                    let mut ended = false;
                    let mut n = 0;
                    while cur.seg < page.nseg {
                        let l = usize::from(page.lacing[cur.seg]);
                        n += l;
                        cur.seg += 1;
                        if l < 255 {
                            ended = true;
                            break;
                        }
                    }
                    cur.left -= n;
                    self.skip = n;
                    self.fresh = !ended;
                } else {
                    self.fresh = false;
                }
                self.cur = Some(cur);
                continue;
            };
            if c.seg >= c.page.nseg {
                // Page done: drop what is left of its body, go on.
                self.skip = c.left;
                self.cur = None;
                if c.page.eos() {
                    self.ended = true;
                    let k = self.skip.min(src.available());
                    src.consume(k);
                    return Next::End;
                }
                continue;
            }
            let v = View::of(src);
            match assemble(&v, c, 0, self.serial, src.at_end()) {
                Asm::Packet(p) => return Next::Packet(p),
                Asm::Need(n) => return Next::Need(n),
                Asm::TooLarge => {
                    // Skip the packet as it streams past: its part in this
                    // page, then (fresh) its continuation on later pages.
                    let mut c = c;
                    let mut n = 0;
                    let mut ended = false;
                    while c.seg < c.page.nseg {
                        let l = usize::from(c.page.lacing[c.seg]);
                        n += l;
                        c.seg += 1;
                        if l < 255 {
                            ended = true;
                            break;
                        }
                    }
                    c.left -= n.min(c.left);
                    self.skip = n;
                    self.cur = Some(c);
                    self.fresh = !ended;
                    return Next::Corrupt;
                }
                Asm::Fail { consume, end } => {
                    src.consume(consume);
                    self.reset();
                    if end {
                        self.ended = true;
                        return Next::End;
                    }
                    return Next::Corrupt;
                }
            }
        }
    }

    /// Consume the packet's bytes (and the pages it finished).
    pub fn commit(&mut self, src: &mut dyn ByteSource, p: &Packet) {
        match p.end_hdr {
            Some(at) => {
                let v = View::of(src);
                if let Ok(page) = header(&v, at, false, true) {
                    self.cur = Some(Cur { page, seg: p.end_seg, left: p.end_left });
                } else {
                    self.cur = None;
                }
            }
            None => {
                if let Some(c) = &mut self.cur {
                    c.seg = p.end_seg;
                    c.left = p.end_left;
                }
            }
        }
        src.consume(p.consumed);
    }
}

/// Read the first `out.len()` packets of the stream starting at the first
/// page of `v`, without consuming anything (header inspection).
pub fn peek_packets(v: &View<'_>, at_end: bool, out: &mut [Packet]) -> Result<(), PageErr> {
    let first = header(v, 0, false, at_end)?;
    let serial = Some(first.serial);
    let mut cur = Cur { page: first, seg: 0, left: first.body_len };
    let mut off = first.header_len();
    for slot in out.iter_mut() {
        while cur.seg >= cur.page.nseg {
            let at = off + cur.left;
            let page = header(v, at, false, at_end)?;
            off = at + page.header_len();
            cur = if Some(page.serial) == serial {
                Cur { page, seg: 0, left: page.body_len }
            } else {
                Cur { page, seg: page.nseg, left: page.body_len }
            };
        }
        match assemble(v, cur, off, serial, at_end) {
            Asm::Packet(p) => {
                *slot = p;
                if let Some(at) = p.end_hdr {
                    let page = header(v, at, false, at_end)?;
                    cur = Cur { page, seg: p.end_seg, left: p.end_left };
                } else {
                    cur.seg = p.end_seg;
                    cur.left = p.end_left;
                }
                off = p.consumed;
            }
            Asm::Need(n) => return Err(PageErr::Need(n)),
            Asm::Fail { .. } | Asm::TooLarge => return Err(PageErr::Bad),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
