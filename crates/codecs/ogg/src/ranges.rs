//! Zero-copy packets (Vorbis): a packet is returned as up to
//! [`MAX_RANGES`] byte ranges of the ring view (one per page it spans) —
//! nothing is copied.
//!
//! Pages may be larger than the ring (ffmpeg writes one-second pages, 40 KB
//! and more at high bitrates): the reader keeps the current page's header
//! (lacing table included) and consumes its body packet by packet, so only
//! a packet, not a page, has to fit in the ring. A page's CRC is checked
//! when the whole page is buffered when first seen (always, for pages up to
//! [`CHECKED_PAGE`] bytes); larger pages are taken unchecked. A packet too
//! large for the ring is skipped as it streams past (`Corrupt`).
//!
//! One logical stream is followed (the first one seen); pages of other
//! streams are skipped; a new stream after ours (chaining) ends it.

use orpheus_audio::{ByteSource, MAX_PEEK, View};

use crate::{CHECKED_PAGE, HEADER_MIN, NO_GRANULE, PageHeader, Parse, page_crc};

/// Pages one packet may span.
pub const MAX_RANGES: usize = 32;

/// A page header in a view, with its lacing table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Page {
    hdr: PageHeader,
    lacing: [u8; 255],
}

impl Page {
    fn nseg(&self) -> usize {
        usize::from(self.hdr.segments)
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
/// is larger than [`CHECKED_PAGE`] or the stream ends (`at_end`).
fn header(v: &View<'_>, at: usize, whole: bool, at_end: bool) -> Result<Page, PageErr> {
    let w = v.skip(at);
    let hdr = match PageHeader::parse(&w) {
        Parse::Page(h) => h,
        Parse::Need(n) => return Err(PageErr::Need(at + n)),
        Parse::NotPage => return Err(PageErr::Bad),
    };
    let mut p = Page { hdr, lacing: [0; 255] };
    w.copy_to(HEADER_MIN, &mut p.lacing[..usize::from(hdr.segments)]);
    let len = hdr.total();
    if w.len() >= len {
        if page_crc(&w, len) != hdr.crc {
            return Err(PageErr::Bad);
        }
    } else if whole && len <= CHECKED_PAGE && !at_end {
        return Err(PageErr::Need(at + len));
    }
    Ok(p)
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

/// Answer of [`Reader::next`].
pub type Next = crate::Next<Packet>;

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
        while s < pg.nseg() {
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
            if pg.lacing[s..pg.nseg()].iter().all(|&l| l == 255) {
                pk.granule = pg.hdr.granule;
                pk.eos = pg.hdr.eos();
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
                Ok(p) if Some(p.hdr.serial) == serial => break p,
                Ok(p) => at += p.hdr.total(),
                Err(PageErr::Need(n)) if n <= MAX_PEEK && !at_end => return Asm::Need(n),
                Err(PageErr::Need(_)) if !at_end => return Asm::TooLarge,
                Err(PageErr::Need(_)) => return Asm::Fail { consume: v.len().min(at).max(1), end: true },
                Err(PageErr::Bad) => return Asm::Fail { consume: at.max(1), end: false },
            }
        };
        if !np.hdr.continued() || pg.hdr.eos() {
            return Asm::Fail { consume: at, end: false };
        }
        end_hdr = Some(at);
        off = at + np.hdr.header_len;
        left = np.hdr.body_len;
        pg = np;
        s = 0;
    }
}

fn need(n: usize, src: &dyn ByteSource) -> Next {
    if src.at_end() { Next::End } else { Next::NeedData(n.min(MAX_PEEK)) }
}

/// Drop the segments of the packet starting at `c.seg` that lie in this
/// page; how many bytes, and whether the packet ends here.
fn drop_segments(c: &mut Cur) -> (usize, bool) {
    let mut n = 0;
    while c.seg < c.page.nseg() {
        let l = usize::from(c.page.lacing[c.seg]);
        n += l;
        c.seg += 1;
        if l < 255 {
            return (n, true);
        }
    }
    (n, false)
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
                        return Next::NeedData(n);
                    }
                    Err(PageErr::Bad) => return self.resync(src),
                };
                let h = page.hdr;
                match self.serial {
                    None => self.serial = Some(h.serial),
                    Some(s) if s != h.serial => {
                        if h.bos() {
                            // A chained stream: ours is over.
                            self.ended = true;
                            return Next::End;
                        }
                        src.consume(h.header_len);
                        self.skip = h.body_len;
                        continue;
                    }
                    Some(_) => {}
                }
                src.consume(h.header_len);
                let mut cur = Cur { page, seg: 0, left: h.body_len };
                if self.fresh && h.continued() {
                    // Drop the tail of a packet whose start was lost; it may
                    // run over several pages.
                    let (n, ended) = drop_segments(&mut cur);
                    cur.left -= n;
                    self.skip = n;
                    self.fresh = !ended;
                } else {
                    self.fresh = false;
                }
                self.cur = Some(cur);
                continue;
            };
            if c.seg >= c.page.nseg() {
                // Page done: drop what is left of its body, go on.
                self.skip = c.left;
                self.cur = None;
                if c.page.hdr.eos() {
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
                Asm::Need(n) => return Next::NeedData(n),
                Asm::TooLarge => {
                    // Skip the packet as it streams past: its part in this
                    // page, then (fresh) its continuation on later pages.
                    let mut c = c;
                    let (n, ended) = drop_segments(&mut c);
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
///
/// # Errors
/// `Need(n)`: `n` bytes of view are needed; `Bad`: no such packets (damage,
/// or a packet that does not fit in the ring).
pub fn peek_packets(v: &View<'_>, at_end: bool, out: &mut [Packet]) -> Result<(), PageErr> {
    let first = header(v, 0, false, at_end)?;
    let serial = Some(first.hdr.serial);
    let mut cur = Cur { page: first, seg: 0, left: first.hdr.body_len };
    let mut off = first.hdr.header_len;
    for slot in out.iter_mut() {
        while cur.seg >= cur.page.nseg() {
            let at = off + cur.left;
            let page = header(v, at, false, at_end)?;
            off = at + page.hdr.header_len;
            cur = if Some(page.hdr.serial) == serial {
                Cur { page, seg: 0, left: page.hdr.body_len }
            } else {
                Cur { page, seg: page.nseg(), left: page.hdr.body_len }
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
