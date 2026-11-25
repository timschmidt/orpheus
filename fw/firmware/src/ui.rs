//! The UI task: key mapper → shell → screens → frame buffer → panel.

use atj2157_hal::dma;
use embassy_futures::select::{Either4, select4};
use embassy_time::{Duration, Instant as TInstant, Timer};
use orpheus_apps::screens::{self, AppScreen};
use orpheus_apps::services::{self, PowerRequest};
use orpheus_input::Key as HwKey;
use orpheus_ui::event::{Event, Instant, Key, ServiceEvent};
use orpheus_ui::gfx::{DirtySet, Gfx};
use orpheus_ui::input::KeyMapper;
use orpheus_ui::nav::Arg;
use orpheus_ui::shell::{Requests, Shell, TICK_MS};
use yoton_bsp::display::{self, Backlight, Display};
use yoton_bsp::settings::{NorRegion, Store};

use crate::services::{Action, Fw};
use crate::{BATTERY, KEYS, state};

fn map_key(k: HwKey) -> Key {
    match k {
        HwKey::Power => Key::Power,
        HwKey::Back => Key::Back,
        HwKey::Menu => Key::Menu,
        HwKey::Ok => Key::Ok,
        HwKey::Prev => Key::Prev,
        HwKey::Next => Key::Next,
        HwKey::Up => Key::Up,
        HwKey::Down => Key::Down,
        HwKey::VolUp => Key::VolUp,
        HwKey::VolDown => Key::VolDown,
    }
}

fn now() -> Instant {
    Instant(TInstant::now().as_millis())
}

struct Ui {
    fw: &'static mut Fw,
    shell: Shell<AppScreen, Fw>,
    mapper: KeyMapper,
    disp: Display,
    bl: Backlight,
    screen_on: bool,
    store: &'static mut Option<Store<NorRegion, 4>>,
    /// Sound settings last published to the audio task.
    sound: Option<crate::media::Sound>,
}

impl Ui {
    fn handle(&mut self, r: Requests) {
        if r == Requests::default() {
            return;
        }
        let gate = &mut self.shell.gate;
        if r.screen_off {
            if gate.locked || !gate.screen_on {
                gate.locked = false;
                gate.screen_on = true;
            } else {
                gate.screen_on = false;
                gate.locked = true;
            }
            self.shell.invalidate();
        }
        if r.wake {
            self.shell.gate.screen_on = true;
            self.shell.invalidate();
        }
        if r.lock_notice && !self.shell.gate.screen_on {
            self.shell.gate.screen_on = true;
            self.shell.invalidate();
        }
        if r.power_off {
            self.fw.actions.push(Action::Power(PowerRequest::PowerOff));
        }
    }

    fn settle(&mut self) {
        while let Some(ev) = self.fw.pending.pop() {
            let r = self.shell.event(&mut *self.fw, ev, now());
            self.handle(r);
        }
    }

