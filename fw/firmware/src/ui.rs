//! The UI task: key mapper → shell → screens → frame buffer → panel.

use atj2157_hal::dma;
use embassy_futures::select::{Either4, select4};
use embassy_time::{Duration, Instant as TInstant, Timer};
use orpheus_apps::screens::{self, AppScreen};
use orpheus_apps::services::{self, ClockService, PowerRequest};
use orpheus_input::Key as HwKey;
use orpheus_ui::event::{Event, Instant, Key, ServiceEvent};
use orpheus_ui::gfx::{DirtySet, Gfx};
use orpheus_ui::input::{KeyMapper, ScreenTimer};
use orpheus_ui::nav::Arg;
use orpheus_ui::shell::{Requests, Shell, TICK_MS};
use yoton_bsp::display::{self, Backlight, Display, Duty};
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
    /// Backlight timeout and lock delay (VER-13, 05 §6.3).
    timer: ScreenTimer,
    /// Settings › Date & time › Correction: what is not yet a whole
    /// second, and when it was last stepped (ms).
    drift: services::ClockDrift,
    drift_at: u64,
    /// The panel is turned by 180° (Settings › Display › Rotate).
    rotated: bool,
    /// The screen reader (A11Y-1, TTS-3).
    #[cfg(feature = "speech")]
    a11y: reader::A11y,
}

/// The screen reader (A11Y-1) with the speech voice (TTS-3): firmware
/// feature `speech`.
#[cfg(feature = "speech")]
mod reader {
    use super::*;
    use orpheus_ui::a11y::{self, Cause, Chord, Chords, Desc, Edge, Reader};
    use orpheus_ui::event::KeyPhase;
    use orpheus_ui::input::Timeouts;

    /// The screen reader in the UI task (A11Y-1, 05 §7.14 "For the firmware";
    /// the simulator's `Sim::observe` is the reference): the chords on the raw
    /// key edges, the reader over the shell's description, the speech voice
    /// (`crate::speech`), Music while speaking › Paused.
    pub(super) struct A11y {
        pub(super) chords: Chords,
        reader: Reader,
        desc: Desc,
        speech: crate::speech::FwSpeech,
        pause: services::SpeechPause,
        /// The reader was on, the keys locked, at the last look.
        pub(super) on: bool,
        locked: bool,
    }

    impl A11y {
        pub(super) const fn new() -> A11y {
            A11y {
                chords: Chords::new(),
                reader: Reader::new(),
                desc: Desc::new(),
                speech: crate::speech::FwSpeech,
                pause: services::SpeechPause::new(),
                on: false,
                locked: false,
            }
        }
    }

    /// The screen reader's settings as the UI follows them.
    pub(super) fn reader_cfg(fw: &Fw) -> a11y::Config {
        services::reader_config(fw)
    }

    /// The screen reader looks at the screen after `cause` (A11Y-1): it says
    /// what changed while it is on, and announces itself when it comes on or
    /// goes off.
    #[inline(never)]
    fn observe(ui: &mut Ui, cause: Cause) {
        let cfg = reader_cfg(ui.fw);
        let a = &mut ui.a11y;
        if cfg.on != a.on {
            a.on = cfg.on;
            a.reader.reset();
            if cfg.on {
                a.reader.announce("Screen reader on", a11y::Earcon::ReaderOn, &cfg, &mut a.speech);
            } else {
                a.reader.announce("Screen reader off", a11y::Earcon::ReaderOff, &cfg, &mut a.speech);
                return;
            }
        }
        if !cfg.on {
            return;
        }
        // Power locked or unlocked the keys: said (unlocked: the screen again).
        let locked = ui.shell.gate.locked;
        if locked != a.locked {
            a.locked = locked;
            if locked {
                a.reader.announce("Keys locked", a11y::Earcon::Off, &cfg, &mut a.speech);
            } else {
                a.reader.announce("Unlocked", a11y::Earcon::On, &cfg, &mut a.speech);
                a.reader.reset();
            }
        }
        ui.shell.describe(&mut *ui.fw, &mut a.desc);
        if ui.shell.gate.locked && !ui.shell.gate.screen_on {
            // Locked and dark: what changes under the lock is not said.
            a.reader.note(&a.desc);
            return;
        }
        a.reader.observe(&a.desc, cause, &cfg, &mut a.speech);
    }

