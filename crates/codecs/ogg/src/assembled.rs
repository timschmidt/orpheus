//! Packets reassembled into a caller buffer (Opus, Speex).
//!
//! [`PacketReader`] pulls pages from a [`ByteSource`] and copies the packets
//! of one logical stream into the caller's buffer, across pages and ring
//! wraps; a packet longer than the buffer is truncated and flagged.
//! Restartable: a call that answers `NeedData` keeps what it has copied so
//! far, so nothing it consumed is needed again.
//!
//! Stream selection: until [`PacketReader::select`] is called only the first
//! packets of BOS pages are delivered (the codec picks its stream by the
//! identification header); afterwards only that serial's packets. A BOS page
//! of another serial after the selected stream's EOS page (a chained file)
//! is delivered so the codec can re-select.
//!
//! After a [`crate::Bisect`], [`PacketReader::restart`] resumes reading
//! after the packets completed on the found page, so the next packet starts
//! exactly at that page's granule position.

use orpheus_audio::{ByteSource, View};

use crate::{CHECKED_PAGE, HEADER_MIN, PageHeader, Sync, crc_page_bytes, crc_update, sync_page};

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
pub type Next = crate::Next<Packet>;

#[derive(Debug, Clone, Copy)]
struct Cur {
    hdr: PageHeader,
    lacing: [u8; 255],
    /// Next segment and bytes of it consumed.
    seg: usize,
    seg_done: usize,
    /// Index of the last segment that ends a packet (`segments` if none).
    last_end: usize,
    /// Running CRC (pages larger than [`CHECKED_PAGE`]).
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
        let checked = h.total() <= CHECKED_PAGE;
        let crc = if checked { 0 } else { crc_page_bytes(v.take(h.header_len)) };
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
                    return Next::NeedData(self.skip.min(CHECKED_PAGE as u64) as usize);
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
            let lace = usize::from(cur.lacing[cur.seg]);
            let want = lace - cur.seg_done;
            let k = want.min(src.available());
            if want > 0 && k == 0 {
                if src.at_end() {
                    self.cur = None;
                    return Next::End;
                }
                return Next::NeedData(want.min(CHECKED_PAGE));
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
}
