//! Sample playback: a block source (the card later), a bounded page cache,
//! and the players behind sample patches and kit sample hits.
//!
//! # Data path
//!
//! ```text
//! SampleSource ──read_block──► Reader ──wav::parse / wav::read_frames──► SampleCache pages (mono i16)
//!   (card file, slice)          (one 512-byte block buffered)              ▲ service(): head pages pinned at load,
//!                                                                         │ the rest streamed ahead of the voices
//!                                                     Engine::render_*_with(&cache) ─► SamplePlayer (pitch, cubic interpolation)
//! ```
//!
//! * [`SampleSource`] reads 512-byte blocks of numbered files: the
//!   player's card layer implements it in SYN-3 (a block read per call; no
//!   allocation); [`SliceSource`] serves byte slices (tests, the
//!   simulator, samples in flash).
//! * [`SampleCache`] owns no memory: it borrows a `&mut [i16]` (on the
//!   player, memory the synth may use while it plays, such as the idle
//!   stream ring) and cuts it into pages of [`PAGE_FRAMES`] mono frames
//!   (at most [`MAX_PAGES`]). [`SampleCache::load`] parses a WAV file into
//!   one of [`MAX_SAMPLES`] slots and loads its first pages *pinned* (the
//!   head: a voice can start on its sample without waiting for the card);
//!   [`SampleCache::service`], called between render blocks with the
//!   engine's play heads ([`crate::Engine::sample_wants`]), loads the pages
//!   the voices will reach within [`AHEAD_FRAMES`] output frames, evicting
//!   the least recently wanted unpinned pages. A whole sample can be pinned
//!   (`head_pages = u32::MAX`), which makes playback independent of the
//!   service and of block sizes (the golden renders do that).
//! * A page a voice needs but the cache does not hold plays as silence for
//!   those samples and counts a miss ([`crate::Engine::sample_misses`]);
//!   time keeps running, so a late page never shifts the music.
//! * Files are converted on loading: 8/16/24/32-bit PCM or float to 16-bit,
//!   stereo to mono (the voice's pan places it). Mono halves the memory, and
//!   the engine's voices are mono.
//!
//! # Players
//!
//! [`SamplePlayer`] keeps a 32.32 fixed-point position and increment
//! (exact, drift-free; the increment is set at control rate from the pitch),
//! interpolates with the 4-point, 3rd-order Hermite (Catmull-Rom)
//! polynomial (O. Niemitalo, "Polynomial Interpolators for High-Quality
//! Resampling of Oversampled Audio", 2001), plays one-shot to the end or
//! loops forward between loop points (reading across the seam from the
//! loop start). At the sample's own rate and pitch (increment exactly 1)
//! the output is the stored samples bit for bit. Pitching up does not
//! band-limit (increments are capped at 8, three octaves up).

use crate::math::{clampf, decay_coef, exp2, flush};
use crate::wav::{self, WavError, WavInfo};

/// Bytes per source block.
pub const BLOCK: usize = 512;
/// Mono frames per cache page (512 bytes).
pub const PAGE_FRAMES: u32 = 256;
/// Pages a cache can index.
pub const MAX_PAGES: usize = 256;
/// Sample slots.
pub const MAX_SAMPLES: usize = 32;
/// How far ahead of a play head [`SampleCache::service`] loads, in output
/// frames (21.3 ms at 48 kHz: a render block or two; service runs between blocks).
pub const AHEAD_FRAMES: u32 = 1024;
/// Highest playback increment (three octaves up).
pub const MAX_INC: f32 = 8.0;

/// A source failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceError {
    /// No such file.
    NotFound,
    /// The read failed.
    Io,
}

/// Numbered files read in 512-byte blocks (a card directory of samples,
/// byte slices). Implementations must not block for long: the synth calls
/// it between render blocks.
pub trait SampleSource {
    /// Length of `file`, bytes.
    ///
    /// # Errors
    ///
    /// [`SourceError`] when the file does not exist or cannot be read.
    fn file_len(&mut self, file: u16) -> Result<u32, SourceError>;

    /// Read block `block` (bytes `512·block…`) of `file` into `buf`;
    /// returns the bytes read (fewer only at the end of the file).
    ///
    /// # Errors
    ///
    /// [`SourceError`] when the read fails.
    fn read_block(&mut self, file: u16, block: u32, buf: &mut [u8; BLOCK]) -> Result<usize, SourceError>;
}