    async fn after(&mut self, fb: &mut [u16]) {
        self.settle();
        // The Authenticator (OTP-2): key derivation, vault changes with the
        // frame buffer as scratch, import file, typing results.
        if self.fw.auth_step(fb, now().0, self.shell.gate.screen_on) {
            self.shell.invalidate();
        }
        self.settle();
        // Settings changed by the screens go to NOR.
        if let Some(store) = self.store.as_mut() {
            let mut dirty = [(0u16, [0u8; 2]); 8];
            let mut n = 0;
            for d in self.fw.settings.take_dirty() {
                if n < dirty.len() {
                    dirty[n] = d;
                    n += 1;
                }
            }
            // VER-12: a test session keeps every change in RAM (the
            // reboot that ends the test brings the stored values back).
            let session = session_on();
            if session {
                SESSION_HELD.fetch_add(n as u16, core::sync::atomic::Ordering::Relaxed);
                n = 0;
            }
            for (k, v) in &dirty[..n] {
                let t0 = atj2157_hal::time::now_us();
                let _ = store.set(*k, v).await;
                let us = (atj2157_hal::time::now_us() - t0).min(0xFFFF) as u32;
                SET_WRITES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                SET_LAST.store(u32::from(*k) | us << 16, core::sync::atomic::Ordering::Relaxed);
            }
            // A newly connected Bluetooth device becomes the reconnect target.
            if let Some(a) = self.fw.bt_to_save.take()
                && !session
            {
                let _ = store.set(crate::bt::LAST_KEY, &a.0).await;
            }
            // The radio presets (UI-5a), in their stored form (a static).
            if core::mem::take(&mut self.fw.presets_dirty) {
                if session {
                    SESSION_HELD.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                } else {
                    let _ = store.set(orpheus_fm::presets::STORE_KEY, self.fw.presets.bytes()).await;
                }
            }
        }
        while let Some(a) = self.fw.actions.pop() {
            if let Action::Usb(m) = a {
                let p = match m {
                    orpheus_apps::services::UsbMode::FileTransfer => crate::usbdev::Personality::FileTransfer,
                    orpheus_apps::services::UsbMode::Keyboard => crate::usbdev::Personality::Keyboard,
                    // USB Drive: the image is in `crate::drive` already.
                    orpheus_apps::services::UsbMode::DiskImage => crate::usbdev::Personality::Image,
                    _ => crate::usbdev::Personality::Dev,
                };
                crate::usbdev::MSC_READ_ONLY.store(false, core::sync::atomic::Ordering::Relaxed);
                crate::usbdev::MODE.signal(p);
            }
            if let Action::Storage(orpheus_apps::services::StorageRequest::RebuildLibrary) = a {
                crate::media::request(crate::media::Req::Rebuild);
            }
            if let Action::Power(p) = a {
                match p {
                    PowerRequest::Restart => atj2157_hal::wdt::reboot(atj2157_hal::wdt::Target::Normal),
                    PowerRequest::Recovery => {
                        // RAM survives the ROM's USB mode: no key in it.
                        crate::vault::wipe_secrets();
                        atj2157_hal::wdt::reboot(atj2157_hal::wdt::Target::Adfu)
                    }
                    PowerRequest::LockKeys => self.shell.gate.locked = true,
                    PowerRequest::PowerOff => self.power_off().await,
                    // Power-off (PWR-3a) and the stock-firmware request
                    // (needs a stage-2 mailbox path) are not wired yet.
                    _ => {}
                }
            }
        }
        // Sound settings to the audio task (published only on change).
        {
            let st = &self.fw.settings;
            let g = |k: u16| st.get(orpheus_apps::settings::SettingId(k));
            let eq = orpheus_apps::settings::eq_gains(g(0x0101), |id| st.get(id));
            let snd = crate::media::Sound {
                eq,
                bass: g(0x0103) as i8,
                balance: g(0x0102) as i8,
                crossfeed: g(0x0104) as u8,
                channels: g(0x0105) as u8,
                replaygain: g(0x0106) as u8,
            };
            if self.sound != Some(snd) {
                self.sound = Some(snd);
                crate::media::publish_sound(snd);
            }
        }
        // Settings › System › Performance for the governor (D-4).
        crate::drain::shared().perf_max.store(
            self.fw.settings.get(orpheus_apps::settings::id::PERFORMANCE) == 1,
            core::sync::atomic::Ordering::Relaxed,
        );
        // Screen on/off follows the gate.
        let on = self.shell.gate.screen_on;
        if on != self.screen_on {
            self.screen_on = on;
            self.disp.sleep(!on);
            let level: u8 = if on { 12 } else { 0 };
            self.bl.set(u32::from(level));
            let sh = crate::drain::shared();
            sh.backlight.store(level, core::sync::atomic::Ordering::Relaxed);
            sh.screen_on.store(on, core::sync::atomic::Ordering::Relaxed);
            if on {
                self.shell.invalidate();
            }
        }
    }

    /// PWR-3a: flush the card, screen off, power off. A firmware started by
    /// a host test reboots into USB recovery instead (CLAUDE.md rule 4).
    async fn power_off(&mut self) {
        if atj2157_hal::persist::test_request().is_some() {
            atj2157_hal::log::line(b"power-off in a test: rebooting instead");
            atj2157_hal::wdt::reboot(atj2157_hal::wdt::Target::Adfu);
        }
        // A recording is stopped and saved first (AU-13).
        crate::rec::stop_for_power_off().await;
        // The battery log's last row goes out with the card task's flush.
        crate::drain::power_off();
        crate::media::SHUT_DOWN.store(false, core::sync::atomic::Ordering::Release);
        crate::media::play(crate::media::PlayReq::Shutdown);
        for _ in 0..100 {
            if crate::media::SHUT_DOWN.load(core::sync::atomic::Ordering::Acquire) {
                break;
            }
            Timer::after_millis(10).await;
        }
        self.bl.set(0);
        self.disp.sleep(true);
        Timer::after_millis(50).await;
        // Bluetooth module: shutdown command 0x24 when it is up, bounded
        // wait, enable line low (owner decision 2026-11-24, BT-2).
        crate::bt::shutdown_for_power_off().await;
        // The tuner into standby (FM-1), at most 300 ms.
        crate::fm::stop_for_power_off().await;
        // An armed RTC alarm wakes the player only once PWR-3c has shown it
        // works (`services::ALARM_WAKE_FROM_OFF`, off).
        atj2157_hal::pmu::power_off(crate::services::ALARM_WAKE_FROM_OFF && self.fw.rtc_alarm.is_some())
    }

