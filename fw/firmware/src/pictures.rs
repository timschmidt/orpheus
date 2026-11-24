//! Pictures and album art in the card task (WP AU-14, device part;
//! docs/plan/04-audio.md §7, 05-ui-apps.md §7.1 and §7.6).
//!
//! **Where pixels go.** Nothing keeps a decoded picture in RAM. A screen
//! asks for a picture while it draws (`PictureService::show_picture`); after
//! that frame went to the panel the UI lends the frame buffer and sends a
//! [`PicJob`] (`media::Req::Picture`). This task writes the job's box of the
//! frame buffer — a decoder through the `orpheus-img` scaler, or a cached
//! thumbnail read straight from the card — and hands the frame buffer back
//! with `tag::PICTURE`; the screen then marks the box dirty. The UI draws
//! nothing in between and may take a job back before it started
//! ([`media::PIC_SLOT`]) or cancel a running one ([`media::PIC_CANCEL`]).
//!
//! **Decoder memory.** Pictures are decoded with the 64 KiB decoder arena,
//! so only while no track is loaded (the idle loop, [`serve_idle`]). While
//! a track is loaded (playing or paused) only ~2 KiB of it are free: a
//! picture file answers `Busy` ("stop playback to view pictures"); album art
//! is shown from the cache only ([`serve_playing`], between ring refills).
//!
//! **Album art** is made when a track opens, before the audio decoder takes
//! the arena ([`track_art`]): the art cache (`/.orpheus/cache/art`, `YTC1`
//! buckets: 52 × 52 `A…`, 128 × 128 `B…`) is checked for the track; on a
//! miss the embedded picture (ID3 `APIC`, FLAC `PICTURE`, via `orpheus-meta`)
//! is decoded once into both sizes (the big one only for JPEG, BMP and QOI:
//! PNG and GIF need the arena for themselves) and stored; without one, the
//! folder's `cover.jpg`/`folder.jpg`/… is used (cached under the cover
//! file's own key). An entry with a 0 × 0 original records "no picture",
//! so the next open does not look again. Big art without a big copy shows
//! the small one centred.

use core::future::poll_fn;
use core::sync::atomic::Ordering;
use core::task::{Context, Poll};

use atj2157_hal::time;
use embassy_time::Timer;
use orpheus_apps::services::{ArtSize, FileRef, PicError, PicFormat, PicInfo, PicSource, PicState};
use orpheus_audio::{ByteSource, SeekSource};
use orpheus_fs::{DirEntry, File, Locator, OpenMode};
use orpheus_img::art::{Tee, cover_rank};
use orpheus_img::thumb::{ART, ART_BIG, CacheFile, CacheKind, ThumbKey, ThumbMeta};
use orpheus_img::{Decoder, Fit, Format, Info, Options, Orient, Placement, RowSink, Scaler, Step, Target};
use orpheus_index::card::spin;
use orpheus_meta::{MetaStep, Opts, TagReader, Tags};
use orpheus_player::TrackRef;

use crate::audio::{Ring, boost};
use crate::media::{self, PicJob, Server, Vol};

/// Panel width: the frame buffer's stride.
const STRIDE: usize = 128;

/// The current track's album art in the cache: the key of its small copy
/// (the big copy has the same key at 128 × 128).
#[derive(Clone, Copy, Debug)]
pub struct ArtRef {
    pub key: ThumbKey,
}

// ------------------------------------------------------------ jobs

/// A job while a track is loaded: album art from the cache, or `Busy`.
pub async fn serve_playing(vol: &mut Vol, srv: &Server, j: &PicJob) {
    if !media::picture_start(j) {
        return;
    }
    let st = match j.req.src {
        PicSource::File(_) => PicState::Busy,
        PicSource::Art(size) => show_art(vol, srv, j, size).await,
    };
    media::picture_done(j, st);
}

/// A job from the idle loop (no track loaded): the whole decoder arena and
/// the stream ring are free.
pub async fn serve_idle(vol: &mut Vol, ring: &mut Ring, arena: &mut [u8], srv: &Server, j: &PicJob) {
    if !media::picture_start(j) {
        return;
    }
    let st = match j.req.src {
        PicSource::Art(size) => show_art(vol, srv, j, size).await,
        PicSource::File(f) => {
            boost(true);
            let st = show_file(vol, ring, arena, j, f).await;
            boost(false);
            st
        }
    };
    media::picture_done(j, st);
}

