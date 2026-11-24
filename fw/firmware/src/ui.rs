//! The UI task: key mapper → shell → screens → frame buffer → panel.

use atj2157_hal::dma;
use embassy_futures::select::{Either4, select4};
use embassy_time::{Duration, Instant as TInstant, Timer};
use yoton_apps::screens::{self, AppScreen};
use yoton_apps::services::{self, PowerRequest};
use yoton_bsp::display::{self, Backlight, Display};
use yoton_bsp::settings::{NorRegion, Store};
use yoton_input::Key as HwKey;
use yoton_ui::event::{Event, Instant, Key, ServiceEvent};
use yoton_ui::gfx::{DirtySet, Gfx};
use yoton_ui::input::KeyMapper;
use yoton_ui::nav::Arg;
use yoton_ui::shell::{Requests, Shell, TICK_MS};

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
    fw: Fw,
    shell: Shell<AppScreen, Fw>,
    mapper: KeyMapper,
    disp: Display,
    bl: Backlight,
    screen_on: bool,
    store: Option<Store<NorRegion, 4>>,
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
            let r = self.shell.event(&mut self.fw, ev, now());
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
        }
        while let Some(a) = self.fw.actions.pop() {
            if let Action::Usb(m) = a {
                let p = match m {
                    yoton_apps::services::UsbMode::FileTransfer => crate::usbdev::Personality::FileTransfer,
                    _ => crate::usbdev::Personality::Dev,
                };
                crate::usbdev::MSC_READ_ONLY.store(false, core::sync::atomic::Ordering::Relaxed);
                crate::usbdev::MODE.signal(p);
            }
            if let Action::Power(p) = a {
                match p {
                    PowerRequest::Restart => atj2157_hal::wdt::reboot(atj2157_hal::wdt::Target::Normal),
                    PowerRequest::Recovery => atj2157_hal::wdt::reboot(atj2157_hal::wdt::Target::Adfu),
                    PowerRequest::LockKeys => self.shell.gate.locked = true,
                    // Power-off (PWR-3a) and the stock-firmware request
                    // (needs a stage-2 mailbox path) are not wired yet.
                    _ => {}
                }
            }
        }
        // Screen on/off follows the gate.
        let on = self.shell.gate.screen_on;
        if on != self.screen_on {
            self.screen_on = on;
            self.disp.sleep(!on);
            self.bl.set(if on { 12 } else { 0 });
            if on {
                self.shell.invalidate();
            }
        }
    }

    async fn draw(&mut self, fb: &mut [u16]) {
        if !self.screen_on {
            return;
        }
        let mut dirty = DirtySet::new();
        let t0 = atj2157_hal::time::now_us();
        {
            let pal = services::palette(&self.fw);
            let mut g = Gfx::new(fb, &mut dirty, pal, None);
            self.shell.draw(&mut self.fw, &mut g);
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
pub static SET_WRITES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static SET_LAST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[embassy_executor::task]
pub async fn ui() {
    let Some(fb) = display::take_framebuffer() else { return };
    // SAFETY: DMA channel 1 is the display's (01-hardware §6).
    let disp = Display::new(unsafe { dma::Channel::steal(1) });
    let bl = Backlight::new(12);
    let mut fw = Fw::new();
    crate::services::apply_volume(fw.volume);
    // Settings from NOR.
    let mut store = Store::<NorRegion, 4>::mount(NorRegion::settings()).await.ok();
    if let Some(s) = store.as_mut() {
        let mut keys = [0u16; 64];
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
            }
        }
    }
    let t = now();
    let shell = Shell::<AppScreen, Fw>::with_root(&mut fw, screens::HOME, Arg(0), t);
    let mut ui = Ui { fw, shell, mapper: KeyMapper::new(), disp, bl, screen_on: true, store };
    ui.shell.invalidate();
    let fb = fb.as_mut_slice();
    ui.draw(fb).await;
    let mut last_cable = state::snapshot().cable;
    loop {
        let t = now();
        let mut deadline = t.0 + 1000;
        if let Some(d) = ui.mapper.next_deadline() {
            deadline = deadline.min(d.0);
        }
        if ui.shell.wants_ticks(&ui.fw) {
            deadline = deadline.min(t.0 + u64::from(TICK_MS));
        }
        let wait = Duration::from_millis(deadline.saturating_sub(t.0).max(1));
        match select4(KEYS.receive(), BATTERY.receive(), Timer::after(wait), crate::media::READY.receive())
            .await
        {
            Either4::Fourth(tag) => {
                ui.fw.sync(tag);
                let (first, count) = (ui.fw.browse.first, ui.fw.browse.n);
                let ev = Event::Data(yoton_ui::event::DataReady { tag, first, count });
                let r = ui.shell.event(&mut ui.fw, ev, now());
                ui.handle(r);
            }
            Either4::First((k, down)) => {
                let t = now();
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
                let r = ui.shell.event(&mut ui.fw, ev, now());
                ui.handle(r);
                if s.cable != last_cable {
                    last_cable = s.cable;
                    let r =
                        ui.shell.event(&mut ui.fw, Event::Service(ServiceEvent::Usb { host: false }), now());
                    ui.handle(r);
                }
            }
            Either4::Third(()) => {}
        }
        let w0 = atj2157_hal::time::now_us();
        LOOPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        atj2157_hal::wdt::beat(&atj2157_hal::wdt::UI_BEAT);
        ui.fw.sync_player();
        let t = now();
        while let Some((k, p)) = ui.mapper.poll(t) {
            let r = ui.shell.key(&mut ui.fw, k, p, t);
            ui.handle(r);
            ui.settle();
        }
        if ui.shell.wants_ticks(&ui.fw) {
            let r = ui.shell.tick(&mut ui.fw, t);
            ui.handle(r);
        }
        ui.after().await;
        ui.draw(fb).await;
        BUSY_US.fetch_add((atj2157_hal::time::now_us() - w0) as u32, core::sync::atomic::Ordering::Relaxed);
    }
}
