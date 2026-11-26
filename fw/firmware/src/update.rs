//! Firmware update on the player (UPD-1, docs/plan/02-architecture.md
//! §2.5): the installer of `orpheus_update` over the HAL's guarded NOR
//! driver.
//!
//! * **From the card** (Settings › System › Update firmware): the card task
//!   (`media::Req::Update`) lists `/orpheus-*.ofw` and
//!   `/.orpheus/update/*.ofw`, reads one file's change list, and installs
//!   one: the header is checked, then the whole image is read and its
//!   SHA-256 compared (nothing written yet), then it is written to the
//!   inactive slot, read back and hashed, and the slots are switched. The
//!   UI restarts the player.
//! * **Over USB** (the developer channel's `UPDATE`, `orpheus_update::wire`;
//!   `xtask dev update`, the browser installer): `BEGIN` checks the header
//!   and erases the target's header sector, each `DATA` chunk is written
//!   inside its request, `COMMIT` starts the read-back, which the USB
//!   supervisor drives in steps ([`poll`]), then the switch. The host
//!   reboots the player.
//!
//! No RAM installer: in the full layout the update never writes the slot
//! the firmware pages from, so the running firmware does it. In the
//! dual-boot layout (one slot) it is refused (`Error::NotFull`).
//!
//! One installer at a time ([`OWNER`]). Every NOR write passes three
//! guards: the target slot (`slots::Target::allows`: inside the inactive
//! slot, or exactly the running slot's magic word), the HAL's full-layout
//! range (never sector 0, never `0x7E0000` and above), and the format
//! check before the first erase.
//!
//! **RAM** (SRAM0 `.bss`): the installer (`Option<Installer>`), the files
//! found and one change list. The UI reads the files only while the phase
//! is not `Searching` and the change list only while it is marked ready;
//! the card task writes them only before publishing those (Release/Acquire
//! on the atomics), so the two never overlap.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU32, Ordering};

use atj2157_hal::nor;
use orpheus_apps::services::{UPDATE_FILES, UpdError, UpdPhase, UpdateFile, UpdateInfo, UpdateRequest};
use orpheus_fs::{DirEntry, OpenMode};
use orpheus_proto::layout::SLOTS;
use orpheus_proto::slot::{HEADER_SIZE, SlotHeader};
use orpheus_update::install::{Flash, Installer, Phase};
use orpheus_update::wire::{self, State, Status};
use orpheus_update::{Digest, Sha256};
use orpheus_update::{Error, ofw, slots};

use crate::media::Vol;

/// Work for the card task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Scan,
    Details(u8),
    Install(u8),
}

/// The NOR as the installer sees it, through the HAL: reads are masked
/// 256-byte transactions there (a page fault elsewhere must not cut into
/// one; `orpheus_pager::bus`), erase and program mask themselves.
struct Nor;

impl Flash for Nor {
    type Error = nor::Error;
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), nor::Error> {
        nor::read(addr, buf)
    }
    fn erase_sector(&mut self, addr: u32) -> Result<(), nor::Error> {
        nor::erase_sector(addr)
    }
    fn program(&mut self, addr: u32, data: &[u8]) -> Result<(), nor::Error> {
        nor::program_page(addr, data)
    }
}

const FREE: u8 = 0;
const USB: u8 = 1;
const CARD: u8 = 2;
/// Who holds the installer.
static OWNER: AtomicU8 = AtomicU8::new(FREE);
/// The installer; only the [`OWNER`] touches it.
static mut INST: Option<Installer> = None;

/// Battery needed without the cable (as the UI checks).
const MIN_BATTERY: u8 = orpheus_apps::screens::UPDATE_MIN_BATTERY;

// ---------------------------------------------------------------- UI state

static PHASE: AtomicU8 = AtomicU8::new(0);
static ERROR: AtomicU8 = AtomicU8::new(0);
static PERMILLE: AtomicU16 = AtomicU16::new(0);
static SEQ: AtomicU32 = AtomicU32::new(0);
static COUNT: AtomicU8 = AtomicU8::new(0);
static CANCEL: AtomicBool = AtomicBool::new(false);
/// File index whose change list is in [`CHANGES`]; [`NOT_READY`] while
/// one is being read.
static CH_READY: AtomicU8 = AtomicU8::new(NOT_READY);
const NOT_READY: u8 = 0xFF;