/// Byte slices as numbered files (file `i` is `files[i]`).
#[derive(Clone, Copy, Debug)]
pub struct SliceSource<'a> {
    files: &'a [&'a [u8]],
    /// Block reads served (a test hook).
    pub reads: u32,
}

impl<'a> SliceSource<'a> {
    /// A source over `files`.
    #[must_use]
    pub const fn new(files: &'a [&'a [u8]]) -> Self {
        Self { files, reads: 0 }
    }
}

impl SampleSource for SliceSource<'_> {
    fn file_len(&mut self, file: u16) -> Result<u32, SourceError> {
        let f = self.files.get(usize::from(file)).ok_or(SourceError::NotFound)?;
        u32::try_from(f.len()).map_err(|_| SourceError::Io)
    }

    fn read_block(&mut self, file: u16, block: u32, buf: &mut [u8; BLOCK]) -> Result<usize, SourceError> {
        let f = self.files.get(usize::from(file)).ok_or(SourceError::NotFound)?;
        self.reads += 1;
        let start = (block as usize).saturating_mul(BLOCK).min(f.len());
        let end = (start + BLOCK).min(f.len());
        buf[..end - start].copy_from_slice(&f[start..end]);
        Ok(end - start)
    }
}

/// Byte-range reads over a [`SampleSource`] file with one block buffered.
pub struct Reader<'s, S: SampleSource + ?Sized> {
    src: &'s mut S,
    file: u16,
    len: u32,
    buf: [u8; BLOCK],
    block: u32,
    valid: usize,
}

impl<S: SampleSource + ?Sized> core::fmt::Debug for Reader<'_, S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Reader").field("file", &self.file).field("len", &self.len).finish_non_exhaustive()
    }
}

impl<'s, S: SampleSource + ?Sized> Reader<'s, S> {
    /// A reader of `file`.
    ///
    /// # Errors
    ///
    /// [`SourceError`] from [`SampleSource::file_len`].
    pub fn new(src: &'s mut S, file: u16) -> Result<Self, SourceError> {
        let len = src.file_len(file)?;
        Ok(Self { src, file, len, buf: [0; BLOCK], block: u32::MAX, valid: 0 })
    }

    /// File length, bytes.
    #[must_use]
    pub fn len(&self) -> u32 {
        self.len
    }

    /// Whether the file is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Read up to `out.len()` bytes at `off`; returns how many (fewer at the
    /// end of the file).
    ///
    /// # Errors
    ///
    /// [`SourceError`] when a block read fails.
    pub fn read_at(&mut self, off: u32, out: &mut [u8]) -> Result<usize, SourceError> {
        let mut done = 0;
        while done < out.len() {
            let Some(pos) = off.checked_add(done as u32) else { break };
            if pos >= self.len {
                break;
            }
            let block = pos / BLOCK as u32;
            if block != self.block {
                self.block = u32::MAX;
                self.valid = self.src.read_block(self.file, block, &mut self.buf)?.min(BLOCK);
                self.block = block;
            }
            let at = (pos % BLOCK as u32) as usize;
            if at >= self.valid {
                break;
            }
            let n = (self.valid - at).min(out.len() - done);
            out[done..done + n].copy_from_slice(&self.buf[at..at + n]);
            done += n;
        }
        Ok(done)
    }
}

/// A loaded sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleInfo {
    /// Source file.
    pub file: u16,
    /// Sample rate, Hz.
    pub rate: u32,
    /// Frames.
    pub frames: u32,
    /// Forward loop (start, end exclusive) from the file.
    pub loop_points: Option<(u32, u32)>,
    /// MIDI note at which it plays at its own rate (the file's, else 60).
    pub root: u8,
    /// The parsed file.
    pub wav: WavInfo,
}

/// Why a sample did not load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// The file is not a usable WAV sample.
    Wav(WavError),
    /// Slot number ≥ [`MAX_SAMPLES`].
    BadSlot,
    /// Not enough unpinned pages for the head.
    NoRoom,
}

impl From<WavError> for LoadError {
    fn from(e: WavError) -> Self {
        Self::Wav(e)
    }
}

impl From<SourceError> for LoadError {
    fn from(e: SourceError) -> Self {
        Self::Wav(WavError::Source(e))
    }
}

