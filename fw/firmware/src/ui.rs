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

    async fn after(&mut self) {
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
            for (k, v) in &dirty[..n] {
                let t0 = atj2157_hal::time::now_us();
                let _ = store.set(*k, v).await;
                let us = (atj2157_hal::time::now_us() - t0).min(0xFFFF) as u32;
                SET_WRITES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                SET_LAST.store(u32::from(*k) | us << 16, core::sync::atomic::Ordering::Relaxed);
            }
            // A newly connected Bluetooth device becomes the reconnect target.
            if let Some(a) = self.fw.bt_to_save.take() {
                let _ = store.set(crate::bt::LAST_KEY, &a.0).await;
            }
        }
        while let Some(a) = self.fw.actions.pop() {
            if let Action::Usb(m) = a {
                let p = match m {
                    orpheus_apps::services::UsbMode::FileTransfer => crate::usbdev::Personality::FileTransfer,
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
                    PowerRequest::Recovery => atj2157_hal::wdt::reboot(atj2157_hal::wdt::Target::Adfu),
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
        crate::PERF_MAX.store(
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
            BACKLIGHT.store(level, core::sync::atomic::Ordering::Relaxed);
            SCREEN_ON.store(on, core::sync::atomic::Ordering::Relaxed);
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
        atj2157_hal::pmu::power_off(false)
    }

    async fn draw(&mut self, fb: &mut [u16]) {
        // A screenshot is being read: do not change the frame buffer.
        let hold = SHOT_HOLD_UNTIL_MS.load(core::sync::atomic::Ordering::Relaxed);
        if hold != 0 && ((atj2157_hal::time::now_us() / 1000) as u32).wrapping_sub(hold) > u32::MAX / 2 {
            return;
        }
        // A picture job has the frame buffer (`pictures`).
        if !self.screen_on || self.fw.pic_lent.is_some() {
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
/// Backlight level (0–16) and panel on, for the battery logger (PWR-5).
pub static BACKLIGHT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(12);
pub static SCREEN_ON: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);
pub static SET_WRITES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static SET_LAST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

static mut FW_CELL: core::mem::MaybeUninit<Fw> = core::mem::MaybeUninit::uninit();
static mut STORE_CELL: Option<Store<NorRegion, 4>> = None;
static mut UI_CELL: core::mem::MaybeUninit<Ui> = core::mem::MaybeUninit::uninit();

/// Once, from the UI task.
#[inline(never)]
fn new_fw() -> &'static mut Fw {
    // SAFETY: called once; the UI task is the only user.
    unsafe { (*(&raw mut FW_CELL)).write(Fw::new()) }
}

/// Once, from the UI task.
fn store_slot() -> &'static mut Option<Store<NorRegion, 4>> {
    // SAFETY: called once; the UI task is the only user.
    unsafe { &mut *(&raw mut STORE_CELL) }
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
    let shell = Shell::<AppScreen, Fw>::with_root(fw, screens::HOME, Arg(0), t);
    let ui = Ui { fw, shell, mapper: KeyMapper::new(), disp, bl, screen_on: true, store, sound: None };
    // SAFETY: called once; the UI task is the only user.
    unsafe { (*(&raw mut UI_CELL)).write(ui) }
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
    let store = store_slot();
    *store = Store::<NorRegion, 4>::mount(NorRegion::settings()).await.ok();
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
        for &k in &keys[..n] {
            let mut b = [0u8; 8];
            if let Ok(Some(len)) = s.get(k, &mut b).await {
                fw.settings.load(k, &b[..len]);
                if k == crate::bt::LAST_KEY && len == 6 {
                    let mut a = [0u8; 6];
                    a.copy_from_slice(&b[..6]);
                    bt_last = Some(orpheus_bt::Addr(a));
                }
            }
        }
    }
    fw.init_player();
    fw.bt_boot(bt_last);
    let ui = new_ui(fw, store, disp, bl, now());
    ui.shell.invalidate();
    let fb = fb.as_mut_slice();
    ui.draw(fb).await;
    let mut last_cable = state::snapshot().cable;
    // Auto-off / low-battery timers (ms since boot).
    let mut last_key_ms = now().0;
    let mut low_since = 0u64;
    loop {
        let t = now();
        let mut deadline = t.0 + 1000;
        if let Some(d) = ui.mapper.next_deadline() {
            deadline = deadline.min(d.0);
        }
        if ui.shell.wants_ticks(&*ui.fw) {
            deadline = deadline.min(t.0 + u64::from(TICK_MS));
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
        let w0 = atj2157_hal::time::now_us();
        LOOPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        atj2157_hal::wdt::beat(&atj2157_hal::wdt::UI_BEAT);
        ui.fw.sync_player();
        ui.fw.sync_bt();
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
        ui.after().await;
        // PWR-3a: auto-off and low battery (both through `power_off`, which
        // reboots instead under a host test).
        {
            let t = now().0;
            let st = state::snapshot();
            let idle = matches!(
                ui.fw.player.source,
                orpheus_ui::event::Source::Idle | orpheus_ui::event::Source::Paused
            );
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
        BUSY_US.fetch_add((atj2157_hal::time::now_us() - w0) as u32, core::sync::atomic::Ordering::Relaxed);
    }
}