const PATH_MAX: usize = 48;

#[derive(Clone, Copy)]
struct Found {
    path: [u8; PATH_MAX],
    path_len: u8,
    /// Where the name starts in `path`.
    name_at: u8,
    version: [u8; ofw::VERSION_LEN],
    version_len: u8,
    installed: bool,
    size: u32,
}

const NONE: Found = Found {
    path: [0; PATH_MAX],
    path_len: 0,
    name_at: 0,
    version: [0; ofw::VERSION_LEN],
    version_len: 0,
    installed: false,
    size: 0,
};
static mut FILES: [Found; UPDATE_FILES] = [NONE; UPDATE_FILES];
static mut CHANGES: ([u8; ofw::CHANGES_MAX], u16) = ([0; ofw::CHANGES_MAX], 0);

const fn phase_code(p: UpdPhase) -> u8 {
    match p {
        UpdPhase::Idle => 0,
        UpdPhase::Searching => 1,
        UpdPhase::Listed => 2,
        UpdPhase::Checking => 3,
        UpdPhase::Writing => 4,
        UpdPhase::Verifying => 5,
        UpdPhase::Done => 6,
        UpdPhase::Failed => 7,
    }
}

const fn phase_of(c: u8) -> UpdPhase {
    match c {
        1 => UpdPhase::Searching,
        2 => UpdPhase::Listed,
        3 => UpdPhase::Checking,
        4 => UpdPhase::Writing,
        5 => UpdPhase::Verifying,
        6 => UpdPhase::Done,
        7 => UpdPhase::Failed,
        _ => UpdPhase::Idle,
    }
}

const fn err_code(e: UpdError) -> u8 {
    match e {
        UpdError::None => 0,
        UpdError::Damaged => 1,
        UpdError::WrongPlayer => 2,
        UpdError::Battery => 3,
        UpdError::Card => 4,
        UpdError::Write => 5,
        UpdError::Busy => 6,
    }
}

const fn err_of(c: u8) -> UpdError {
    match c {
        1 => UpdError::Damaged,
        2 => UpdError::WrongPlayer,
        3 => UpdError::Battery,
        4 => UpdError::Card,
        5 => UpdError::Write,
        6 => UpdError::Busy,
        _ => UpdError::None,
    }
}

fn set_phase(p: UpdPhase) {
    PHASE.store(phase_code(p), Ordering::Release);
    SEQ.fetch_add(1, Ordering::Relaxed);
}

fn progress(permille: u16) {
    if PERMILLE.swap(permille, Ordering::Relaxed) != permille {
        SEQ.fetch_add(1, Ordering::Relaxed);
    }
}

/// What the Update screen shows.
pub fn info() -> UpdateInfo {
    let phase = phase_of(PHASE.load(Ordering::Acquire));
    UpdateInfo {
        phase,
        files: if phase == UpdPhase::Searching { 0 } else { COUNT.load(Ordering::Relaxed) },
        permille: PERMILLE.load(Ordering::Relaxed),
        error: err_of(ERROR.load(Ordering::Relaxed)),
        seq: SEQ.load(Ordering::Relaxed),
    }
}

/// File `i` of the last search (none while searching).
pub fn file(i: usize) -> Option<UpdateFile<'static>> {
    if phase_of(PHASE.load(Ordering::Acquire)) == UpdPhase::Searching
        || i >= usize::from(COUNT.load(Ordering::Relaxed))
    {
        return None;
    }
    // SAFETY: the card task writes `FILES` only while the phase is
    // `Searching` (set by `request` before the scan is queued, checked by
    // `scan`), and publishes with a Release store of the phase; we read
    // only after an Acquire load saw another phase.
    let f = unsafe { &(*(&raw const FILES))[i] };
    let path = core::str::from_utf8(&f.path[..usize::from(f.path_len)]).ok()?;
    Some(UpdateFile {
        name: path.get(usize::from(f.name_at)..)?,
        version: core::str::from_utf8(&f.version[..usize::from(f.version_len)]).ok()?,
        size: f.size,
        installed: f.installed,
    })
}