/// A play head: what [`SampleCache::service`] keeps loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Want {
    /// Sample slot.
    pub slot: u8,
    /// Current frame.
    pub frame: u32,
    /// Frames per output frame.
    pub speed: f32,
    /// Loops: wrap the look-ahead at the loop end.
    pub looping: bool,
}

const FREE: u8 = 0xFF;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tag {
    slot: u8,
    pinned: bool,
    page: u32,
    used: u32,
}

const NO_TAG: Tag = Tag { slot: FREE, pinned: false, page: 0, used: 0 };

/// Counters of a cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Pages loaded (head and streamed).
    pub loads: u32,
    /// Pages evicted to make room.
    pub evictions: u32,
    /// Wanted pages that could not be loaded (no room or a source error).
    pub failures: u32,
}

/// The bounded sample cache (see the module docs).
pub struct SampleCache<'m> {
    mem: &'m mut [i16],
    pages: usize,
    tags: [Tag; MAX_PAGES],
    slots: [Option<SampleInfo>; MAX_SAMPLES],
    clock: u32,
    stats: CacheStats,
}

impl core::fmt::Debug for SampleCache<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SampleCache")
            .field("pages", &self.pages)
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl<'m> SampleCache<'m> {
    /// A cache over `mem` (`mem.len() / 256` pages, at most [`MAX_PAGES`]).
    #[must_use]
    pub fn new(mem: &'m mut [i16]) -> Self {
        let pages = (mem.len() / PAGE_FRAMES as usize).min(MAX_PAGES);
        Self {
            mem,
            pages,
            tags: [NO_TAG; MAX_PAGES],
            slots: [None; MAX_SAMPLES],
            clock: 0,
            stats: CacheStats::default(),
        }
    }

    /// Pages in the cache.
    #[must_use]
    pub fn pages(&self) -> usize {
        self.pages
    }

    /// Pages holding nothing.
    #[must_use]
    pub fn free_pages(&self) -> usize {
        self.tags[..self.pages].iter().filter(|t| t.slot == FREE).count()
    }

    /// Counters.
    #[must_use]
    pub fn stats(&self) -> CacheStats {
        self.stats
    }

    /// The sample in `slot`.
    #[must_use]
    pub fn info(&self, slot: u8) -> Option<&SampleInfo> {
        self.slots.get(usize::from(slot)).and_then(Option::as_ref)
    }

    /// Frames of page `page` of sample `slot` (fewer in the last page), if
    /// cached. `hint` remembers where the last lookup found its page.
    #[inline]
    #[must_use]
    pub fn page(&self, slot: u8, page: u32, hint: &mut u16) -> Option<&[i16]> {
        let h = usize::from(*hint);
        let i = if h < self.pages && self.tags[h].slot == slot && self.tags[h].page == page {
            h
        } else {
            let i = self.tags[..self.pages].iter().position(|t| t.slot == slot && t.page == page)?;
            *hint = i as u16;
            i
        };
        let frames = self.slots[usize::from(slot)].map_or(0, |s| s.frames);
        let n = frames.saturating_sub(page * PAGE_FRAMES).min(PAGE_FRAMES) as usize;
        let at = i * PAGE_FRAMES as usize;
        Some(&self.mem[at..at + n])
    }

    /// Free `slot` and its pages.
    pub fn unload(&mut self, slot: u8) {
        if let Some(s) = self.slots.get_mut(usize::from(slot)) {
            *s = None;
        }
        for t in &mut self.tags[..self.pages] {
            if t.slot == slot {
                *t = NO_TAG;
            }
        }
    }

    /// Parse `file` of `src` as a WAV sample into `slot` (replacing what it
    /// held) and load its first `head_pages` pages pinned (`u32::MAX`: the
    /// whole sample). A note starting inside a render block plays from the
    /// head until the next [`SampleCache::service`], so the head must hold
    /// a block's worth at the highest speed the sample is played at
    /// (`block × speed / 256` pages; 4 pages cover 512-frame blocks up to
    /// an octave above the root at the same rate).
    ///
    /// # Errors
    ///
    /// [`LoadError`]; the slot is then empty.
    pub fn load<S: SampleSource + ?Sized>(
        &mut self,
        src: &mut S,
        slot: u8,
        file: u16,
        head_pages: u32,
    ) -> Result<SampleInfo, LoadError> {
        if usize::from(slot) >= MAX_SAMPLES {
            return Err(LoadError::BadSlot);
        }
        self.unload(slot);
        let mut r = Reader::new(src, file)?;
        let w = wav::parse(&mut r)?;
        let info = SampleInfo {
            file,
            rate: w.rate,
            frames: w.frames,
            loop_points: w.loop_points,
            root: w.unity_note.unwrap_or(60),
            wav: w,
        };
        let head = info.frames.div_ceil(PAGE_FRAMES).min(head_pages);
        let pinned = self.tags[..self.pages].iter().filter(|t| t.pinned).count();
        if head as usize > self.pages - pinned {
            return Err(LoadError::NoRoom);
        }
        self.slots[usize::from(slot)] = Some(info);
        self.clock = self.clock.wrapping_add(1);
        for page in 0..head {
            if let Err(e) = self.fill(&mut r, slot, page, true) {
                self.unload(slot);
                return Err(e);
            }
        }
        Ok(info)
    }

    /// A page for (`slot`, `page`): a free one, else the least recently
    /// wanted unpinned one not wanted in this pass.
    fn victim(&mut self) -> Option<usize> {
        if let Some(i) = self.tags[..self.pages].iter().position(|t| t.slot == FREE) {
            return Some(i);
        }
        let mut best: Option<(usize, u32)> = None;
        for (i, t) in self.tags[..self.pages].iter().enumerate() {
            let age = self.clock.wrapping_sub(t.used);
            if !t.pinned && age > 0 && best.is_none_or(|(_, a)| age > a) {
                best = Some((i, age));
            }
        }
        let i = best?.0;
        self.stats.evictions += 1;
        Some(i)
    }

    fn fill<S: SampleSource + ?Sized>(
        &mut self,
        r: &mut Reader<'_, S>,
        slot: u8,
        page: u32,
        pinned: bool,
    ) -> Result<(), LoadError> {
        let Some(info) = self.slots[usize::from(slot)] else { return Ok(()) };
        let Some(i) = self.victim() else {
            self.stats.failures += 1;
            return Err(LoadError::NoRoom);
        };
        self.tags[i] = NO_TAG;
        let at = i * PAGE_FRAMES as usize;
        let buf = &mut self.mem[at..at + PAGE_FRAMES as usize];
        let n = match wav::read_frames(r, &info.wav, page * PAGE_FRAMES, buf) {
            Ok(n) => n,
            Err(e) => {
                self.stats.failures += 1;
                return Err(e.into());
            }
        };
        buf[n..].fill(0);
        self.tags[i] = Tag { slot, pinned, page, used: self.clock };
        self.stats.loads += 1;
        Ok(())
    }

    fn touch(&mut self, slot: u8, page: u32) -> bool {
        let clock = self.clock;
        match self.tags[..self.pages].iter_mut().find(|t| t.slot == slot && t.page == page) {
            Some(t) => {
                t.used = clock;
                true
            }
            None => false,
        }
    }

    /// Load (at most `max_pages`) pages the play heads `wants` will reach
    /// within [`AHEAD_FRAMES`] output frames; returns the pages loaded.
    /// Wanted pages already cached are marked as recently used, so they are
    /// evicted last.
    ///
    /// # Errors
    ///
    /// The first source error (the remaining wants are skipped; a page that
    /// did not fit counts in [`CacheStats::failures`] without an error).
    pub fn service<S: SampleSource + ?Sized>(
        &mut self,
        src: &mut S,
        wants: &[Want],
        max_pages: usize,
    ) -> Result<usize, SourceError> {
        self.clock = self.clock.wrapping_add(1);
        // Mark first, so that loading for one head never evicts another's.
        let mut need = 0;
        for w in wants {
            for (slot, page) in Self::span(self.info(w.slot).copied(), w) {
                if !self.touch(slot, page) {
                    need += 1;
                }
            }
        }
        if need == 0 {
            return Ok(0);
        }
        let mut loaded = 0;
        for w in wants {
            let Some(info) = self.info(w.slot).copied() else { continue };
            let mut r: Option<Reader<'_, S>> = None;
            for (slot, page) in Self::span(Some(info), w) {
                if loaded >= max_pages {
                    return Ok(loaded);
                }
                if self.touch(slot, page) {
                    continue;
                }
                if r.is_none() {
                    r = Some(Reader::new(src, info.file)?);
                }
                if let Some(rd) = r.as_mut() {
                    match self.fill(rd, slot, page, false) {
                        Ok(()) => loaded += 1,
                        Err(LoadError::Wav(WavError::Source(e))) => return Err(e),
                        Err(_) => {}
                    }
                }
            }
        }
        Ok(loaded)
    }

    /// The pages a play head reaches within the look-ahead.
    fn span(info: Option<SampleInfo>, w: &Want) -> impl Iterator<Item = (u8, u32)> {
        let (first, last, wrap) = match info {
            Some(info) if w.frame < info.frames => {
                let reach = (clampf(w.speed, 0.0, MAX_INC) * AHEAD_FRAMES as f32) as u32 + 3;
                let end = w.frame.saturating_add(reach);
                let lp = info.loop_points.filter(|_| w.looping);
                match lp {
                    Some((ls, le)) if end >= le => {
                        let over = (end - le).min(le - ls);
                        (w.frame.saturating_sub(1), le - 1, Some((ls, ls + over)))
                    }
                    _ => (w.frame.saturating_sub(1), end.min(info.frames - 1), None),
                }
            }
            _ => (1, 0, None),
        };
        // Page ranges as (first, count): empty when there is nothing to load.
        let a = if first > last {
            (0, 0)
        } else {
            (first / PAGE_FRAMES, last / PAGE_FRAMES - first / PAGE_FRAMES + 1)
        };
        let b = wrap.map_or((0, 0), |(s, e)| (s / PAGE_FRAMES, e / PAGE_FRAMES - s / PAGE_FRAMES + 1));
        let slot = w.slot;
        (a.0..a.0 + a.1).chain(b.0..b.0 + b.1).map(move |p| (slot, p))
    }
}