    /// A chord the detector recognised: the toggle, where am I, the time and
    /// battery.
    #[inline(never)]
    fn chord(ui: &mut Ui, c: Chord) {
        let cfg = reader_cfg(ui.fw);
        match c {
            Chord::Toggle => {
                orpheus_apps::services::SettingsService::set_setting(
                    &mut *ui.fw,
                    orpheus_apps::settings::id::READER,
                    i16::from(!cfg.on),
                );
                ui.settle();
                observe(ui, Cause::Other);
            }
            Chord::WhereAmI if cfg.on => {
                let a = &mut ui.a11y;
                ui.shell.describe(&mut *ui.fw, &mut a.desc);
                a.reader.where_am_i(&a.desc, &cfg, &mut a.speech);
            }
            Chord::TimeBattery if cfg.on => {
                let st = orpheus_ui::shell::System::status(&*ui.fw);
                let a = &mut ui.a11y;
                a.reader.time_battery(&st, &cfg, &mut a.speech);
            }
            _ => {}
        }
    }

    /// A raw key edge through the chords first (A11Y-1: from any screen,
    /// locked, dark, with the reader off), then the key mapper.
    #[inline(never)]
    pub(super) fn key_edge(ui: &mut Ui, k: Key, down: bool, t: Instant) {
        let cfg = reader_cfg(ui.fw);
        // Settings › Accessibility › Keys with screen off.
        ui.shell.gate.reader_keys = cfg.on && cfg.dark_keys;
        let wk = if cfg.on { cfg.where_keys } else { a11y::WhereKeys::Off };
        let edge = if down { ui.a11y.chords.press(k, t, wk) } else { ui.a11y.chords.release(k) };
        match edge {
            Edge::Pass if down => ui.mapper.press(k, t),
            Edge::Pass => ui.mapper.release(k, t),
            Edge::Swallow => {}
            Edge::Start(first) => {
                // The chord's first key: forgotten without a release.
                ui.mapper.cancel(first);
                ui.shell.gate.forget(first);
            }
        }
    }

    /// The reader after a key phase the shell handled (only while it is on:
    /// the description costs a screen's walk).
    pub(super) fn observe_key(ui: &mut Ui, k: Key, p: KeyPhase) {
        if ui.a11y.on {
            observe(ui, Cause::Key(k, p));
        }
    }

    /// Each loop: a chord due, Music while speaking › Paused (TTS-3), the
    /// reader's look at whatever changed by itself.
    #[inline(never)]
    pub(super) fn reader_loop(ui: &mut Ui, t: Instant) {
        if let Some(c) = ui.a11y.chords.poll(t, ui.mapper.timing.hold) {
            chord(ui, c);
        }
        let cfg = reader_cfg(ui.fw);
        let src = orpheus_apps::services::PlaybackService::playing(&*ui.fw).map(|p| p.source);
        let speaking = cfg.on && crate::speech::speaking();
        if let Some(cmd) = ui.a11y.pause.step(cfg.on && cfg.music == 1, speaking, src, t.0) {
            orpheus_apps::services::PlayerService::command(&mut *ui.fw, cmd);
            ui.settle();
        }
        if cfg.on || ui.a11y.on {
            observe(ui, Cause::Other);
        }
    }

    /// The loop's next deadline for the reader: a chord's, and while it
    /// speaks the pause policy's quiet time.
    pub(super) fn deadline(ui: &Ui, t: u64) -> Option<u64> {
        let mut d = ui.a11y.chords.next_deadline(ui.mapper.timing.hold).map(|d| d.0.max(t + 1));
        if ui.a11y.on && crate::speech::speaking() {
            d = Some(d.map_or(t + 100, |d| d.min(t + 100)));
        }
        d
    }

    /// The screen timers with the reader's keys with the screen off: the
    /// backlight timeout darkens without locking (Power still locks).
    pub(super) fn timeouts(fw: &Fw, t: Timeouts) -> Timeouts {
        reader_cfg(fw).timeouts(t)
    }
}

/// Without feature `speech`: the keys go to the key mapper as they come.
#[cfg(not(feature = "speech"))]
mod reader {
    use super::*;
    use orpheus_ui::event::KeyPhase;
    use orpheus_ui::input::Timeouts;

    pub(super) fn key_edge(ui: &mut Ui, k: Key, down: bool, t: Instant) {
        if down {
            ui.mapper.press(k, t);
        } else {
            ui.mapper.release(k, t);
        }
    }

    pub(super) fn observe_key(_ui: &mut Ui, _k: Key, _p: KeyPhase) {}

    pub(super) fn reader_loop(_ui: &mut Ui, _t: Instant) {}

    pub(super) fn deadline(_ui: &Ui, _t: u64) -> Option<u64> {
        None
    }