    async fn draw(&mut self, fb: &mut [u16]) {
        // A screenshot is being read: do not change the frame buffer.
        let hold = SHOT_HOLD_UNTIL_MS.load(core::sync::atomic::Ordering::Relaxed);
        if hold != 0 && ((atj2157_hal::time::now_us() / 1000) as u32).wrapping_sub(hold) > u32::MAX / 2 {
            return;
        }
        // A picture job has the frame buffer (`pictures`), or the card task
        // is reading the Authenticator's import file into it (`vault`).
        if !self.screen_on || self.fw.pic_lent.is_some() || self.fw.otp_lent() {
            return;
        }
        if self.fw.pic_after_overlay && !self.shell.overlay_shown() {
            // A picture was not asked for under an overlay: ask again.
            self.fw.pic_after_overlay = false;
            self.shell.invalidate();
        }
        self.fw.pic_want.set(None);
        let mut dirty = DirtySet::new();
        let t0 = atj2157_hal::time::now_us();
        {
            let pal = services::palette(&*self.fw);
            let mut g = Gfx::new(fb, &mut dirty, pal, None);
            self.shell.draw(&mut *self.fw, &mut g);
        }
        let t1 = atj2157_hal::time::now_us();
        T_DRAW.fetch_add((t1 - t0) as u32, core::sync::atomic::Ordering::Relaxed);
        // Flush the union of the dirty rows as one band (full width).
        let mut y0 = 128u32;
        let mut y1 = 0u32;
        for r in dirty.rects() {
            y0 = y0.min(r.y.max(0) as u32);
            y1 = y1.max((r.y + r.h).clamp(0, 128) as u32);
        }
        if y1 > y0 {
            ROWS.fetch_add(y1 - y0, core::sync::atomic::Ordering::Relaxed);
            self.disp.flush_rows(fb, y0, y1).await;
            T_FLUSH
                .fetch_add((atj2157_hal::time::now_us() - t1) as u32, core::sync::atomic::Ordering::Relaxed);
        }
        // The frame is on the panel: lend the frame buffer for the picture
        // a screen asked for in it. Not under a menu, dialog, toast or the
        // volume (the picture would cover them).
        if self.fw.pic_want.get().is_some() && self.shell.overlay_shown() {
            self.fw.pic_want.set(None);
            self.fw.pic_after_overlay = true;
        }
        self.fw.lend_picture(fb, now().0);
    }
}

/// UI loop counters for STATS: iterations, microseconds of work, rows
/// flushed to the panel.
pub static LOOPS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static BUSY_US: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static ROWS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// Settings written to NOR: count, last key | (microseconds of the last write << 16).
/// Draw (shell drawing into the frame buffer) and flush (panel DMA) time.
pub static T_DRAW: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static T_FLUSH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// Set by the developer channel while a screenshot is read (ms deadline,
/// 0 = none): the UI does not draw until then.
pub static SHOT_HOLD_UNTIL_MS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static SET_WRITES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static SET_LAST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// The top screen for the developer channel's `ui top` (VER-7, 07 §5.7):
/// id | depth << 8 | dialog << 12 | overlay << 13 | keys locked << 14 |
/// screen on << 15 | name length << 16, and the name's bytes (a
/// `&'static str`). Written by the UI task after every loop, read by the USB
/// task; both run on the main executor, so the pair is never torn.
static TOP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static TOP_NAME: core::sync::atomic::AtomicPtr<u8> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

fn publish_top(ui: &Ui) {
    let s = &ui.shell;
    let name = s.top_name();
    let w = u32::from(s.top_id().0)
        | (s.depth().min(15) as u32) << 8
        | u32::from(s.dialog_open()) << 12
        | u32::from(s.overlay_shown()) << 13
        | u32::from(s.gate.locked) << 14
        | u32::from(s.gate.screen_on) << 15
        | (name.len().min(0xFFFF) as u32) << 16;
    TOP_NAME.store(name.as_ptr().cast_mut(), core::sync::atomic::Ordering::Relaxed);
    TOP.store(w, core::sync::atomic::Ordering::Relaxed);
}