/// Loop mode of a sample patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleLoop {
    /// Play once to the end.
    Off,
    /// Loop the file's loop points (from its `smpl` chunk); once if it has
    /// none.
    File,
    /// Loop the whole sample.
    Whole,
}

impl SampleLoop {
    /// Binary code.
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// From a binary code.
    #[must_use]
    pub fn from_code(c: u8) -> Option<Self> {
        [Self::Off, Self::File, Self::Whole].get(usize::from(c)).copied()
    }
}

/// The source of a sample patch (`Kind::Sample`): played through the
/// patch's filter, envelopes and LFO like the oscillators.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleParams {
    /// Sample slot in the cache.
    pub slot: u8,
    /// MIDI note that plays the sample at its own rate (255: the file's
    /// unity note, else 60).
    pub root: u8,
    /// Fine tuning, semitones (−24 to 24).
    pub tune: f32,
    /// Loop mode.
    pub looping: SampleLoop,
    /// Note-off does not release: the note plays until the sample ends
    /// (one-shot samples only).
    pub one_shot: bool,
    /// Start position, fraction of the sample (0–0.99).
    pub start: f32,
}

impl SampleParams {
    /// Slot `slot` at its own pitch, looping its file loop.
    #[must_use]
    pub const fn new(slot: u8) -> Self {
        Self { slot, root: 255, tune: 0.0, looping: SampleLoop::File, one_shot: false, start: 0.0 }
    }