    pub(super) fn timeouts(_fw: &Fw, t: Timeouts) -> Timeouts {
        t
    }
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
        // The developer channel's normal reboot: through the same saves.
        if RESTART_REQ.swap(false, core::sync::atomic::Ordering::AcqRel) {
            self.restart().await;
        }
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
                // The card goes to the computer: the game in progress is
                // saved first (its file is written before the hand-over).
                if matches!(
                    m,
                    orpheus_apps::services::UsbMode::FileTransfer
                        | orpheus_apps::services::UsbMode::DiskImage
                ) {
                    self.save_games(1000).await;
                }
                let p = match m {
                    orpheus_apps::services::UsbMode::FileTransfer => crate::usbdev::Personality::FileTransfer,
                    orpheus_apps::services::UsbMode::Keyboard => crate::usbdev::Personality::Keyboard,
                    // USB Drive: the image is in `crate::drive` already.
                    orpheus_apps::services::UsbMode::DiskImage => crate::usbdev::Personality::Image,
                    _ => crate::usbdev::Personality::Dev,
                };
                // Settings › USB › Read-only card (VER-13).
                crate::usbdev::MSC_READ_ONLY
                    .store(services::usb_card_read_only(&*self.fw), core::sync::atomic::Ordering::Relaxed);
                crate::usbdev::MODE.signal(p);
            }
            if let Action::Storage(orpheus_apps::services::StorageRequest::RebuildLibrary) = a {
                crate::media::request(crate::media::Req::Rebuild);
            }
            if let Action::Power(p) = a {
                match p {
                    PowerRequest::Restart => self.restart().await,
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
            use crate::media::opt;
            let bit = |k: u16, b: u8| if g(k) != 0 { b } else { 0 };
            let snd = crate::media::Sound {
                eq,
                bass: g(0x0103) as i8,
                balance: g(0x0102) as i8,
                crossfeed: g(0x0104) as u8,
                channels: g(0x0105) as u8,
                replaygain: g(0x0106) as u8,
                // Fade on pause, Resume at start-up (VER-13).
                opts: opt::VALID | bit(0x0109, opt::FADE) | bit(0x0204, opt::RESUME),
            };
            if self.sound != Some(snd) {
                self.sound = Some(snd);
                crate::media::publish_sound(snd);
            }
        }
        publish_settings(self.fw);
        // Screen on/off follows the gate; the screen saver keeps the panel
        // lit (VER-13).
        let on = self.shell.gate.screen_on || self.shell.gate.saver;
        if on != self.screen_on {
            self.screen_on = on;
            self.disp.sleep(!on);
            crate::drain::shared().screen_on.store(on, core::sync::atomic::Ordering::Relaxed);
            if on {
                self.shell.invalidate();
            }
        }
        // Brightness, the panel's state and the alert's flash (U-PWM-3).
        apply_backlight(self);
    }

    /// PWR-3a: flush the card, screen off, power off. A firmware started by
    /// a host test reboots into USB recovery instead (CLAUDE.md rule 4).
    async fn power_off(&mut self) {
        if atj2157_hal::persist::test_request().is_some() {
            atj2157_hal::log::line(b"power-off in a test: rebooting instead");
            atj2157_hal::wdt::reboot(atj2157_hal::wdt::Target::Adfu);
        }
        self.save_for_shutdown().await;
        self.bl.set(None);
        crate::drain::shared().backlight.store(0, core::sync::atomic::Ordering::Relaxed);
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

    /// What power-off and a restart save first: the recording, the game in
    /// progress, the battery log's last row, and the player's place (the
    /// card task's shutdown). A restart used to reboot at once, so Resume
    /// at start-up reopened the track at its start (2026-11-26).
    #[inline(never)]
    async fn save_for_shutdown(&mut self) {
        // A recording is stopped and saved first (AU-13).
        crate::rec::stop_for_power_off().await;
        // The game in progress goes to its save file (05 §7.13).
        self.save_games(1500).await;
        // The open book's reading position (05 §7.15).
        crate::books::save_for_shutdown(1000).await;
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
    }

    /// Restart (the power menu, or the developer channel's normal reboot):
    /// save as for power-off, then reboot.
    #[inline(never)]
    async fn restart(&mut self) {
        self.save_for_shutdown().await;
        atj2157_hal::log::line(b"ui: restart");
        atj2157_hal::wdt::reboot(atj2157_hal::wdt::Target::Normal)
    }

    /// Save the game in progress and wait (at most `max_ms`) until the
    /// card task has written it (power-off, USB hand-over).
    async fn save_games(&mut self, max_ms: u64) {
        use orpheus_apps::games::GamesService;
        if !self.fw.games().session.in_progress() && !self.fw.games_busy() {
            return;
        }
        self.fw.games_request(orpheus_apps::games::GamesRequest::Save);
        let t0 = now().0;
        while self.fw.games_busy() && now().0.saturating_sub(t0) < max_ms {
            Timer::after_millis(10).await;
            self.fw.games_poll(now().0);
        }
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
            // Settings › Display › Rotate (VER-13): the band turned and sent
            // to the mirrored rows, then turned back (the frame buffer stays
            // upright for drawing and screenshots).
            let p0 = turn_for_panel(fb, y0, y1, self.rotated);
            self.disp.flush_band(fb, y0, y1, p0).await;
            if self.rotated {
                orpheus_ui::gfx::turn_band(fb, y0, y1);
            }
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

/// Settings › Display › Brightness on the backlight (VER-13, U-PWM-3):
/// the stock duty of the level while the panel is lit (screen on or the
/// screen saver), dark when it is off, and an alert's flash (15 / 3 every
/// 500 ms) while one rings — `services::backlight`, the same function the
/// simulator uses; never a duty outside the stock table (`Duty`). Written
/// only when the duty changes (by the vendor's stop/start, `Backlight::set`)
/// and published for `ui top` (`bl=`) and the battery log.
#[inline(never)]
fn apply_backlight(ui: &mut Ui) {
    use core::sync::atomic::Ordering::Relaxed;
    let ringing = ui.fw.tools.ringing().is_some();
    let want = services::backlight(&*ui.fw, ui.screen_on, ringing, now().0);
    let duty = want.map_or(0, Duty::get);
    let sh = crate::drain::shared();
    if sh.backlight.load(Relaxed) != duty {
        ui.bl.set(want);
        sh.backlight.store(duty, Relaxed);
    }
}

/// The panel row the band `y0..y1` goes to, the band turned first when
/// the display is rotated.
#[inline(never)]
fn turn_for_panel(fb: &mut [u16], y0: u32, y1: u32, rotated: bool) -> u32 {
    if !rotated {
        return y0;
    }
    orpheus_ui::gfx::turn_band(fb, y0, y1);
    orpheus_ui::gfx::rotated_band(y0, y1).0
}

/// Settings other tasks follow (VER-13), published every loop: Power mode
/// (the governor), Time zone (`vault::unix_now`), Developer channel (the
/// USB task; a change re-enumerates the developer personality).
#[inline(never)]
fn publish_settings(fw: &Fw) {
    use core::sync::atomic::Ordering::Relaxed;
    // Automatic while a host test runs (the owner's mode must not change a
    // suite's result).
    let pm = fw.settings.get(orpheus_apps::settings::id::POWER_MODE);
    POWER_MODE.store(orpheus_power::governor::PowerMode::for_governor(pm, dev_channel_held()) as u8, Relaxed);
    UTC_OFFSET_S.store(services::utc_offset_s(fw), Relaxed);
    let dev = services::developer_channel(fw, dev_channel_held());
    if DEV_CHANNEL.swap(dev, Relaxed) != dev && crate::usbdev::CURRENT.load(Relaxed) == 0 {
        // The developer personality again, built with or without it.
        crate::usbdev::MODE.signal(crate::usbdev::Personality::Dev);
    }
}

/// Settings › Power › Power mode as the governor takes it (0 Automatic, 1
/// Battery saver, 2 High performance).
pub static POWER_MODE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
/// Settings › Date & time › Time zone, seconds east of UTC
/// (`vault::unix_now` gives UTC).
pub static UTC_OFFSET_S: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);
/// Settings › USB › Developer channel as offered (`usbdev` builds the
/// device with or without it); on until the settings are read.
pub static DEV_CHANNEL: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// The developer channel stays offered whatever Settings › USB ›
/// Developer channel says while a host test runs (the test mailbox, the
/// test look, a settings test session): the tools need it, and a test
/// can never take it away (VER-13).
fn dev_channel_held() -> bool {
    session_on() || test_look_on() || atj2157_hal::persist::test_request().is_some()
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

/// The Display, Power and Date & time settings the UI loop carries out
/// (VER-13): Rotate (the key map and the panel), the key timing (A11Y-2),
/// Keys when locked, the
/// backlight timeout and lock delay (held under a host test unless its
/// session armed them), Correction (the RTC stepped by whole seconds; not
/// during a test session).
#[inline(never)]
fn apply_settings(ui: &mut Ui) {
    let t = now().0;
    let rot = services::rotated(&*ui.fw);
    ui.mapper.rotated = rot;
    // Settings › Accessibility › Hold delay, Repeat speed (A11Y-2).
    ui.mapper.timing = services::key_timing(&*ui.fw);
    if rot != ui.rotated {
        ui.rotated = rot;
        ui.shell.invalidate();
    }
    ui.shell.gate.when_locked = services::locked_keys(&*ui.fw);
    if TEST_WAKE.swap(false, core::sync::atomic::Ordering::Relaxed) {
        ui.shell.gate.wake_for_test();
        ui.timer = ScreenTimer::new(t);
        ui.shell.invalidate();
    }
    let tm = if test_look_on() || atj2157_hal::persist::test_request().is_some() {
        services::TimerHold::from_bits(TIMER_HOLD.load(core::sync::atomic::Ordering::Relaxed))
            .timeouts(&*ui.fw)
    } else {
        services::screen_timeouts(&*ui.fw)
    };
    // A11Y-1: with the reader on and Keys with screen off, the backlight
    // timeout darkens without locking (Power still locks).
    let tm = reader::timeouts(ui.fw, tm);
    if ui.timer.step(&mut ui.shell.gate, tm, t) {
        ui.shell.invalidate();
    }
    let per_day = ui.fw.settings.get(orpheus_apps::settings::id::CLOCK_CORRECTION);
    let fix = ui.drift.step(t.saturating_sub(ui.drift_at), per_day);
    ui.drift_at = t;
    // Not in a test session: a test never moves the owner's clock.
    if fix != 0 && !session_on() && ui.fw.clock_valid() {
        let now = services::unix_of(&ui.fw.now()).saturating_add_signed(i64::from(fix));
        ui.fw.set_now(services::date_of(now));
    }
}

/// The top screen for the developer channel's `ui top` (VER-7, 07 §5.7):
/// id | depth << 8 | dialog << 12 | overlay << 13 | keys locked << 14 |
/// screen on << 15 | name length << 16 | screen saver shown << 31, and the name's bytes (a
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
        | (name.len().min(0x7FFF) as u32) << 16
        | u32::from(s.gate.saver && !s.gate.screen_on) << 31;
    TOP_NAME.store(name.as_ptr().cast_mut(), core::sync::atomic::Ordering::Relaxed);
    TOP.store(w, core::sync::atomic::Ordering::Relaxed);
}

/// Developer `ui top` (VER-7): `top ID NAME depth=D dialog=0|1 overlay=0|1
/// locked=0|1 screen=0|1 look=0|1 saver=0|1 bl=DUTY speech=0|1`, answered
/// at once (`bl`: the backlight's PWM4 duty of 16, 0 dark; VER-13
/// Brightness; `speech`: the screen reader's speech sounds or waits,
/// TTS-3, `ui speech` for the rest).
pub fn top_reply() {
    let w = TOP.load(core::sync::atomic::Ordering::Relaxed);
    let p = TOP_NAME.load(core::sync::atomic::Ordering::Relaxed);
    let name: &[u8] = if p.is_null() {
        b"-"
    } else {
        // SAFETY: `p` and the length in `TOP` were stored together from one
        // `&'static str` by `publish_top` on this executor.
        unsafe { core::slice::from_raw_parts(p, ((w >> 16) & 0x7FFF) as usize) }
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
            .u(u64::from(TEST_LOOK.load(core::sync::atomic::Ordering::Relaxed)))
            .s(b" saver=")
            .u(bit(31))
            .s(b" bl=")
            .u(u64::from(crate::drain::shared().backlight.load(core::sync::atomic::Ordering::Relaxed)))
            // TTS-3: speech sounds or waits (`ui speech` says more).
            .s(b" speech=")
            .u(u64::from(crate::speech::speaking()));
    });
}

