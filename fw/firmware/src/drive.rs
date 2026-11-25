//! USB Drive (USB-6, 06 §3, 05 §7.10): an image file on the card presented
//! to a computer as a CD-ROM or a disk.
//!
//! The request (the app's `drive_start`, or the developer command `usb
//! image <path> [cd|disk|fixed] [ro|rw]`) is kept here; the USB task asks
//! the card task for the image (`media::Req::Image`). The card task stops
//! playback, opens the file, applies the export rules
//! (`orpheus_usb::msc::image::check`: a CD is read-only, a disk is
//! read-only unless the file is in `/.orpheus/` and the player is in test
//! mode, CLAUDE.md rule 5) and maps its runs into the extent table, which
//! lives in the decoder arena after the USB transfer buffer
//! (`audio::usb_extents`). Then it hands the card over exactly as for the
//! card reader (`audio::release`) and the USB task serves the computer's
//! SCSI/MMC commands through `ImageDevice`, which reads and writes only the
//! card sectors of the file's runs. Back in the app (or a new personality)
//! ends it; the card returns re-initialised and is mounted again.
//!
//! The card's file system is not mounted while the image is presented:
//! the computer may write the image's clusters (writable test images
//! only), never its size or any other cluster.

use core::cell::Cell;
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

use critical_section::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use orpheus_apps::services::{DriveInfo, DrivePhase, DriveRefusal, FileRef};
use orpheus_fs::{DirEntry, Locator};
use orpheus_usb::msc::extent::{Extent, ExtentTable, POOL_EXTENTS};
use orpheus_usb::msc::image::{self, Export, Request, TEST_DIR, USER_DIR};

use crate::audio::reply;
use crate::media::Vol;

macro_rules! t {
    ($s:literal) => {{
        static S: [u8; $s.len()] = *$s;
        let s: &'static [u8] = &S;
        s
    }};
}

/// Longest path the developer command takes.
pub const PATH_LEN: usize = 96;
/// Longest name kept for the status screen.
pub const NAME_LEN: usize = 48;

/// Which file.
#[derive(Clone, Copy)]
pub enum Source {
    /// A file of a listing (the app).
    File(FileRef),
    /// A path (the developer command).
    Path([u8; PATH_LEN], u8),
}

/// What was asked for.
#[derive(Clone, Copy)]
pub struct Want {
    pub src: Source,
    pub req: Request,
    /// Writable exports of test images are allowed (the developer channel,
    /// or the app with the developer rows shown).
    pub test_mode: bool,
}

/// The mapped image the card task hands to the USB task with the card.
#[derive(Clone, Copy, Debug)]
pub struct Mapped {
    pub export: Export,
    /// Runs in `audio::usb_extents`.
    pub runs: usize,
}

static WANT: Mutex<Cell<Option<Want>>> = Mutex::new(Cell::new(None));
static INFO: Mutex<Cell<DriveInfo>> = Mutex::new(Cell::new(DriveInfo {
    phase: DrivePhase::Off,
    medium: image::Medium::RemovableDisk,
    read_only: true,
    size: 0,
    bytes_read: 0,
    bytes_written: 0,
    host: false,
    refusal: None,
    runs: 0,
}));
static NAME: Mutex<Cell<([u8; NAME_LEN], u8)>> = Mutex::new(Cell::new(([0; NAME_LEN], 0)));
/// Bumped when the phase, the host or the name changed (the UI sends an
/// event; the counters it reads on its own ticks).
pub static GEN: AtomicU32 = AtomicU32::new(0);
/// The card task's answer to `Req::Image`.
pub static MAPPED: Signal<CriticalSectionRawMutex, Result<Mapped, DriveRefusal>> = Signal::new();
/// First clusters of `/Images` and `/.orpheus/images` (0: none), found at
/// mount.
static DIRS: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

/// The export as the UI sees it.
#[must_use]
pub fn info() -> DriveInfo {
    critical_section::with(|cs| INFO.borrow(cs).get())
}

/// Change the export's state (bumps [`GEN`] when the UI should look).
pub fn update(f: impl FnOnce(&mut DriveInfo)) {
    let (before, after) = critical_section::with(|cs| {
        let c = INFO.borrow(cs);
        let mut i = c.get();
        let b = i;
        f(&mut i);
        c.set(i);
        (b, i)
    });
    if (before.phase, before.host, before.refusal) != (after.phase, after.host, after.refusal) {
        GEN.fetch_add(1, Relaxed);
    }
}

/// The presented file's name into `out`.
pub fn name<const N: usize>(out: &mut orpheus_ui::text::StrBuf<N>) {
    let (b, n) = critical_section::with(|cs| NAME.borrow(cs).get());
    out.clear();
    out.push_str_or_ellipsis(core::str::from_utf8(&b[..usize::from(n)]).unwrap_or(""));
}

fn set_name(s: &str) {
    let mut b = [0u8; NAME_LEN];
    // Cut at a character boundary.
    let mut n = s.len().min(NAME_LEN);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    b[..n].copy_from_slice(&s.as_bytes()[..n]);
    critical_section::with(|cs| NAME.borrow(cs).set((b, n as u8)));
    GEN.fetch_add(1, Relaxed);
}

/// The folders the app lists.
#[must_use]
pub fn dirs() -> (u32, u32) {
    (DIRS[0].load(Relaxed), DIRS[1].load(Relaxed))
}

/// At mount and after the card came back: where the image folders are.
pub async fn find_dirs(vol: &mut Vol) {
    let mut e = DirEntry::new();
    for (i, p) in [USER_DIR, TEST_DIR].iter().enumerate() {
        let c = if vol.stat(p, &mut e).await.is_ok() && e.is_dir() { e.first_cluster() } else { 0 };
        DIRS[i].store(c, Relaxed);
    }
}