/// The job's box, if it lies inside the panel.
fn job_box(j: &PicJob) -> Option<(usize, usize, u16, u16)> {
    let (x, y, w, h) = (usize::from(j.x), usize::from(j.y), u16::from(j.w), u16::from(j.h));
    (w > 0 && h > 0 && x + usize::from(w) <= STRIDE && y + usize::from(h) <= STRIDE).then_some((x, y, w, h))
}

/// The frame buffer the UI lent for `j`.
fn frame_buffer(j: &PicJob) -> &'static mut [u16] {
    // SAFETY: `j.fb` is the UI's 128 × 128 frame buffer (an `&mut [u16]`
    // the UI task holds and does not touch while a job is out:
    // `media::picture_start` succeeded and `picture_done` has not been
    // called). The slice is used only until `picture_done`; the
    // critical sections of `PIC_SLOT`/`PIC_RESULT` order the accesses.
    unsafe { core::slice::from_raw_parts_mut(j.fb as *mut u16, STRIDE * STRIDE) }
}

/// A picture file into the job's box.
async fn show_file(vol: &mut Vol, ring: &mut Ring, arena: &mut [u8], j: &PicJob, f: FileRef) -> PicState {
    let Some((x, y, w, h)) = job_box(j) else { return PicState::Failed(PicError::Io) };
    let mut e = DirEntry::new();
    if vol.open_locator(Locator { dir: f.dir, index: f.index }, &mut e).await.is_err() {
        return PicState::Failed(PicError::Io);
    }
    let Ok(mut file) = vol.open_entry(&e) else { return PicState::Failed(PicError::Io) };
    ring.reset(file.size());
    let Some((work, rest)) = carve_u32(arena, Scaler::need_words(usize::from(w.max(h)))) else {
        return PicState::Failed(PicError::TooLarge);
    };
    let p = Placement {
        fit: if j.req.fill { Fit::Cover } else { Fit::Contain },
        rotate: Orient::quarter_turns(j.req.turns),
        ..Placement::fit(w, h)
    };
    let fb = frame_buffer(j);
    let Ok(mut sink) = Scaler::new(work, p, Target { buf: fb, stride: STRIDE, x, y }) else {
        return PicState::Failed(PicError::TooLarge);
    };
    let Some(dec) = decoder_in(rest) else { return PicState::Failed(PicError::TooLarge) };
    let r = run(vol, ring, &mut file, dec, &mut sink, ui_cancelled).await;
    let _ = vol.close(file).await;
    match r {
        Ok(()) => dec.info().map_or(PicState::Failed(PicError::Corrupt), |i| PicState::Shown(pic_info(i))),
        Err(Stop::Cancelled) => PicState::Pending,
        Err(Stop::Io) => PicState::Failed(PicError::Io),
        Err(Stop::Img(e)) => PicState::Failed(pic_error(e)),
    }
}

/// The current track's album art from the cache into the job's box.
async fn show_art(vol: &mut Vol, srv: &Server, j: &PicJob, size: ArtSize) -> PicState {
    let Some(a) = srv.art else { return PicState::Nothing };
    let Some((x, y, w, h)) = job_box(j) else { return PicState::Failed(PicError::Io) };
    let fb = frame_buffer(j);
    let at = y * STRIDE + x;
    if size == ArtSize::Full && w >= u16::from(ART_BIG.w) && h >= u16::from(ART_BIG.h) {
        let key = sized(a.key, &ART_BIG);
        if let Some(m) = show_cached(vol, &ART_BIG, &key, fb, at).await {
            return PicState::Shown(art_info(m));
        }
    }
    // The small copy, centred in a larger box (Big art without a big copy).
    let (sw, sh) = (u16::from(ART.w), u16::from(ART.h));
    if w < sw || h < sh {
        return PicState::Nothing;
    }
    let (dx, dy) = (usize::from((w - sw) / 2), usize::from((h - sh) / 2));
    let key = sized(a.key, &ART);
    match show_cached(vol, &ART, &key, fb, at + dy * STRIDE + dx).await {
        Some(m) if m.orig_w > 0 => {
            // Black around it.
            for row in 0..usize::from(h) {
                let line = &mut fb[at + row * STRIDE..at + row * STRIDE + usize::from(w)];
                if row < dy || row >= dy + usize::from(sh) {
                    line.fill(0);
                } else {
                    line[..dx].fill(0);
                    line[dx + usize::from(sw)..].fill(0);
                }
            }
            PicState::Shown(art_info(m))
        }
        _ => PicState::Nothing,
    }
}