/// Developer `ui test-look on|off` (VER-7): while on, the appearance
/// settings read as their defaults (Dark, Blue, all Home apps, clock in the
/// bar, lists roll round, no rotation, English, Now Playing's first view)
/// so screenshots match the simulator; the settings store is not written.
/// A RAM flag: cleared by `off` and by every reboot (`.bss`).
/// A normal reboot asked for over the developer channel: the UI task
/// saves (as for power-off) and reboots; the USB task reboots itself if
/// that has not happened within 3 s.
static RESTART_REQ: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn request_restart() {
    RESTART_REQ.store(true, core::sync::atomic::Ordering::Release);
}

static TEST_LOOK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Set by `ui test-look on`: the screen timers may have locked the keys
/// before a test tool arrived (2026-11-26).
static TEST_WAKE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn set_test_look(on: bool) {
    TEST_LOOK.store(on, core::sync::atomic::Ordering::Relaxed);
    if on {
        // The UI task wakes the screen and frees the keys at its next
        // settings step (`KeyGate::wake_for_test`).
        TEST_WAKE.store(true, core::sync::atomic::Ordering::Relaxed);
    }
    crate::audio::reply(|r| {
        r.s(if on { b"test-look on" } else { b"test-look off" });
    });
}

/// The value `sid` reads as under the test look, if it is one of its
/// settings: Display 0x0304–0x030C (theme … roll round), language,
/// Now Playing view, Power mode (Automatic: the clock governor and the
/// marquees of a test do not follow the owner's mode).
pub fn test_look(sid: orpheus_apps::settings::SettingId) -> Option<i16> {
    // A11Y-2: text size, hold delay and key repeat too (a test's keys and
    // shots assume the defaults), and Brightness (its value shows in
    // Settings › Display and the quick panel), unless this session set
    // them.
    let armed = look_armed_bit(sid.0)
        .is_some_and(|b| LOOK_ARMED.load(core::sync::atomic::Ordering::Relaxed) & b == 0);
    // Books' reading settings (0x0F01–0x0F07): the reader's pages and the
    // library's scope in the suite's shots.
    let look = (0x0304..=0x030C).contains(&sid.0)
        || (0x0F01..=0x0F07).contains(&sid.0)
        || matches!(sid.0, 0x0401 | 0x0A01 | 0x0D03)
        || armed;
    (look && TEST_LOOK.load(core::sync::atomic::Ordering::Relaxed))
        .then(|| orpheus_apps::settings::def(sid).map_or(0, |d| d.default))
}