    /// The settings with every field in range.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        Self {
            slot: self.slot.min(MAX_SAMPLES as u8 - 1),
            root: if self.root > 127 { 255 } else { self.root },
            tune: clampf(self.tune, -24.0, 24.0),
            looping: self.looping,
            one_shot: self.one_shot,
            start: clampf(self.start, 0.0, 0.99),
        }
    }
}

/// A sample hit of a kit (see [`crate::kit`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitParams {
    /// Sample slot.
    pub slot: u8,
    /// Pitch, semitones (−24 to 24).
    pub tune: f32,
    /// Fade-out time constant ×0.25 s (0.1–4; 4 = no fade, play to the end).
    pub decay: f32,
    /// One-pole low pass from 200 Hz (0) to open (1).
    pub tone: f32,
    /// Level, 0–1.
    pub level: f32,
}

impl HitParams {
    /// The settings with every field in range.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        Self {
            slot: self.slot.min(MAX_SAMPLES as u8 - 1),
            tune: clampf(self.tune, -24.0, 24.0),
            decay: clampf(self.decay, 0.1, 4.0),
            tone: clampf(self.tone, 0.0, 1.0),
            level: clampf(self.level, 0.0, 1.0),
        }
    }
}

const NO_HINT: u16 = u16::MAX;
const S16: f32 = 1.0 / 32_768.0;