/// Developer `ui top` (VER-7): `top ID NAME depth=D dialog=0|1 overlay=0|1
/// locked=0|1 screen=0|1`, answered at once.
pub fn top_reply() {
    let w = TOP.load(core::sync::atomic::Ordering::Relaxed);
    let p = TOP_NAME.load(core::sync::atomic::Ordering::Relaxed);
    let name: &[u8] = if p.is_null() {
        b"-"
    } else {
        // SAFETY: `p` and the length in `TOP` were stored together from one
        // `&'static str` by `publish_top` on this executor.
        unsafe { core::slice::from_raw_parts(p, (w >> 16) as usize) }
    };
    let bit = |n: u32| u64::from((w >> n) & 1);
    crate::audio::reply(|r| {
        r.s(b"top ")
            .u(u64::from(w & 0xFF))
            .s(b" ")
            .s(name)
            .s(b" depth=")
            .u(u64::from((w >> 8) & 0xF))
            .s(b" dialog=")
            .u(bit(12))
            .s(b" overlay=")
            .u(bit(13))
            .s(b" locked=")
            .u(bit(14))
            .s(b" screen=")
            .u(bit(15))
            .s(b" look=")
            .u(u64::from(TEST_LOOK.load(core::sync::atomic::Ordering::Relaxed)));
    });
}

/// Developer `ui test-look on|off` (VER-7): while on, the appearance
/// settings read as their defaults (Dark, Blue, all Home apps, clock in the
/// bar, lists roll round, no rotation, English, Now Playing's first view)
/// so screenshots match the simulator; the settings store is not written.
/// A RAM flag: cleared by `off` and by every reboot (`.bss`).
static TEST_LOOK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn set_test_look(on: bool) {
    TEST_LOOK.store(on, core::sync::atomic::Ordering::Relaxed);
    crate::audio::reply(|r| {
        r.s(if on { b"test-look on" } else { b"test-look off" });
    });
}

/// The value `sid` reads as under the test look, if it is one of its
/// settings: Display 0x0304–0x030C (theme … roll round), language,
/// Now Playing view.
pub fn test_look(sid: orpheus_apps::settings::SettingId) -> Option<i16> {
    let look = (0x0304..=0x030C).contains(&sid.0) || sid.0 == 0x0A01 || sid.0 == 0x0D03;
    (look && TEST_LOOK.load(core::sync::atomic::Ordering::Relaxed))
        .then(|| orpheus_apps::settings::def(sid).map_or(0, |d| d.default))
}

fn test_look_on() -> bool {
    TEST_LOOK.load(core::sync::atomic::Ordering::Relaxed)
}

// ------------------------------------------------- VER-12 soak test hooks

/// `settings test-session on`: setting changes (keys, `settings test-set`,
/// the radio presets) stay in RAM and are not written to NOR; the reboot
/// that ends every test brings the stored values back. A RAM flag: no
/// command switches it off, every reboot does.
static SESSION: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Changes kept from NOR during the session.
static SESSION_HELD: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);
/// `settings test-set ID VALUE` for the UI task: `id << 16 | value`, 0 none.
static PENDING_SET: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// `PENDING_SET` value asking for the `settings changed` list (no setting
/// has id 0xFFFF).
const CHANGED_REQ: u32 = u32::MAX;
/// `PENDING_SET` value field asking for the setting's default
/// (`settings default ID`; no setting uses i16::MIN).
const DEFAULT_MARK: i16 = i16::MIN;
/// Settings keys loaded from NOR at boot (0xFFFF: the store did not mount)
/// and a CRC-32 over them (key, bytes): the soak compares the boot before
/// and after a run (the store is readable and unchanged).
static STORE_KEYS: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0xFFFF);
static STORE_CRC: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// `ui screen-off`: the screen goes off without locking the keys (as a
/// backlight timeout would); the next key wakes it.
static SCREEN_OFF: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn session_on() -> bool {
    SESSION.load(core::sync::atomic::Ordering::Relaxed)
}

/// Settings a session may set from the host: Sound, Playback, Display
/// (not the Home apps), Language, Performance, Files sort and hidden
/// files, Now Playing's view. Never Bluetooth, power, clock, USB, storage,
/// alarms or the radio's station (CLAUDE.md rules 4 and 6).
fn session_settable(id: u16) -> bool {
    (matches!(id >> 8, 0x01 | 0x02 | 0x03 | 0x0A) && id != 0x030B) || matches!(id, 0x0C07 | 0x0D01..=0x0D03)
}