/// Ask for an export (the USB task is told by the caller: `MODE`).
/// `false` while one is running.
pub fn want(w: Want, name: &str) -> bool {
    if info().phase.is_active() {
        return false;
    }
    critical_section::with(|cs| WANT.borrow(cs).set(Some(w)));
    set_name(name);
    update(|i| {
        *i = DriveInfo {
            phase: DrivePhase::Preparing,
            medium: w.req.medium,
            read_only: !w.req.write,
            ..DriveInfo::default()
        };
    });
    true
}

/// The app's Back on a refused export: forget it.
pub fn clear_failed() {
    update(|i| {
        if i.phase == DrivePhase::Failed {
            i.phase = DrivePhase::Off;
            i.refusal = None;
        }
    });
}

/// Card task: open the wanted file, check it, map its runs into
/// `storage` (the arena's extent area). On `Ok` the caller hands the card
/// to the USB task.
pub async fn map(vol: &mut Vol, storage: &mut [Extent]) -> Result<Mapped, DriveRefusal> {
    let Some(w) = critical_section::with(|cs| WANT.borrow(cs).take()) else {
        return Err(DriveRefusal::NotFound);
    };
    let mut e = DirEntry::new();
    let under_orpheus = match w.src {
        Source::File(f) => {
            vol.open_locator(Locator { dir: f.dir, index: f.index }, &mut e)
                .await
                .map_err(|_| DriveRefusal::NotFound)?;
            // In the test folder (found again now: the app may be older
            // than the last remount).
            let mut d = DirEntry::new();
            vol.stat(TEST_DIR, &mut d).await.is_ok() && d.is_dir() && d.first_cluster() == f.dir
        }
        Source::Path(b, n) => {
            let p = core::str::from_utf8(&b[..usize::from(n)]).map_err(|_| DriveRefusal::NotFound)?;
            vol.stat(p, &mut e).await.map_err(|_| DriveRefusal::NotFound)?;
            image::under_orpheus(p)
        }
    };
    if e.is_dir() {
        return Err(DriveRefusal::NotFound);
    }
    let f = vol.open_entry(&e).map_err(|_| DriveRefusal::Io)?;
    let export = image::check(w.req, under_orpheus, w.test_mode, f.size(), f.valid_length())?;
    let mut t = ExtentTable::new(storage);
    image::map_file(vol, &f, &mut t).await?;
    Ok(Mapped { export, runs: t.len() })
}

/// The extent area as a table's storage (`POOL_EXTENTS` runs, 8 KiB).
pub fn as_extents(area: &mut [u8]) -> &mut [Extent] {
    // SAFETY: `Extent` is three `u32` (`repr(C)`), valid for any bits;
    // `align_to_mut` gives only the aligned middle part.
    let (_, ext, _) = unsafe { area.align_to_mut::<Extent>() };
    let n = ext.len().min(POOL_EXTENTS);
    &mut ext[..n]
}

/// `usb image <path> [cd|disk|fixed] [ro|rw]` and `usb image status`
/// (developer channel). Starting is the caller's: `true` = switch to the
/// image personality.
pub fn test_command(rest: &[u8]) -> bool {
    let text = core::str::from_utf8(rest).unwrap_or("");
    if text.trim() == "status" {
        status_reply();
        return false;
    }
    let Some((path, req)) = image::parse_command(text) else {
        reply(|w| {
            w.s(t!(b"usage: usb image <path> [cd|disk|fixed] [ro|rw] | usb image status"));
        });
        return false;
    };
    if path.len() > PATH_LEN {
        reply(|w| {
            w.s(t!(b"usb image: path too long"));
        });
        return false;
    }
    let mut b = [0u8; PATH_LEN];
    b[..path.len()].copy_from_slice(path.as_bytes());
    let name = path.rsplit('/').next().unwrap_or(path);
    // The developer channel is the test path: writable test images allowed
    // (still only under /.orpheus/, checked by the card task).
    let w = Want { src: Source::Path(b, path.len() as u8), req, test_mode: true };
    if !want(w, name) {
        reply(|w| {
            w.s(t!(b"usb image: already presenting"));
        });
        return false;
    }
    reply(|w| {
        w.s(t!(b"usb image: preparing ")).s(req.medium.word().as_bytes()).s(if req.write {
            t!(b" rw")
        } else {
            t!(b" ro")
        });
    });
    true
}

/// `usb image status`: phase, medium, access, size, runs, counters.
pub fn status_reply() {
    let i = info();
    let phase: &[u8] = match i.phase {
        DrivePhase::Off => t!(b"off"),
        DrivePhase::Preparing => t!(b"preparing"),
        DrivePhase::Attached if i.host => t!(b"attached host"),
        DrivePhase::Attached => t!(b"attached"),
        DrivePhase::Failed => t!(b"failed"),
    };
    reply(|w| {
        w.s(t!(b"image ")).s(phase).s(t!(b" medium=")).s(i.medium.word().as_bytes());
        w.s(if i.read_only { t!(b" ro") } else { t!(b" rw") });
        w.s(t!(b" size=")).u(i.size).s(t!(b" runs=")).u(u64::from(i.runs));
        w.s(t!(b" read=")).u(i.bytes_read).s(t!(b" written=")).u(i.bytes_written);
        if let Some(r) = i.refusal {
            w.s(t!(b" refused=")).s(r.text().as_bytes());
        }
    });
}