/// Cubic Hermite (Catmull-Rom) through `y1` (t = 0) and `y2` (t = 1).
#[inline]
fn hermite(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

/// A sample play head (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SamplePlayer {
    slot: u8,
    mode: SampleLoop,
    start: f32,
    ready: bool,
    done: bool,
    looping: bool,
    hint: u16,
    /// Frames per second ÷ output rate (set when the sample is found).
    rate_ratio: f32,
    root: u8,
    pos: u64,
    inc: u64,
    end: u32,
    loop_start: u32,
    loop_end: u32,
    misses: u32,
}

impl SamplePlayer {
    /// A player of `slot` that starts at fraction `start` of the sample on
    /// the first render (the sample's length and loop points are looked up
    /// then: a note-on does not need the cache).
    #[must_use]
    pub const fn new(slot: u8, mode: SampleLoop, start: f32) -> Self {
        Self {
            slot,
            mode,
            start,
            ready: false,
            done: false,
            looping: false,
            hint: NO_HINT,
            rate_ratio: 1.0,
            root: 60,
            pos: 0,
            inc: 1 << 32,
            end: 0,
            loop_start: 0,
            loop_end: 0,
            misses: 0,
        }
    }

    /// Look the sample up (once); false if the slot is empty.
    pub fn prepare(&mut self, cache: Option<&SampleCache<'_>>, rate: f32) -> bool {
        if self.ready {
            return !self.done;
        }
        self.ready = true;
        let Some(info) = cache.and_then(|c| c.info(self.slot)) else {
            self.done = true;
            return false;
        };
        self.end = info.frames;
        self.root = info.root;
        self.rate_ratio = info.rate as f32 / rate;
        let lp = match self.mode {
            SampleLoop::Off => None,
            SampleLoop::File => info.loop_points,
            SampleLoop::Whole => Some((0, info.frames)),
        };
        if let Some((s, e)) = lp.filter(|&(s, e)| s < e) {
            self.looping = true;
            self.loop_start = s;
            self.loop_end = e;
        }
        let first = (self.start * info.frames as f32) as u32;
        self.pos = u64::from(first.min(info.frames)) << 32;
        if info.frames == 0 {
            self.done = true;
        }
        !self.done
    }

    /// The file's root note (after [`SamplePlayer::prepare`]).
    #[must_use]
    pub fn root(&self) -> u8 {
        self.root
    }

    /// Set the speed from a pitch offset in semitones (0: the sample's own
    /// rate, adjusted to the output rate).
    pub fn set_pitch(&mut self, semis: f32) {
        let r = clampf(exp2(clampf(semis, -96.0, 96.0) * (1.0 / 12.0)) * self.rate_ratio, 0.0, MAX_INC);
        self.inc = (r * 4_294_967_296.0) as u64;
    }

    /// Frames per output frame.
    #[must_use]
    pub fn speed(&self) -> f32 {
        (self.inc >> 16) as f32 * (1.0 / 65_536.0)
    }

    /// Whether the sample has ended (or was not found).
    #[must_use]
    pub fn done(&self) -> bool {
        self.done
    }

    /// The play head, for [`SampleCache::service`].
    #[must_use]
    pub fn want(&self) -> Option<Want> {
        (self.ready && !self.done).then(|| Want {
            slot: self.slot,
            frame: (self.pos >> 32) as u32,
            speed: self.speed(),
            looping: self.looping,
        })
    }

    /// Samples rendered as silence because their page was missing, since
    /// the last call.
    pub fn take_misses(&mut self) -> u32 {
        core::mem::take(&mut self.misses)
    }

    /// Frame `j` (signed), mapping the loop seam; `None` if its page is
    /// missing.
    fn frame(&mut self, c: &SampleCache<'_>, j: i64) -> Option<f32> {
        let mut j = j;
        if self.looping && j >= i64::from(self.loop_end) {
            j -= i64::from(self.loop_end - self.loop_start);
        }
        if j < 0 || j >= i64::from(self.end) {
            return Some(0.0);
        }
        let j = j as u32;
        let p = c.page(self.slot, j / PAGE_FRAMES, &mut self.hint)?;
        Some(f32::from(p[(j % PAGE_FRAMES) as usize]))
    }