fn art_info(m: ThumbMeta) -> PicInfo {
    PicInfo { width: u32::from(m.orig_w), height: u32::from(m.orig_h), format: None, reduced: m.approximate }
}

fn pic_info(i: Info) -> PicInfo {
    let format = match i.format {
        Format::Jpeg => PicFormat::Jpeg,
        Format::Png => PicFormat::Png,
        Format::Gif => PicFormat::Gif,
        Format::Bmp => PicFormat::Bmp,
        Format::Qoi => PicFormat::Qoi,
    };
    PicInfo { width: i.width, height: i.height, format: Some(format), reduced: i.interlaced }
}

fn pic_error(e: orpheus_img::Error) -> PicError {
    use orpheus_img::Error as E;
    match e {
        E::UnknownFormat | E::Unsupported(_) => PicError::Unsupported,
        E::Arena { .. } => PicError::TooLarge,
        E::Corrupt | E::Truncated => PicError::Corrupt,
    }
}

// ------------------------------------------------------------ decoding

/// Why a decode ended early.
enum Stop {
    Cancelled,
    Io,
    Img(orpheus_img::Error),
}

/// The UI cancelled the running job (a key).
fn ui_cancelled() -> bool {
    media::PIC_CANCEL.load(Ordering::Relaxed)
}

/// Something for the player arrived while a track opens (skip, stop).
fn player_waiting() -> bool {
    !media::PLAY.is_empty()
}

/// Run a decoder over the file `f` through the stream ring until the
/// picture is complete. Sleeps briefly after every 8 ms of work: the card
/// task runs on the RT executor, thread mode (UI, USB, the watchdog's
/// beats) must keep running during a long decode.
async fn run(
    vol: &mut Vol,
    ring: &mut Ring,
    f: &mut File,
    dec: &mut Decoder<'_>,
    sink: &mut dyn RowSink,
    cancelled: fn() -> bool,
) -> Result<(), Stop> {
    let mut t0 = time::now_us();
    // Each step does some work or needs data the next fill brings: bounded
    // by the file, the bound only stops a broken decoder.
    for _ in 0..4_000_000u32 {
        match dec.step(ring, sink) {
            Ok(Step::Done) => return Ok(()),
            Ok(Step::Progress) => {}
            Ok(Step::NeedData(_)) => {
                if (ring.eof && !ring.reseek) || (!ring.fill(vol, f).await && !ring.eof) {
                    return Err(Stop::Io);
                }
            }
            Err(e) => return Err(Stop::Img(e)),
        }
        if cancelled() {
            return Err(Stop::Cancelled);
        }
        if time::now_us() - t0 >= 8_000 {
            Timer::after_micros(1_500).await;
            t0 = time::now_us();
        }
    }
    Err(Stop::Img(orpheus_img::Error::Corrupt))
}

/// Place `v()` at the start of `buf` (aligned); returns it and the rest.
/// Used for the decoder and the tag reader: they live in the arena, not in
/// the card task's future or on the stack. Only types without drop glue
/// (they are never dropped).
fn place<T>(buf: &mut [u8], v: impl FnOnce() -> T) -> Option<(&mut T, &mut [u8])> {
    const { assert!(!core::mem::needs_drop::<T>()) };
    let off = buf.as_ptr().align_offset(core::mem::align_of::<T>());
    let end = off.checked_add(core::mem::size_of::<T>())?;
    if end > buf.len() {
        return None;
    }
    let (head, rest) = buf.split_at_mut(end);
    let p = head[off..].as_mut_ptr().cast::<T>();
    // SAFETY: `p` is aligned for `T` and `size_of::<T>()` bytes from it lie
    // in `head`, which is exclusively borrowed for the returned lifetime.
    unsafe {
        p.write(v());
        Some((&mut *p, rest))
    }
}