/// The screen timers armed by a session (`orpheus_apps::services::TimerHold`
/// bits; RAM, cleared by every reboot): under the test look or a host test
/// they run only while armed.
static TIMER_HOLD: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// The settings a session set with `settings test-set` that the test look
/// otherwise reads as their defaults (RAM, cleared by every reboot): the
/// accessibility settings (A11Y-2) and Brightness ([`look_armed_bit`]).
static LOOK_ARMED: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// `LOOK_ARMED`'s bit of `id`: Text size, Hold delay, Key repeat
/// (0x1001–0x1003, bit `id & 3`), Brightness (bit 4), Screen reader (bit
/// 5: the owner's reader is off under the test look, so a suite's keys
/// and timers are as the simulator's, until a session turns it on).
fn look_armed_bit(id: u16) -> Option<u8> {
    match id {
        0x1001..=0x1003 => Some(1 << (id & 3)),
        _ if id == orpheus_apps::settings::id::BRIGHTNESS.0 => Some(1 << 4),
        0x1004 => Some(1 << 5),
        _ => None,
    }
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

pub(crate) fn session_on() -> bool {
    SESSION.load(core::sync::atomic::Ordering::Relaxed)
}

/// Settings a session may set from the host: Sound, Playback, Display
/// (not the Home apps), Language, Files sort and hidden
/// files, Now Playing's view, Accessibility (A11Y-2) with the screen
/// reader's (A11Y-1, TTS-3). Never Bluetooth, power, clock, USB, storage,
/// alarms or the radio's station (CLAUDE.md rules 4 and 6).
fn session_settable(id: u16) -> bool {
    (matches!(id >> 8, 0x01 | 0x02 | 0x03 | 0x0A) && id != 0x030E)
        || matches!(id, 0x0D01..=0x0D03 | 0x1001..=0x100D)
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
        // Failed NOR reads since the start: command phase, data phase, cut
        // into (`nor::read_errors`; 2026-11-26). One loop: `u` once.
        let [c, d, i] = atj2157_hal::nor::read_errors();
        for (k, n) in [(b" nor=".as_slice(), c), (b"/", d), (b"/", i)] {
            r.s(k).u(u64::from(n));
        }
    });
}