fn session_reply() {
    let keys = STORE_KEYS.load(core::sync::atomic::Ordering::Relaxed);
    crate::audio::reply(|r| {
        r.s(if session_on() { b"test-session on held=" } else { b"test-session off held=" })
            .u(u64::from(SESSION_HELD.load(core::sync::atomic::Ordering::Relaxed)))
            .s(if keys == 0xFFFF { b" store=none keys=0" } else { b" store=ok keys=" });
        if keys != 0xFFFF {
            r.u(u64::from(keys));
        }
        r.s(b" crc=").h(STORE_CRC.load(core::sync::atomic::Ordering::Relaxed));
    });
}

fn parse_num(w: &[u8]) -> Option<i32> {
    let (neg, d) = match w.split_first() {
        Some((b'-', d)) => (true, d),
        _ => (false, w),
    };
    let (radix, d) = match d.strip_prefix(b"0x") {
        Some(h) => (16, h),
        None => (10, d),
    };
    if d.is_empty() || d.len() > 6 {
        return None;
    }
    let mut v = 0i32;
    for &c in d {
        let x = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' if radix == 16 => c - b'a' + 10,
            _ => return None,
        };
        v = v * radix + i32::from(x);
    }
    Some(if neg { -v } else { v })
}

/// Developer `settings …` (VER-12): `test-session on`, `test-session`
/// (status), `test-set ID VALUE` (only in a session; the UI task applies
/// it as the screens do and answers `test-set ID=VALUE ok|refused`).
pub fn settings_command(rest: &[u8]) -> bool {
    if rest == b"test-session on" {
        SESSION.store(true, core::sync::atomic::Ordering::Relaxed);
        session_reply();
        return true;
    }
    if rest == b"test-session" {
        session_reply();
        return true;
    }
    // `settings default ID`: store the setting's default (persistent; for
    // repairs the owner approved, e.g. after the 2026-11-25 fuzzing).
    if let Some(id) = rest.strip_prefix(b"default ").and_then(parse_num) {
        let Ok(id) = u16::try_from(id) else { return false };
        PENDING_SET.store(
            u32::from(id) << 16 | u32::from(DEFAULT_MARK as u16),
            core::sync::atomic::Ordering::Relaxed,
        );
        return true;
    }
    if rest == b"changed" {
        // Read-only: the UI loop lists every stored setting whose value is
        // not its default (no session needed; nothing is written).
        PENDING_SET.store(CHANGED_REQ, core::sync::atomic::Ordering::Relaxed);
        return true;
    }
    let Some(args) = rest.strip_prefix(b"test-set ") else { return false };
    let mut w = args.split(|&b| b == b' ').filter(|w| !w.is_empty());
    let (Some(id), Some(v)) = (w.next().and_then(parse_num), w.next().and_then(parse_num)) else {
        crate::audio::reply(|r| {
            r.s(b"usage: settings test-set ID VALUE");
        });
        return true;
    };
    let ok_id = u16::try_from(id).ok().filter(|&i| i != 0 && session_settable(i));
    match (session_on(), ok_id, i16::try_from(v)) {
        (false, ..) => crate::audio::reply(|r| {
            r.s(b"test-set: no test session (settings test-session on)");
        }),
        (true, Some(i), Ok(v)) => {
            crate::audio::reply(|r| {
                r.s(b"test-set: queued");
            });
            PENDING_SET
                .store(u32::from(i) << 16 | u32::from(v as u16), core::sync::atomic::Ordering::Relaxed);
        }
        _ => crate::audio::reply(|r| {
            r.s(b"test-set: not allowed");
        }),
    }
    true
}

pub fn request_screen_off() {
    SCREEN_OFF.store(true, core::sync::atomic::Ordering::Relaxed);
    crate::audio::reply(|r| {
        r.s(b"screen-off");
    });
}