/// A decoder whose memory is the rest of `buf` after the decoder itself.
fn decoder_in(buf: &mut [u8]) -> Option<&mut Decoder<'_>> {
    let need = core::mem::size_of::<Decoder<'static>>() + core::mem::align_of::<Decoder<'static>>();
    if buf.len() < need {
        return None;
    }
    let (slot, mem) = buf.split_at_mut(need);
    place(slot, || Decoder::new(mem, Options::default())).map(|(d, _)| d)
}

/// `n` words from the start of `buf` (aligned), and the rest.
fn carve_u32(buf: &mut [u8], n: usize) -> Option<(&mut [u32], &mut [u8])> {
    let off = buf.as_ptr().align_offset(4);
    let end = off.checked_add(n * 4)?;
    if end > buf.len() {
        return None;
    }
    let (head, rest) = buf.split_at_mut(end);
    // SAFETY: any bit pattern is a valid `u32`; `align_to_mut` only yields
    // aligned words.
    let (_, words, _) = unsafe { head[off..].align_to_mut::<u32>() };
    Some((words, rest))
}

/// `n` pixels from the start of `buf` (aligned), and the rest.
fn carve_u16(buf: &mut [u8], n: usize) -> Option<(&mut [u16], &mut [u8])> {
    let off = buf.as_ptr().align_offset(2);
    let end = off.checked_add(n * 2)?;
    if end > buf.len() {
        return None;
    }
    let (head, rest) = buf.split_at_mut(end);
    // SAFETY: any bit pattern is a valid `u16`.
    let (_, px, _) = unsafe { head[off..].align_to_mut::<u16>() };
    Some((px, rest))
}

// ------------------------------------------------------------ the cache

/// A bucket file of the art cache, read and written synchronously (the
/// `CacheFile` trait) by spinning the file system with the card task's
/// context, like the index files (`orpheus_index::card`).
struct CardFile<'a, 'c> {
    vol: &'a mut Vol,
    cx: &'a mut Context<'c>,
    f: &'a mut File,
}

impl CacheFile for CardFile<'_, '_> {
    type Error = ();

    fn read_at(&mut self, off: u32, buf: &mut [u8]) -> Result<usize, ()> {
        self.f.set_position(u64::from(off));
        let mut n = 0;
        while n < buf.len() {
            match spin(self.cx, self.vol.read(self.f, &mut buf[n..])) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(_) => return Err(()),
            }
        }
        Ok(n)
    }

    fn write_at(&mut self, off: u32, data: &[u8]) -> Result<(), ()> {
        self.f.set_position(u64::from(off));
        spin(self.cx, self.vol.write(self.f, data)).map(|_| ()).map_err(|_| ())
    }
}

/// `dir/XNNN.YTC` of the bucket holding `key`.
struct BucketPath {
    buf: [u8; 40],
    len: usize,
}