/// The file whose change list the UI asked for last.
static WANT: AtomicU8 = AtomicU8::new(NOT_READY);

/// The change list asked for last (`""` until it is read).
pub fn changes() -> &'static str {
    let want = WANT.load(Ordering::Relaxed);
    if want == NOT_READY || CH_READY.load(Ordering::Acquire) != want {
        return "";
    }
    // SAFETY: written by the card task only after it stored NOT_READY and
    // before it stores the index (Release); a Details request for the same
    // index while it is ready is not carried out again (`details`).
    let c = unsafe { &*(&raw const CHANGES) };
    core::str::from_utf8(&c.0[..usize::from(c.1)]).unwrap_or("")
}

/// The UI's requests (`UpdateService::update_request`).
pub fn request(r: UpdateRequest) {
    let busy = OWNER.load(Ordering::Acquire) != FREE;
    match r {
        UpdateRequest::Scan => {
            if busy
                && matches!(
                    phase_of(PHASE.load(Ordering::Acquire)),
                    UpdPhase::Checking | UpdPhase::Writing | UpdPhase::Verifying | UpdPhase::Done
                )
            {
                return;
            }
            ERROR.store(0, Ordering::Relaxed);
            set_phase(UpdPhase::Searching);
            if !crate::media::try_request(crate::media::Req::Update(Op::Scan)) {
                fail(UpdError::Card);
            }
        }
        UpdateRequest::Details(i) => {
            WANT.store(i, Ordering::Relaxed);
            if CH_READY.load(Ordering::Acquire) != i {
                CH_READY.store(NOT_READY, Ordering::Release);
                let _ = crate::media::try_request(crate::media::Req::Update(Op::Details(i)));
            }
        }
        UpdateRequest::Install(i) => {
            if busy {
                fail(UpdError::Busy);
                return;
            }
            CANCEL.store(false, Ordering::Relaxed);
            ERROR.store(0, Ordering::Relaxed);
            PERMILLE.store(0, Ordering::Relaxed);
            set_phase(UpdPhase::Checking);
            if !crate::media::try_request(crate::media::Req::Update(Op::Install(i))) {
                fail(UpdError::Card);
            }
        }
        UpdateRequest::Cancel => CANCEL.store(true, Ordering::Relaxed),
    }
}

fn fail(e: UpdError) {
    ERROR.store(err_code(e), Ordering::Relaxed);
    set_phase(UpdPhase::Failed);
}

/// Stage 2's record of the slot it started (boot-info word: magic `B`,
/// path 1 = firmware, slot index in the low byte), as `main::find_slot`.
fn booted() -> Option<u8> {
    // SAFETY: persistent boot-info word written by stage 2 (the last word
    // of the mailbox).
    let bi = unsafe {
        core::ptr::read_volatile(
            (orpheus_proto::mailbox::BASE + orpheus_proto::mailbox::OFF_BOOT_INFO as u32) as *const u32,
        )
    };
    (bi >> 24 == 0x42 && (bi >> 16) & 0xFF == 1).then_some((bi & 0xFF) as u8)
}

/// The build id of the slot the firmware runs from (`INFO`, the Update
/// screen's "installed" mark).
pub fn running_build() -> Option<[u8; 16]> {
    let base = *SLOTS.get(usize::from(booted()?))?;
    let mut h = [0u8; HEADER_SIZE];
    Nor.read(base, &mut h).ok()?;
    // Stage 2 may have started a superseded slot as a fallback (T-6).
    SlotHeader::from_bytes_or_superseded(&h).ok().map(|s| s.build_id)
}

fn target() -> Result<slots::Target, Error> {
    slots::choose(nor::full_layout(), booted())
}

fn battery_ok() -> bool {
    let s = crate::state::snapshot();
    s.cable || s.percent >= MIN_BATTERY
}

// ---------------------------------------------------------------- card

/// The card task's part (`media::Server::serve`). `buf`: free arena, at
/// least 4 KiB (the idle arena when installing).
pub async fn card_op(vol: &mut Vol, op: Op, buf: &mut [u8]) {
    match op {
        Op::Scan => scan(vol, buf).await,
        Op::Details(i) => details(vol, i, buf).await,
        Op::Install(i) => install(vol, i, buf).await,
    }
}