/// The UI task's side of the hooks (every loop).
fn soak_hooks(ui: &mut Ui) {
    use orpheus_apps::services::SettingsService;
    let p = PENDING_SET.swap(0, core::sync::atomic::Ordering::Relaxed);
    if p == CHANGED_REQ {
        crate::audio::reply(|r| {
            r.s(b"changed:");
            for d in orpheus_apps::settings::all().filter(|d| d.stored()) {
                let v = ui.fw.settings.get(d.id);
                if v != d.default {
                    r.s(b" ").h(u32::from(d.id.0)).s(b"=").i(v);
                }
            }
        });
        return;
    }
    if p != 0 && p as u16 as i16 == DEFAULT_MARK {
        let sid = orpheus_apps::settings::SettingId((p >> 16) as u16);
        if let Some(d) = orpheus_apps::settings::def(sid).filter(|d| d.stored()) {
            ui.fw.set_setting(sid, d.default);
            ui.settle();
            ui.shell.invalidate();
        }
        let got = ui.fw.settings.get(sid);
        crate::audio::reply(|r| {
            r.s(b"default ").h(u32::from(sid.0)).s(b"=").i(got).s(if session_on() {
                b" (session: RAM only)"
            } else {
                b" stored"
            });
        });
        return;
    }
    if p != 0 && session_on() {
        let sid = orpheus_apps::settings::SettingId((p >> 16) as u16);
        let want = p as u16 as i16;
        ui.fw.set_setting(sid, want);
        ui.settle();
        let got = ui.fw.settings.get(sid);
        crate::audio::reply(|r| {
            r.s(b"test-set ").u(u64::from(sid.0)).s(b"=").i(got).s(if got == want {
                b" ok"
            } else {
                b" refused"
            });
        });
        ui.shell.invalidate();
    }
    if SCREEN_OFF.swap(false, core::sync::atomic::Ordering::Relaxed) && ui.shell.gate.screen_on {
        ui.shell.gate.screen_on = false;
        ui.shell.invalidate();
    }
}

static mut FW_CELL: core::mem::MaybeUninit<Fw> = core::mem::MaybeUninit::uninit();
static mut STORE_CELL: Option<Store<NorRegion, 4>> = None;
static mut UI_CELL: core::mem::MaybeUninit<Ui> = core::mem::MaybeUninit::uninit();

/// Once, from the UI task.
#[inline(never)]
fn new_fw() -> &'static mut Fw {
    // SAFETY: called once; the UI task is the only user.
    unsafe { (*(&raw mut FW_CELL)).write(Fw::new()) }
}

/// Once, from the UI task: the settings store mounted straight into its
/// static. Not inlined, and not awaited in the task: the mount's result
/// (an `Option<Store>` of 1 KB) passed through the UI task's poll frame
/// three times, 3.2 KB of stack reserved on every poll under the RT
/// executor's (VER-7 stack check, 2026-11-25). The NOR driver is blocking
/// (an erase yields only to let others run), so `block_on` returns at once.
#[inline(never)]
fn mount_store() -> &'static mut Option<Store<NorRegion, 4>> {
    // SAFETY: called once; the UI task is the only user.
    let slot = unsafe { &mut *(&raw mut STORE_CELL) };
    *slot = embassy_futures::block_on(Store::<NorRegion, 4>::mount(NorRegion::settings())).ok();
    slot
}

/// Once, from the UI task.
#[inline(never)]
fn new_ui(
    fw: &'static mut Fw,
    store: &'static mut Option<Store<NorRegion, 4>>,
    disp: Display,
    bl: Backlight,
    t: Instant,
) -> &'static mut Ui {
    // Not entered here: this frame holds the whole `Ui` as a temporary
    // (4.8 KB), and the root screen's `enter` (any screen's, as far as
    // the call graph knows) would run below it. `enter_root` does that
    // once the shell is in its static (VER-10).
    let shell = Shell::<AppScreen, Fw>::unentered(fw, screens::HOME, Arg(0), t);
    let ui = Ui { fw, shell, mapper: KeyMapper::new(), disp, bl, screen_on: true, store, sound: None };
    // SAFETY: called once; the UI task is the only user.
    unsafe { (*(&raw mut UI_CELL)).write(ui) }
}

/// Enter the root screen of the shell [`new_ui`] built, in place.
#[inline(never)]
fn enter_root(ui: &mut Ui) {
    ui.shell.enter_root(ui.fw);
}