/// Numbers in test commands, hex in either case (`orpheus_proto::num`).
fn parse_num(w: &[u8]) -> Option<i32> {
    orpheus_proto::num::parse(w)
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
        session_set_hooks(ui, sid, want);
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
        ui.shell.gate.saver = false;
        ui.shell.invalidate();
    }
}

/// A session's `test-set` (VER-13): the screen timers run while it set
/// one to another value than its default, and when the last goes back to
/// its default the screen comes on and the keys unlock (the journey ends
/// as it started: CLAUDE.md rule 4; under the test look they are held
/// again).
#[inline(never)]
fn session_set_hooks(ui: &mut Ui, sid: orpheus_apps::settings::SettingId, want: i16) {
    use core::sync::atomic::Ordering::Relaxed;
    let mut h = services::TimerHold::from_bits(TIMER_HOLD.load(Relaxed));
    let was = h.bits();
    h.test_set(sid, want);
    TIMER_HOLD.store(h.bits(), Relaxed);
    if let Some(b) = look_armed_bit(sid.0) {
        LOOK_ARMED.fetch_or(b, Relaxed);
    }
    if was != 0 && h.bits() == 0 {
        ui.shell.gate.screen_on = true;
        ui.shell.gate.locked = false;
        ui.shell.invalidate();
    }
}