/// Read the `.ofw` header and the slot header (512 bytes) of `path`.
async fn head(vol: &mut Vol, path: &str, buf: &mut [u8]) -> Option<(orpheus_fs::File, u64)> {
    let need = ofw::HEADER_SIZE + HEADER_SIZE;
    let mut f = vol.open(path, OpenMode::Read).await.ok()?;
    let size = f.size();
    let n = vol.read_exact_or_eof(&mut f, buf.get_mut(..need)?).await.ok();
    if n != Some(need) {
        let _ = vol.close(f).await;
        return None;
    }
    Some((f, size))
}

fn is_update_name(name: &str, top: bool) -> bool {
    let b = name.as_bytes();
    let ext = b.len() > 4 && b[b.len() - 4..].eq_ignore_ascii_case(b".ofw");
    ext && (!top || (b.len() > 8 && b[..8].eq_ignore_ascii_case(b"orpheus-")))
}

async fn scan(vol: &mut Vol, buf: &mut [u8]) {
    if phase_of(PHASE.load(Ordering::Acquire)) != UpdPhase::Searching {
        return;
    }
    let running = running_build();
    let mut n = 0usize;
    for (dir, top) in [("/", true), ("/.orpheus/update", false)] {
        let Ok(mut cur) = vol.open_dir(dir).await else { continue };
        let mut e = DirEntry::new();
        // Names first (the directory cursor is in use while reading).
        let mut names = [[0u8; PATH_MAX]; UPDATE_FILES];
        let mut lens = [0usize; UPDATE_FILES];
        let mut k = 0;
        while let Ok(true) = vol.read_dir(&mut cur, &mut e).await {
            if k + n >= UPDATE_FILES {
                break;
            }
            let name = e.name();
            let prefix = if top { "/" } else { "/.orpheus/update/" };
            if e.is_dir() || !is_update_name(name, top) || prefix.len() + name.len() > PATH_MAX {
                continue;
            }
            names[k][..prefix.len()].copy_from_slice(prefix.as_bytes());
            names[k][prefix.len()..prefix.len() + name.len()].copy_from_slice(name.as_bytes());
            lens[k] = prefix.len() + name.len();
            k += 1;
        }
        for j in 0..k {
            let Ok(path) = core::str::from_utf8(&names[j][..lens[j]]) else { continue };
            let Some((f, size)) = head(vol, path, buf).await else { continue };
            let _ = vol.close(f).await;
            let Ok((h, s)) = ofw::check(&buf[..ofw::HEADER_SIZE], &buf[ofw::HEADER_SIZE..]) else { continue };
            if size != ofw::HEADER_SIZE as u64 + u64::from(h.image_len) {
                continue;
            }
            let v = h.version_str().unwrap_or("?");
            let mut found = NONE;
            found.path[..lens[j]].copy_from_slice(&names[j][..lens[j]]);
            found.path_len = lens[j] as u8;
            found.name_at = path.rfind('/').map_or(0, |p| p + 1) as u8;
            found.version[..v.len()].copy_from_slice(v.as_bytes());
            found.version_len = v.len() as u8;
            found.installed = running == Some(s.build_id);
            found.size = h.image_len;
            // SAFETY: see `file`: only while the phase is `Searching`.
            unsafe { (*(&raw mut FILES))[n] = found };
            n += 1;
        }
    }
    COUNT.store(n as u8, Ordering::Relaxed);
    set_phase(UpdPhase::Listed);
}