    /// Render into `out` (overwriting; zeros after the end); returns
    /// whether the sample is still playing. Values are in [−1, 1).
    pub fn render(&mut self, cache: Option<&SampleCache<'_>>, out: &mut [f32]) -> bool {
        let Some(c) = cache.filter(|_| !self.done && self.ready) else {
            out.fill(0.0);
            self.done = true;
            return false;
        };
        let mut k = 0;
        while k < out.len() {
            if self.done {
                out[k..].fill(0.0);
                break;
            }
            let i = (self.pos >> 32) as u32;
            let page = i / PAGE_FRAMES;
            let base = page * PAGE_FRAMES;
            let mut limit = base + PAGE_FRAMES;
            if self.looping {
                limit = limit.min(self.loop_end);
            }
            limit = limit.min(self.end);
            // Fast path: the four frames i−1…i+2 inside this page.
            if i > base
                && i + 2 < limit
                && let Some(p) = c.page(self.slot, page, &mut self.hint)
            {
                while k < out.len() {
                    let i = (self.pos >> 32) as u32;
                    if i + 2 >= limit {
                        break;
                    }
                    let t = ((self.pos as u32) >> 8) as f32 * (1.0 / 16_777_216.0);
                    let a = (i - base) as usize;
                    let y = hermite(
                        f32::from(p[a - 1]),
                        f32::from(p[a]),
                        f32::from(p[a + 1]),
                        f32::from(p[a + 2]),
                        t,
                    );
                    out[k] = y * S16;
                    k += 1;
                    self.pos += self.inc;
                }
                self.wrap();
                continue;
            }
            // Slow path: page edges, the loop seam, the ends, missing pages.
            let t = ((self.pos as u32) >> 8) as f32 * (1.0 / 16_777_216.0);
            let j = i64::from(i);
            let ys = [self.frame(c, j - 1), self.frame(c, j), self.frame(c, j + 1), self.frame(c, j + 2)];
            out[k] = match ys {
                [Some(a), Some(b), Some(d), Some(e)] => hermite(a, b, d, e, t) * S16,
                _ => {
                    self.misses += 1;
                    0.0
                }
            };
            k += 1;
            self.pos += self.inc;
            self.wrap();
        }
        !self.done
    }

    fn wrap(&mut self) {
        let i = (self.pos >> 32) as u32;
        if self.looping {
            if i >= self.loop_end {
                let len = u64::from(self.loop_end - self.loop_start) << 32;
                while (self.pos >> 32) as u32 >= self.loop_end {
                    self.pos -= len;
                }
            }
        } else if i >= self.end {
            self.done = true;
        }
    }
}

/// Level below which a sample hit's fade ends it (−80 dB).
const END: f32 = 1.0e-4;

/// A kit's sample hit: a one-shot player with a fade and a low pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleHit {
    player: SamplePlayer,
    tune: f32,
    level: f32,
    amp: f32,
    amp_c: f32,
    lp: f32,
    lp_c: f32,
    open: bool,
}

impl SampleHit {
    /// A hit of `p` at velocity gain `vel` (0–1), at `rate` Hz.
    #[must_use]
    pub fn new(p: &HitParams, vel: f32, rate: f32) -> Self {
        let p = p.sanitized();
        let fade = p.decay < 4.0;
        let cutoff = 200.0 * exp2(6.643_856 * p.tone);
        Self {
            player: SamplePlayer::new(p.slot, SampleLoop::Off, 0.0),
            tune: p.tune,
            level: p.level * vel,
            amp: 1.0,
            amp_c: if fade { decay_coef(0.25 * p.decay, rate) } else { 1.0 },
            lp: 0.0,
            lp_c: decay_coef(1.0 / (core::f32::consts::TAU * cutoff), rate),
            open: p.tone >= 1.0,
        }
    }

    /// The player.
    #[must_use]
    pub fn player(&self) -> &SamplePlayer {
        &self.player
    }

    /// Mutable player (misses).
    pub fn player_mut(&mut self) -> &mut SamplePlayer {
        &mut self.player
    }

    /// Current level.
    #[must_use]
    pub fn level(&self) -> f32 {
        if self.player.done() { 0.0 } else { self.amp * self.level }
    }

    /// Fade out quickly (choke group): τ 3 ms.
    pub fn choke(&mut self, rate: f32) {
        self.amp_c = self.amp_c.min(decay_coef(0.003, rate));
    }