static mut FW_CELL: core::mem::MaybeUninit<Fw> = core::mem::MaybeUninit::uninit();
static mut STORE_CELL: Option<Store<NorRegion, 4>> = None;

// Every key the firmware stores fits the store's index with room for more
// (VER-4, 2026-11-26: the index was full).
const _: () = assert!(
    orpheus_apps::settings::STORE_KEYS + orpheus_apps::settings::MIN_FREE_STORE_KEYS
        <= orpheus_settings::MAX_KEYS
);
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
    // A NOR read error at boot must not leave the player on defaults
    // (2026-11-26: `store=none` once): retried, and logged.
    let r = embassy_futures::block_on(Store::<NorRegion, 4>::mount_retrying(NorRegion::settings, 3));
    // `settings: store mounted after 1 read errors, last nor read/data`, or
    // `settings: store not mounted, defaults this start: nor read/data`.
    let fail = match &r {
        Ok((_, None)) => None,
        Ok((_, Some((n, e)))) => Some((*n, e)),
        Err(e) => Some((0, e)),
    };
    if let Some((n, e)) = fail {
        let mut l = atj2157_hal::log::LineBuf::new();
        if n > 0 {
            l.push_str("settings: store mounted after ")
                .push_u32(u32::from(n))
                .push_str(" read errors, last ");
        } else {
            l.push_str("settings: store not mounted, defaults this start: ");
        }
        l.push_str(store_error_name(e)).emit();
    }
    *slot = r.ok().map(|(s, _)| s);
    slot
}

/// The store error for the log, by NOR error kind (`nor errors` counts
/// them since start; `orpheus_pager::bus`).
fn store_error_name(e: &orpheus_settings::Error<atj2157_hal::nor::Error>) -> &'static str {
    use atj2157_hal::nor::Error as N;
    use orpheus_pager::bus::ReadFault as R;
    match e {
        orpheus_settings::Error::Nor(N::Read(R::Command)) => "nor read/command",
        orpheus_settings::Error::Nor(N::Read(R::Data)) => "nor read/data",
        orpheus_settings::Error::Nor(N::Read(R::Interleaved)) => "nor read/cut",
        orpheus_settings::Error::Nor(N::Timeout) => "nor timeout",
        orpheus_settings::Error::Nor(N::Refused) => "nor refused",
        orpheus_settings::Error::Nor(N::Verify) => "nor verify",
        _ => "store",
    }
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
    // Not entered here: the root screen's `enter` (any screen's, as far
    // as the call graph knows) would run below this frame. `enter_root`
    // does that once the shell is in its static (VER-10).
    //
    // Built field by field in its static (BOOK-1/2, 2026-11-27): built as
    // a value and moved in, the whole `Ui` was a temporary in this frame
    // (2,948 B, one of the UI task's deepest paths with the screens'
    // creation below it). The shell, the largest field, is made by a
    // function of its own that writes it into its place: this frame holds
    // no copy of it.
    let ui = (&raw mut UI_CELL).cast::<Ui>();
    // SAFETY: called once; the UI task is the only user. Every field is
    // written before the reference is made.
    unsafe {
        new_shell(&raw mut (*ui).shell, fw, t);
        ui_fields(ui, fw, store, disp, bl, t);
        &mut *ui
    }
}