async fn details(vol: &mut Vol, i: u8, buf: &mut [u8]) {
    if CH_READY.load(Ordering::Acquire) == i {
        return;
    }
    if file(usize::from(i)).is_none() {
        return;
    }
    let mut path = [0u8; PATH_MAX];
    // SAFETY: see `file`.
    let fl = unsafe { (*(&raw const FILES))[usize::from(i)] };
    path[..usize::from(fl.path_len)].copy_from_slice(&fl.path[..usize::from(fl.path_len)]);
    let Ok(p) = core::str::from_utf8(&path[..usize::from(fl.path_len)]) else { return };
    let Some((file, _)) = head(vol, p, buf).await else { return };
    let _ = vol.close(file).await;
    let text = ofw::changes(&buf[..ofw::HEADER_SIZE]);
    CH_READY.store(NOT_READY, Ordering::Release);
    // SAFETY: NOT_READY is stored: the UI does not read `CHANGES` now.
    unsafe {
        let c = &mut *(&raw mut CHANGES);
        c.0[..text.len()].copy_from_slice(text.as_bytes());
        c.1 = text.len() as u16;
    }
    CH_READY.store(i, Ordering::Release);
    SEQ.fetch_add(1, Ordering::Relaxed);
}

/// Hand the CPU to thread mode for a moment: the card task runs on the
/// interrupt executor, and the long hash and NOR loops must not starve the
/// UI and the watchdog's task.
async fn breathe() {
    embassy_time::Timer::after_millis(1).await;
}

/// Bytes read from the card per step.
const STEP: usize = 4096;

async fn install(vol: &mut Vol, i: u8, buf: &mut [u8]) {
    if OWNER.compare_exchange(FREE, CARD, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return fail(UpdError::Busy);
    }
    let r = install_inner(vol, i, buf).await;
    match r {
        Ok(true) => {
            // Done: the installer stays owned until the restart.
            set_phase(UpdPhase::Done);
            atj2157_hal::log::line(b"update: installed, restart pending");
            return;
        }
        Ok(false) => set_phase(UpdPhase::Listed),
        Err(e) => {
            atj2157_hal::log::line(b"update: failed");
            fail(e);
        }
    }
    // SAFETY: this task owns the installer (OWNER = CARD).
    unsafe { *(&raw mut INST) = None };
    OWNER.store(FREE, Ordering::Release);
}

/// `Ok(true)` installed, `Ok(false)` cancelled.
async fn install_inner(vol: &mut Vol, i: u8, buf: &mut [u8]) -> Result<bool, UpdError> {
    if !battery_ok() {
        return Err(UpdError::Battery);
    }
    if buf.len() < STEP {
        return Err(UpdError::Card);
    }
    // SAFETY: see `file`: the list is stable outside `Searching`.
    let fl = unsafe { (*(&raw const FILES)).get(usize::from(i)).copied() }.ok_or(UpdError::Card)?;
    let mut path = [0u8; PATH_MAX];
    path[..usize::from(fl.path_len)].copy_from_slice(&fl.path[..usize::from(fl.path_len)]);
    let p = core::str::from_utf8(&path[..usize::from(fl.path_len)]).map_err(|_| UpdError::Card)?;
    let (mut f, size) = head(vol, p, buf).await.ok_or(UpdError::Card)?;
    let r = install_file(vol, &mut f, size, buf).await;
    let _ = vol.close(f).await;
    r
}