    /// Render into `out` (overwriting); returns whether it still sounds.
    /// Ends are decided on control ticks only (see [`crate::drum`]).
    pub fn render(
        &mut self,
        cache: Option<&SampleCache<'_>>,
        rate: f32,
        out: &mut [f32],
        tick: bool,
    ) -> bool {
        if !self.player.ready && self.player.prepare(cache, rate) {
            self.player.set_pitch(self.tune);
        }
        if tick {
            self.amp = flush(self.amp);
            self.lp = flush(self.lp);
            if self.amp < END || self.level <= 0.0 || self.player.done() {
                out.fill(0.0);
                return false;
            }
        }
        self.player.render(cache, out);
        let g = self.level;
        for o in out.iter_mut() {
            let x = *o * self.amp;
            let y = if self.open {
                x
            } else {
                self.lp = x + (self.lp - x) * self.lp_c;
                self.lp
            };
            *o = y * g;
            self.amp *= self.amp_c;
        }
        // The end is decided on the next tick (above).
        true
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// A mono 16-bit WAV of `pcm` at `rate`.
    pub(crate) fn wav16(rate: u32, pcm: &[i16]) -> Vec<u8> {
        let mut v = Vec::new();
        let data = (pcm.len() * 2) as u32;
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * 2).to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data.to_le_bytes());
        for s in pcm {
            v.extend_from_slice(&s.to_le_bytes());
        }
        v
    }

    #[test]
    fn unit_speed_plays_the_samples_exactly() {
        let pcm: Vec<i16> = (0..1000).map(|i| ((i * 37) % 2000 - 1000) as i16).collect();
        let file = wav16(48_000, &pcm);
        let files = [file.as_slice()];
        let mut src = SliceSource::new(&files);
        let mut mem = [0i16; 256 * 8];
        let mut cache = SampleCache::new(&mut mem);
        let info = cache.load(&mut src, 3, 0, u32::MAX).unwrap();
        assert_eq!(info.frames, 1000);
        let mut p = SamplePlayer::new(3, SampleLoop::Off, 0.0);
        assert!(p.prepare(Some(&cache), 48_000.0));
        p.set_pitch(0.0);
        let mut out = [0.0f32; 1100];
        for chunk in out.chunks_mut(77) {
            p.render(Some(&cache), chunk);
        }
        for (i, &s) in pcm.iter().enumerate() {
            assert_eq!(out[i], f32::from(s) / 32_768.0, "frame {i}");
        }
        assert!(out[1000..].iter().all(|&x| x == 0.0));
        assert!(p.done());
    }

    #[test]
    fn loops_wrap_and_streaming_counts_misses() {
        let pcm: Vec<i16> = (0..3000).map(|i| (i % 300) as i16 * 50).collect();
        let file = wav16(48_000, &pcm);
        let files = [file.as_slice()];
        let mut src = SliceSource::new(&files);
        let mut mem = [0i16; 256 * 6];
        let mut cache = SampleCache::new(&mut mem);
        cache.load(&mut src, 0, 0, 2).unwrap();
        assert_eq!(cache.free_pages(), 4);
        let mut p = SamplePlayer::new(0, SampleLoop::Whole, 0.0);
        p.prepare(Some(&cache), 48_000.0);
        p.set_pitch(0.0);
        // Without service the third page is missing: silence and misses.
        let mut out = [0.0f32; 800];
        p.render(Some(&cache), &mut out);
        assert!(p.take_misses() > 0);
        // With service before each block: no misses, the loop wraps.
        let mut p = SamplePlayer::new(0, SampleLoop::Whole, 0.0);
        p.prepare(Some(&cache), 48_000.0);
        p.set_pitch(0.0);
        let mut all = Vec::new();
        for _ in 0..40 {
            let w = p.want().unwrap();
            cache.service(&mut src, &[w], 64).unwrap();
            let mut b = [0.0f32; 256];
            p.render(Some(&cache), &mut b);
            all.extend_from_slice(&b);
        }
        assert_eq!(p.take_misses(), 0);
        for (i, &x) in all.iter().enumerate() {
            assert_eq!(x, f32::from(pcm[i % 3000]) / 32_768.0, "frame {i}");
        }
        assert!(cache.stats().evictions > 0);
    }
}