impl BucketPath {
    fn new(kind: &CacheKind, key: &ThumbKey) -> Self {
        let mut buf = [0u8; 40];
        let dir = kind.dir.as_bytes();
        let name = kind.file_name(kind.bucket(key));
        let len = (dir.len() + 1 + name.len()).min(buf.len());
        let d = dir.len().min(len);
        buf[..d].copy_from_slice(&dir[..d]);
        if d < len {
            buf[d] = b'/';
            let n = len - d - 1;
            buf[d + 1..len].copy_from_slice(&name[..n]);
        }
        Self { buf, len }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

/// Open the bucket of `key`; for writing it is created (with its folders,
/// all under `/.orpheus/`).
async fn open_bucket(vol: &mut Vol, kind: &CacheKind, key: &ThumbKey, write: bool) -> Option<File> {
    let p = BucketPath::new(kind, key);
    if !write {
        return vol.open(p.as_str(), OpenMode::Read).await.ok();
    }
    if let Ok(f) = vol.open(p.as_str(), OpenMode::Create).await {
        return Some(f);
    }
    for d in ["/.orpheus", "/.orpheus/cache", kind.dir] {
        let _ = vol.create_dir(d).await;
    }
    vol.open(p.as_str(), OpenMode::Create).await.ok()
}

/// What the cache holds for `key` (no pixels read).
async fn cached_meta(vol: &mut Vol, kind: &CacheKind, key: &ThumbKey) -> Option<ThumbMeta> {
    let mut f = open_bucket(vol, kind, key, false).await?;
    let r = poll_fn(|cx| Poll::Ready(kind.meta(&mut CardFile { vol: &mut *vol, cx, f: &mut f }, key))).await;
    let _ = vol.close(f).await;
    r.ok().flatten()
}

/// The cached picture of `key` into `fb` at `at` (rows of [`STRIDE`]).
async fn show_cached(
    vol: &mut Vol,
    kind: &CacheKind,
    key: &ThumbKey,
    fb: &mut [u16],
    at: usize,
) -> Option<ThumbMeta> {
    let mut f = open_bucket(vol, kind, key, false).await?;
    let out = fb.get_mut(at..)?;
    let r = poll_fn(|cx| {
        Poll::Ready(kind.lookup_into(&mut CardFile { vol: &mut *vol, cx, f: &mut f }, key, &mut *out, STRIDE))
    })
    .await;
    let _ = vol.close(f).await;
    r.ok().flatten()
}

/// Store a picture (`kind.w × kind.h` pixels) under `key`.
async fn store(vol: &mut Vol, kind: &CacheKind, key: &ThumbKey, meta: &ThumbMeta, px: &[u16]) -> bool {
    let Some(mut f) = open_bucket(vol, kind, key, true).await else { return false };
    let r =
        poll_fn(|cx| Poll::Ready(kind.store(&mut CardFile { vol: &mut *vol, cx, f: &mut f }, key, meta, px)))
            .await;
    vol.close(f).await.is_ok() && r.is_ok()
}

/// Record that `key` has no picture (a 0 × 0 original, black pixels).
async fn store_none(vol: &mut Vol, arena: &mut [u8], key: &ThumbKey) {
    let n = usize::from(ART.w) * usize::from(ART.h);
    let Some((px, _)) = carve_u16(arena, n) else { return };
    px.fill(0);
    let none = ThumbMeta { orig_w: 0, orig_h: 0, approximate: false };
    if store(vol, &ART, &sized(*key, &ART), &none, px).await {
        let _ = vol.flush().await;
    }
}

/// `key` for the thumbnails of `kind`.
fn sized(key: ThumbKey, kind: &CacheKind) -> ThumbKey {
    ThumbKey { w: kind.w, h: kind.h, ..key }
}

/// The key of a file's art (04 §6.3 locator, size, modification time).
fn file_key(serial: u32, e: &DirEntry) -> ThumbKey {
    let t = TrackRef::new(serial, e.locator(), e.name());
    let mut locator = [0u8; 16];
    for (i, v) in [t.serial, t.dir, t.index, t.name_hash].into_iter().enumerate() {
        locator[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
    }
    let (date, tm) = e.modified().to_fat();
    ThumbKey {
        locator,
        file_size: e.size(),
        mtime: u32::from(date) << 16 | u32::from(tm),
        part: 0,
        w: 0,
        h: 0,
        rotation: 1,
        reserved: 0,
    }
}

// ------------------------------------------------------------ album art

/// Find the album art of the track at `l` (cache, embedded picture, folder
/// cover) and make it the current one. Called when a track opens, before
/// the audio decoder takes the arena; gives up as soon as a player request
/// arrives (the track is being skipped).
pub async fn track_art(vol: &mut Vol, ring: &mut Ring, arena: &mut [u8], srv: &mut Server, l: Locator) {
    srv.set_art(None);
    let serial = vol.serial();
    let mut e = DirEntry::new();
    if vol.open_locator(l, &mut e).await.is_err() {
        return;
    }
    let key = file_key(serial, &e);
    match cached_meta(vol, &ART, &sized(key, &ART)).await {
        Some(m) if m.orig_w > 0 => {
            srv.set_art(Some(ArtRef { key }));
            return;
        }
        // Recorded before: no embedded picture.
        Some(_) => {}
        None => {
            if let Some(at) = embedded(vol, ring, arena, &e).await
                && make_art(vol, ring, arena, &e, Some(at), &key).await
            {
                srv.set_art(Some(ArtRef { key }));
                return;
            }
            if player_waiting() {
                return;
            }
            store_none(vol, arena, &key).await;
        }
    }
    // A cover picture in the track's folder: looked for once per folder
    // (the next track of an album, or of a big folder without one, does
    // not scan the folder again).
    if let Some((dir, art)) = srv.folder_art
        && dir == l.dir
    {
        srv.set_art(art);
        return;
    }
    if player_waiting() {
        return;
    }
    let art = folder_art(vol, ring, arena, l, &mut e, serial).await;
    if !player_waiting() {
        srv.folder_art = Some((l.dir, art));
    }
    srv.set_art(art);
}

/// The art of the best cover picture in the folder of `l`, made if needed.
async fn folder_art(
    vol: &mut Vol,
    ring: &mut Ring,
    arena: &mut [u8],
    l: Locator,
    e: &mut DirEntry,
    serial: u32,
) -> Option<ArtRef> {
    if !find_cover(vol, l, e).await {
        return None;
    }
    let key = file_key(serial, e);
    match cached_meta(vol, &ART, &sized(key, &ART)).await {
        Some(m) if m.orig_w > 0 => Some(ArtRef { key }),
        Some(_) => None,
        None => {
            if make_art(vol, ring, arena, e, None, &key).await {
                return Some(ArtRef { key });
            }
            if !player_waiting() {
                store_none(vol, arena, &key).await;
            }
            None
        }
    }
}

/// The location of the embedded front cover (or first picture) of a
/// track, from its tags. Pictures stored ID3-unsynchronised are not used
/// (their bytes need a staging copy the ring cannot refill under).
async fn embedded(
    vol: &mut Vol,
    ring: &mut Ring,
    arena: &mut [u8],
    e: &DirEntry,
) -> Option<orpheus_meta::Located> {
    use orpheus_audio::Format as A;
    let ext = e.name().rsplit_once('.').map_or("", |(_, x)| x);
    let is = |x: &str| ext.eq_ignore_ascii_case(x);
    let format = if is("mp3") || is("mp2") {
        A::Mpeg
    } else if is("flac") {
        A::Flac
    } else if is("wav") {
        A::Wav
    } else if is("aif") || is("aiff") {
        A::Aiff
    } else {
        return None;
    };
    let mut f = vol.open_entry(e).ok()?;
    ring.reset(f.size());
    let (reader, rest) = place(arena, || TagReader::new(format, Opts::default()))?;
    let (tags, _) = place(rest, Tags::default)?;
    let mut t0 = time::now_us();
    for _ in 0..4096 {
        match reader.step(ring, tags) {
            Ok(MetaStep::Done) | Err(_) => break,
            Ok(MetaStep::Continue) => {}
            Ok(MetaStep::NeedData(_)) => {
                if (ring.eof && !ring.reseek) || (!ring.fill(vol, &mut f).await && !ring.eof) {
                    break;
                }
            }
        }
        // The front cover: the rest of the tags do not matter here.
        if tags.picture.is_some_and(|p| p.kind == 3) || player_waiting() {
            break;
        }
        if time::now_us() - t0 >= 8_000 {
            Timer::after_micros(1_500).await;
            t0 = time::now_us();
        }
    }
    let _ = vol.close(f).await;
    tags.picture.map(|p| p.at).filter(|at| !at.unsync && at.len > 0 && !player_waiting())
}

/// The best cover picture in the folder of the entry at `l` (`e` then
/// holds it).
async fn find_cover(vol: &mut Vol, l: Locator, e: &mut DirEntry) -> bool {
    if vol.open_locator(l, e).await.is_err() {
        return false;
    }
    let mut cur = e.cursor();
    if vol.seek_dir(&mut cur, 0).await.is_err() {
        return false;
    }
    let mut best: Option<(u8, Locator)> = None;
    // Large folders: the first 4096 entries (a cover is usually early).
    for _ in 0..4096 {
        match vol.read_dir(&mut cur, e).await {
            Ok(true) => {}
            _ => break,
        }
        if e.is_dir() {
            continue;
        }
        if let Some(r) = cover_rank(e.name())
            && best.is_none_or(|b| r < b.0)
        {
            best = Some((r, e.locator()));
            if r == 0 {
                break;
            }
        }
    }
    match best {
        Some((_, at)) if !player_waiting() => vol.open_locator(at, e).await.is_ok(),
        _ => false,
    }
}

/// Decode the picture of `e` (embedded at `at`, else the whole file) into
/// the small and, if the arena allows, the big album art, and store them
/// under `key`. True if the small copy was stored.
async fn make_art(
    vol: &mut Vol,
    ring: &mut Ring,
    arena: &mut [u8],
    e: &DirEntry,
    at: Option<orpheus_meta::Located>,
    key: &ThumbKey,
) -> bool {
    let Ok(mut f) = vol.open_entry(e) else { return false };
    boost(true);
    let mut ok = false;
    // With the big copy first; again without it if the decoder needs the
    // memory (or the format does: PNG's window, GIF's canvas).
    for big in [true, false] {
        ring.reset(f.size());
        if let Some(at) = at
            && ring.seek(at.offset).is_err()
        {
            break;
        }
        if !ring.fill(vol, &mut f).await && ring.available() < 8 {
            break;
        }
        let mut head = [0u8; 8];
        let Some(fmt) = ring
            .peek(8)
            .map(|(a, b)| {
                let n = a.len().min(8);
                head[..n].copy_from_slice(&a[..n]);
                head[n..].copy_from_slice(&b[..8 - n]);
            })
            .and_then(|()| orpheus_img::probe(&head))
        else {
            break;
        };
        let big = big && matches!(fmt, Format::Jpeg | Format::Bmp | Format::Qoi);
        match decode_art(vol, ring, &mut f, arena, big, key).await {
            Ok(()) => {
                ok = true;
                break;
            }
            Err(Stop::Img(orpheus_img::Error::Arena { .. })) if big => {}
            Err(_) => break,
        }
    }
    boost(false);
    let _ = vol.close(f).await;
    ok
}

/// One decode of the picture at the ring's position into the art sizes,
/// then the cache writes.
async fn decode_art(
    vol: &mut Vol,
    ring: &mut Ring,
    f: &mut File,
    arena: &mut [u8],
    big: bool,
    key: &ThumbKey,
) -> Result<(), Stop> {
    let art = |kind: &CacheKind| Placement {
        fit: Fit::Cover,
        ..Placement::fit(u16::from(kind.w), u16::from(kind.h))
    };
    let oom = Stop::Img(orpheus_img::Error::Arena { needed: 0, capacity: arena.len() });
    let (small, rest) = carve_u16(arena, usize::from(ART.w) * usize::from(ART.h)).ok_or(Stop::Io)?;
    let (ws, rest) = carve_u32(rest, Scaler::need_words(usize::from(ART.w))).ok_or(Stop::Io)?;
    let mut s1 =
        Scaler::new(ws, art(&ART), Target { buf: &mut *small, stride: usize::from(ART.w), x: 0, y: 0 })
            .map_err(|_| Stop::Io)?;
    let (r, info, big_px) = if big {
        let (bpx, rest) = carve_u16(rest, usize::from(ART_BIG.w) * usize::from(ART_BIG.h)).ok_or(oom)?;
        let (wb, rest) = carve_u32(rest, Scaler::need_words(usize::from(ART_BIG.w))).ok_or(Stop::Io)?;
        let t = Target { buf: &mut *bpx, stride: usize::from(ART_BIG.w), x: 0, y: 0 };
        let mut s2 = Scaler::new(wb, art(&ART_BIG), t).map_err(|_| Stop::Io)?;
        let dec = decoder_in(rest).ok_or(Stop::Io)?;
        let r = run(vol, ring, f, dec, &mut Tee::new(&mut s1, &mut s2), player_waiting).await;
        (r, dec.info(), Some(bpx))
    } else {
        let dec = decoder_in(rest).ok_or(Stop::Io)?;
        let r = run(vol, ring, f, dec, &mut s1, player_waiting).await;
        (r, dec.info(), None)
    };
    r?;
    let info = info.ok_or(Stop::Io)?;
    let sat = |v: u32| u16::try_from(v).unwrap_or(u16::MAX).max(1);
    let meta = ThumbMeta { orig_w: sat(info.width), orig_h: sat(info.height), approximate: info.interlaced };
    if !store(vol, &ART, &sized(*key, &ART), &meta, small).await {
        return Err(Stop::Io);
    }
    if let Some(px) = big_px {
        store(vol, &ART_BIG, &sized(*key, &ART_BIG), &meta, px).await;
    }
    let _ = vol.flush().await;
    Ok(())
}