async fn install_file(
    vol: &mut Vol,
    f: &mut orpheus_fs::File,
    size: u64,
    buf: &mut [u8],
) -> Result<bool, UpdError> {
    let mut hdr = [0u8; ofw::HEADER_SIZE + HEADER_SIZE];
    let n = hdr.len();
    hdr.copy_from_slice(&buf[..n]);
    let (h, _) = ofw::check(&hdr[..ofw::HEADER_SIZE], &hdr[ofw::HEADER_SIZE..]).map_err(UpdError::from)?;
    if size != ofw::HEADER_SIZE as u64 + u64::from(h.image_len) {
        return Err(UpdError::Damaged);
    }
    let t = target().map_err(UpdError::from)?;
    let inst =
        Installer::new(&hdr[..ofw::HEADER_SIZE], &hdr[ofw::HEADER_SIZE..], t).map_err(UpdError::from)?;
    let len = h.image_len as usize;

    // 1. The whole image's SHA-256, before anything is written.
    let mut sha = Sha256::new();
    let mut off = 0usize;
    f.set_position(ofw::HEADER_SIZE as u64);
    while off < len {
        if CANCEL.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let n = (len - off).min(STEP);
        let got = vol.read_exact_or_eof(f, &mut buf[..n]).await.map_err(|_| UpdError::Card)?;
        if got != n {
            return Err(UpdError::Damaged);
        }
        sha.update(&buf[..n]);
        off += n;
        progress(((off as u64 * 1000) / len as u64) as u16);
        breathe().await;
    }
    let digest: [u8; 32] = sha.finalize().into();
    if digest != h.sha256 {
        return Err(UpdError::Damaged);
    }

    // 2. Write. From here on the target's header sector is erased.
    // SAFETY: this task owns the installer (OWNER = CARD).
    let inst = unsafe { (*(&raw mut INST)).insert(inst) };
    inst.start(&mut Nor).map_err(UpdError::from)?;
    set_phase(UpdPhase::Writing);
    progress(0);
    f.set_position(ofw::HEADER_SIZE as u64);
    let mut off = 0usize;
    while off < len {
        let n = (len - off).min(STEP);
        let got = vol.read_exact_or_eof(f, &mut buf[..n]).await.map_err(|_| UpdError::Card)?;
        if got != n {
            return Err(UpdError::Card);
        }
        inst.write(&mut Nor, off as u32, &buf[..n]).map_err(UpdError::from)?;
        off += n;
        progress(inst.permille());
        breathe().await;
    }
    if inst.phase() != Phase::Verifying {
        return Err(UpdError::Damaged);
    }

    // 3. Read back, 4. switch.
    set_phase(UpdPhase::Verifying);
    while !inst.verify_step(&mut Nor, &mut buf[..STEP]).map_err(UpdError::from)? {
        progress(inst.permille());
        breathe().await;
    }
    inst.commit(&mut Nor).map_err(UpdError::from)?;
    progress(1000);
    Ok(true)
}

// ---------------------------------------------------------------- USB

/// The developer channel's `UPDATE` OUT (`orpheus_update::wire`). Runs in
/// the USB task; the NOR work of one chunk (an erase, sixteen programs) is
/// done inside the request.
pub fn usb_out(op: u16, index: u16, data: &[u8]) -> bool {
    match op {
        wire::OP_BEGIN => usb_begin(data),
        wire::OP_DATA => with_usb(|inst| inst.write(&mut Nor, u32::from(index) * wire::CHUNK as u32, data)),
        wire::OP_COMMIT => with_usb(|inst| {
            if inst.phase() == Phase::Verifying {
                USB_COMMIT.store(true, Ordering::Release);
                Ok(())
            } else {
                Err(Error::State)
            }
        }),
        wire::OP_ABORT => {
            USB_COMMIT.store(false, Ordering::Release);
            if OWNER.load(Ordering::Acquire) == USB {
                // SAFETY: the USB task owns the installer.
                unsafe { *(&raw mut INST) = None };
                OWNER.store(FREE, Ordering::Release);
            }
            true
        }
        _ => false,
    }
}

/// Last error of a USB update (for the status when no installer is left).
static USB_ERROR: AtomicU8 = AtomicU8::new(0);
/// The host sent `COMMIT`: [`poll`] reads back and switches.
static USB_COMMIT: AtomicBool = AtomicBool::new(false);

fn usb_begin(data: &[u8]) -> bool {
    let owner = OWNER.load(Ordering::Acquire);
    if owner == CARD {
        USB_ERROR.store(Error::Busy as u8, Ordering::Relaxed);
        return false;
    }
    if owner == FREE && OWNER.compare_exchange(FREE, USB, Ordering::AcqRel, Ordering::Acquire).is_err() {
        USB_ERROR.store(Error::Busy as u8, Ordering::Relaxed);
        return false;
    }
    // A new BEGIN replaces an earlier USB update (its target was never
    // made valid unless it finished).
    USB_COMMIT.store(false, Ordering::Release);
    let r = (|| {
        if data.len() != wire::BEGIN_LEN {
            return Err(Error::Short);
        }
        let t = target()?;
        let mut inst = Installer::new(&data[..ofw::HEADER_SIZE], &data[ofw::HEADER_SIZE..], t)?;
        inst.start(&mut Nor)?;
        Ok(inst)
    })();
    match r {
        Ok(inst) => {
            // SAFETY: the USB task owns the installer.
            unsafe { *(&raw mut INST) = Some(inst) };
            USB_ERROR.store(0, Ordering::Relaxed);
            atj2157_hal::log::line(b"update: usb begin");
            true
        }
        Err(e) => {
            // SAFETY: as above.
            unsafe { *(&raw mut INST) = None };
            USB_ERROR.store(e as u8, Ordering::Relaxed);
            OWNER.store(FREE, Ordering::Release);
            false
        }
    }
}