#[embassy_executor::task]
pub async fn ui() {
    let Some(fb) = display::take_framebuffer() else { return };
    // SAFETY: DMA channel 1 is the display's (01-hardware §6).
    let disp = Display::new(unsafe { dma::Channel::steal(1) });
    let bl = Backlight::new(12);
    // `Fw` (1.5 KB), the settings store (1 KB) and `Ui` (≈ 5 KB) live in
    // statics, built by non-inlined constructors: built by value here they
    // made this task's poll frame 6 KB, reserved on every poll under the RT
    // interrupt's stack use (2026-11-24).
    let fw = new_fw();
    crate::services::apply_volume(fw.volume);
    // Settings from NOR.
    let store = mount_store();
    let mut bt_last = None;
    if let Some(s) = store.as_mut() {
        // Every stored setting (65 since PWR-5) and the Bluetooth address:
        // after a factory reset all of them are in the store.
        let mut keys = [0u16; orpheus_apps::settings::STORED + 8];
        let mut n = 0;
        for k in s.keys() {
            if n < keys.len() {
                keys[n] = k;
                n += 1;
            }
        }
        let mut crc = 0xFFFF_FFFF;
        for &k in &keys[..n] {
            let mut b = [0u8; 8];
            crc = orpheus_proto::hash::crc32_update(crc, &k.to_le_bytes());
            if let Ok(Some(len)) = s.get(k, &mut b).await {
                crc = orpheus_proto::hash::crc32_update(crc, &b[..len.min(8)]);
                fw.settings.load(k, &b[..len]);
                if k == crate::bt::LAST_KEY && len == 6 {
                    let mut a = [0u8; 6];
                    a.copy_from_slice(&b[..6]);
                    bt_last = Some(orpheus_bt::Addr(a));
                }
            }
        }
        STORE_KEYS.store(n as u16, core::sync::atomic::Ordering::Relaxed);
        STORE_CRC.store(crc, core::sync::atomic::Ordering::Relaxed);
    }
    // The radio presets: straight into their static (332 bytes).
    if let Some(s) = store.as_mut() {
        match s.get(orpheus_fm::presets::STORE_KEY, fw.presets.raw_mut()).await {
            Ok(Some(n)) => {
                fw.presets.loaded(n);
                let crc = orpheus_proto::hash::crc32_update(
                    STORE_CRC.load(core::sync::atomic::Ordering::Relaxed),
                    fw.presets.bytes(),
                );
                STORE_CRC.store(crc, core::sync::atomic::Ordering::Relaxed);
            }
            _ => *fw.presets = orpheus_fm::Presets::new(),
        }
    }
    fw.init_player();
    fw.bt_boot(bt_last);
    fw.radio_boot();
    fw.auth_boot(now().0);
    let ui = new_ui(fw, store, disp, bl, now());
    enter_root(ui);
    ui.shell.invalidate();
    let fb = fb.as_mut_slice();
    ui.draw(fb).await;
    publish_top(ui);
    let mut last_cable = state::snapshot().cable;
    // Auto-off / low-battery timers (ms since boot).
    let mut last_key_ms = now().0;
    let mut low_since = 0u64;
    let mut flashing = false;
    let mut seen_look = false;
    loop {
        let t = now();
        let mut deadline = t.0 + 1000;
        if let Some(d) = ui.mapper.next_deadline() {
            deadline = deadline.min(d.0);
        }
        if ui.shell.wants_ticks(&*ui.fw) {
            deadline = deadline.min(t.0 + u64::from(TICK_MS));
        }
        // UI-5d: the timer's end on time; the alert's backlight flashes.
        if let Some(end) = ui.fw.tools.timer.ends_at() {
            deadline = deadline.min(end.max(t.0 + 1));
        }
        if ui.fw.tools.ringing().is_some() {
            deadline = deadline.min(t.0 + 250);
        }
        // A key derivation waits one frame; the import file is awaited.
        if ui.fw.auth_busy() {
            deadline = deadline.min(t.0 + 2);
        }
        let wait = Duration::from_millis(deadline.saturating_sub(t.0).max(1));
        match select4(KEYS.receive(), BATTERY.receive(), Timer::after(wait), crate::media::READY.receive())
            .await
        {
            Either4::Fourth(tag) => {
                ui.fw.sync(tag);
                let (first, count) = (ui.fw.browse.first, ui.fw.browse.n);
                let ev = Event::Data(orpheus_ui::event::DataReady { tag, first, count });
                let r = ui.shell.event(&mut *ui.fw, ev, now());
                ui.handle(r);
            }
            Either4::First((k, down)) => {
                let t = now();
                last_key_ms = t.0;
                // Key timing feeds the vault's entropy pool (salts, nonces).
                ui.fw.auth.mix(&atj2157_hal::time::now_us().to_le_bytes());
                // A key: a picture still coming is not waited for (the
                // volume overlay can wait).
                if down && !matches!(k, HwKey::VolUp | HwKey::VolDown) {
                    ui.fw.reclaim_picture();
                }
                if down {
                    ui.mapper.press(map_key(k), t);
                } else {
                    ui.mapper.release(map_key(k), t);
                }
            }
            Either4::Second(()) => {
                let s = state::snapshot();
                let ev =
                    Event::Service(ServiceEvent::Battery { percent: s.percent, charging: s.charge_active() });
                let r = ui.shell.event(&mut *ui.fw, ev, now());
                ui.handle(r);
                if s.cable != last_cable {
                    last_cable = s.cable;
                    let r =
                        ui.shell.event(&mut *ui.fw, Event::Service(ServiceEvent::Usb { host: false }), now());
                    ui.handle(r);
                }
            }
            Either4::Third(()) => {}
        }
        // VER-7: the test look switched: every screen reloads and redraws.
        if test_look_on() != seen_look {
            seen_look = !seen_look;
            ui.fw
                .pending
                .push(Event::Service(ServiceEvent::SettingChanged(orpheus_apps::settings::id::HOME_APPS.0)));
            ui.shell.invalidate();
        }
        // VER-12: `settings test-set`, `ui screen-off`.
        soak_hooks(ui);
        let w0 = atj2157_hal::time::now_us();
        LOOPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        atj2157_hal::wdt::beat(&atj2157_hal::wdt::UI_BEAT);
        ui.fw.sync_player();
        ui.fw.sync_bt();
        ui.fw.sync_radio(now().0);
        ui.fw.sync_rec();
        ui.fw.sync_drive();
        // SENS-1 / UI-5c: the Steps app counts while it is open.
        let steps_open = ui.shell.top_id() == screens::STEPS;
        ui.fw.sync_steps(steps_open);
        // UI-5d: an alert lights the screen (keys stay as they were).
        if ui.fw.sync_tools(now().0) == Some(orpheus_apps::services::tools_event::ALERT) {
            ui.shell.gate.screen_on = true;
            ui.shell.invalidate();
        }
        let t = now();
        ui.fw.poll_picture(t.0);
        while let Some((k, p)) = ui.mapper.poll(t) {
            let r = ui.shell.key(&mut *ui.fw, k, p, t);
            ui.handle(r);
            ui.settle();
        }
        if ui.shell.wants_ticks(&*ui.fw) {
            let r = ui.shell.tick(&mut *ui.fw, t);
            ui.handle(r);
        }
        ui.after(fb).await;
        // PWR-3a: auto-off and low battery (both through `power_off`, which
        // reboots instead under a host test).
        {
            let t = now().0;
            let st = state::snapshot();
            // The radio playing is not idle either (UI-5a); a running
            // stopwatch or timer keeps the player on (it would be lost);
            // alarms do not (they ring only while it is on, and say so).
            let idle = matches!(
                ui.fw.player.source,
                orpheus_ui::event::Source::Idle | orpheus_ui::event::Source::Paused
            ) && !ui.fw.radio_on()
                && !crate::rec::active()
                && !ui.fw.tools.stopwatch.running()
                && !ui.fw.tools.timer.running()
                && ui.fw.tools.ringing().is_none();
            let mins = match ui.fw.settings.get(orpheus_apps::settings::SettingId(0x0403)) {
                1 => 5,
                2 => 10,
                3 => 15,
                4 => 30,
                5 => 60,
                _ => 0,
            };
            // A battery log started with `stay` keeps auto-off away
            // (PWR-5); otherwise the logger changes nothing here.
            if mins > 0
                && idle
                && !st.cable
                && t.saturating_sub(last_key_ms) >= mins * 60_000
                && !crate::drain::stay()
            {
                atj2157_hal::log::line(b"pwr: auto-off");
                ui.power_off().await;
            }
            // 0 % on the gauge (3.30 V) for 10 s without a cable.
            if !st.cable && st.millivolts != 0 && st.millivolts < 3300 {
                if low_since == 0 {
                    low_since = t.max(1);
                } else if t.saturating_sub(low_since) >= 10_000 {
                    atj2157_hal::log::line(b"pwr: battery empty");
                    ui.power_off().await;
                }
            } else {
                low_since = 0;
            }
        }
        // The alert flashes the backlight (bright / dim every 500 ms).
        let flash = ui.screen_on && ui.fw.tools.ringing().is_some();
        if flash || flashing {
            let level: u32 = match (flash, (now().0 / 500) % 2 == 0) {
                (true, true) => 16,
                (true, false) => 3,
                // Over: the normal level, or dark if the screen went off.
                (false, _) if ui.screen_on => 12,
                (false, _) => 0,
            };
            ui.bl.set(level);
            flashing = flash;
        }
        ui.draw(fb).await;
        publish_top(ui);
        BUSY_US.fetch_add((atj2157_hal::time::now_us() - w0) as u32, core::sync::atomic::Ordering::Relaxed);
    }
}