/// The fields of [`new_ui`] but the shell, in a frame of their own (their
/// temporaries, 624 B, were under the shell's creation).
///
/// # Safety
/// `ui` is the UI's static, written once.
#[inline(never)]
unsafe fn ui_fields(
    ui: *mut Ui,
    fw: &'static mut Fw,
    store: &'static mut Option<Store<NorRegion, 4>>,
    disp: Display,
    bl: Backlight,
    t: Instant,
) {
    // SAFETY: the caller's.
    unsafe {
        (&raw mut (*ui).fw).write(fw);
        (&raw mut (*ui).mapper).write(KeyMapper::new());
        (&raw mut (*ui).disp).write(disp);
        (&raw mut (*ui).bl).write(bl);
        (&raw mut (*ui).screen_on).write(true);
        (&raw mut (*ui).store).write(store);
        (&raw mut (*ui).sound).write(None);
        (&raw mut (*ui).timer).write(ScreenTimer::new(t.0));
        (&raw mut (*ui).drift).write(services::ClockDrift::new());
        (&raw mut (*ui).drift_at).write(t.0);
        (&raw mut (*ui).rotated).write(false);
        #[cfg(feature = "speech")]
        (&raw mut (*ui).a11y).write(reader::A11y::new());
    }
}

/// The shell of [`new_ui`], unentered, written to `dst`.
///
/// # Safety
/// `dst` is valid for writes (the UI's static, once).
#[inline(never)]
unsafe fn new_shell(dst: *mut Shell<AppScreen, Fw>, fw: &mut Fw, t: Instant) {
    // SAFETY: the caller's.
    unsafe { dst.write(Shell::<AppScreen, Fw>::unentered(fw, screens::HOME, Arg(0), t)) }
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
    // The stock default until the settings are read (U-PWM-3).
    let bl = Backlight::new(Duty::DEFAULT);
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
        // Every stored setting and the Bluetooth address: after a factory
        // reset all of them are in the store. The keys are taken one at a
        // time by position (`get` borrows the store), not copied into a
        // list first: no list to size, and 268 B less in this task's
        // future (VER-4, 2026-11-26).
        let mut n = 0;
        let mut crc = 0xFFFF_FFFF;
        loop {
            let next = s.keys().nth(n);
            let Some(k) = next else { break };
            n += 1;
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
    // The stored Brightness at once (not after the first event).
    apply_backlight(ui);
    ui.shell.invalidate();
    let fb = fb.as_mut_slice();
    ui.draw(fb).await;
    publish_top(ui);
    let mut last_cable = state::snapshot().cable;
    // Auto-off / low-battery timers (ms since boot).
    let mut last_key_ms = now().0;
    let mut low_since = 0u64;
    let mut seen_look = false;
    loop {
        let t = now();
        let mut deadline = t.0 + 1000;
        if let Some(d) = ui.mapper.next_deadline() {
            deadline = deadline.min(d.0);
        }
        if let Some(d) = reader::deadline(ui, t.0) {
            deadline = deadline.min(d);
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
                ui.timer.key(t.0);
                // Key timing feeds the vault's entropy pool (salts, nonces).
                ui.fw.auth.mix(&atj2157_hal::time::now_us().to_le_bytes());
                // A key: a picture still coming is not waited for (the
                // volume overlay can wait).
                if down && !matches!(k, HwKey::VolUp | HwKey::VolDown) {
                    ui.fw.reclaim_picture();
                }
                reader::key_edge(ui, map_key(k), down, t);
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
                .push(Event::Service(ServiceEvent::SettingChanged(orpheus_apps::settings::id::HOME_HIDDEN.0)));
            ui.shell.invalidate();
        }
        // VER-12: `settings test-set`, `ui screen-off`.
        soak_hooks(ui);
        // VER-13: rotation, keys when locked, the screen timers, the clock's
        // correction.
        apply_settings(ui);
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
        // Games: answers of the card task, the next card operation.
        ui.fw.games_poll(t.0);
        // Books: card work changed the model.
        ui.fw.books_poll();
        while let Some((k, p)) = ui.mapper.poll(t) {
            let r = ui.shell.key(&mut *ui.fw, k, p, t);
            ui.handle(r);
            ui.settle();
            reader::observe_key(ui, k, p);
        }
        if ui.shell.wants_ticks(&*ui.fw) {
            let r = ui.shell.tick(&mut *ui.fw, t);
            ui.handle(r);
        }
        ui.after(fb).await;
        // A11Y-1: chords, the reader, Music while speaking (TTS-3).
        reader::reader_loop(ui, now());
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
        ui.draw(fb).await;
        publish_top(ui);
        // The Books model goes back to the card task until the next loop.
        crate::books::ui_release();
        BUSY_US.fetch_add((atj2157_hal::time::now_us() - w0) as u32, core::sync::atomic::Ordering::Relaxed);
    }
}