fn with_usb(f: impl FnOnce(&mut Installer) -> Result<(), Error>) -> bool {
    if OWNER.load(Ordering::Acquire) != USB {
        USB_ERROR.store(Error::State as u8, Ordering::Relaxed);
        return false;
    }
    // SAFETY: the USB task owns the installer.
    let Some(inst) = (unsafe { (*(&raw mut INST)).as_mut() }) else { return false };
    match f(inst) {
        Ok(()) => true,
        Err(e) => {
            USB_ERROR.store(e as u8, Ordering::Relaxed);
            false
        }
    }
}

/// Bytes read back per [`poll`] (in 256-byte pieces).
const POLL_BYTES: usize = 16 * 1024;

/// A USB update is committed and its read-back or switch not done: [`poll`]
/// has work (the USB supervisor then runs it every 20 ms). In the USB task.
pub fn usb_busy() -> bool {
    if OWNER.load(Ordering::Acquire) != USB || !USB_COMMIT.load(Ordering::Acquire) {
        return false;
    }
    // SAFETY: the USB task owns the installer; this runs in it.
    unsafe { (*(&raw const INST)).as_ref() }
        .is_some_and(|i| matches!(i.phase(), Phase::Verifying | Phase::Commit))
}

/// Drive a USB update's read-back and switch (the USB supervisor, every
/// 20 ms while [`usb_busy`]).
pub fn poll() {
    if OWNER.load(Ordering::Acquire) != USB {
        return;
    }
    // SAFETY: the USB task owns the installer; `poll` runs in it.
    let Some(inst) = (unsafe { (*(&raw mut INST)).as_mut() }) else { return };
    if !USB_COMMIT.load(Ordering::Acquire) {
        return;
    }
    if inst.phase() == Phase::Verifying {
        let mut buf = [0u8; 256];
        let mut done = 0;
        while done < POLL_BYTES {
            match inst.verify_step(&mut Nor, &mut buf) {
                Ok(true) => break,
                Ok(false) => done += buf.len(),
                Err(e) => {
                    USB_ERROR.store(e as u8, Ordering::Relaxed);
                    return;
                }
            }
        }
    }
    if inst.phase() == Phase::Commit {
        match inst.commit(&mut Nor) {
            Ok(()) => atj2157_hal::log::line(b"update: usb installed"),
            Err(e) => USB_ERROR.store(e as u8, Ordering::Relaxed),
        }
    }
}

/// The developer channel's `UPDATE` IN.
pub fn usb_status(out: &mut [u8]) -> usize {
    let owner = OWNER.load(Ordering::Acquire);
    let mut s = Status {
        state: State::Idle,
        error: USB_ERROR.load(Ordering::Relaxed),
        slot: 0xFF,
        received: 0,
        verified: 0,
        image_len: 0,
    };
    if owner == CARD {
        s.state = State::Card;
    } else if owner == USB {
        // SAFETY: read in the USB task, which owns the installer.
        if let Some(inst) = unsafe { (*(&raw const INST)).as_ref() } {
            s.state = match inst.phase() {
                Phase::Ready | Phase::Writing => State::Writing,
                Phase::Verifying if USB_COMMIT.load(Ordering::Acquire) => State::Verifying,
                Phase::Verifying => State::Writing,
                Phase::Commit => State::Committing,
                Phase::Done => State::Done,
                Phase::Failed(e) => {
                    s.error = e as u8;
                    State::Failed
                }
            };
            s.slot = inst.target().index;
            s.received = inst.received();
            s.verified = inst.verified();
            s.image_len = inst.image_len();
        }
    } else if s.error != 0 {
        s.state = State::Failed;
    }
    let b = s.to_bytes();
    let n = out.len().min(b.len());
    out[..n].copy_from_slice(&b[..n]);
    n
}
