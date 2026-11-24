//! The firmware's implementation of the `yoton-apps` service traits, from
//! state the tasks publish (battery, cable) and the settings cache.

use core::sync::atomic::Ordering;

use yoton_apps::services::{
    self, BatteryInfo, BluetoothService, CardInfo, CardState, ClockService, DateTime, PlaybackService,
    Playing, PowerRequest, PowerService, SettingsService, StorageRequest, StorageService, SystemService,
    UsbMode, UsbService, UsbState,
};
use yoton_apps::settings::{SettingId, SettingsCache, id};
use yoton_apps::{App, Lang};
use yoton_ui::event::{BtState, Event, ServiceEvent};
use yoton_ui::shell::System;
use yoton_ui::widgets::bars::{MiniPlayer, Status};

use crate::state;

/// Requests the UI made that other tasks carry out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Power(PowerRequest),
    Storage(StorageRequest),
    Usb(UsbMode),
}

pub struct Fw {
    pub settings: SettingsCache,
    pub volume: u8,
    pub locked: bool,
    pub pending: heapless_queue::Queue<Event, 8>,
    pub actions: heapless_queue::Queue<Action, 8>,
}

/// A tiny fixed queue (no external dependency, no allocation).
pub mod heapless_queue {
    pub struct Queue<T: Copy, const N: usize> {
        buf: [Option<T>; N],
        len: usize,
    }
    impl<T: Copy, const N: usize> Queue<T, N> {
        #[must_use]
        pub const fn new() -> Self {
            Self { buf: [None; N], len: 0 }
        }
        pub fn push(&mut self, v: T) {
            if self.len < N {
                self.buf[self.len] = Some(v);
                self.len += 1;
            }
        }
        pub fn pop(&mut self) -> Option<T> {
            if self.len == 0 {
                return None;
            }
            let v = self.buf[0];
            self.buf.copy_within(1..self.len, 0);
            self.len -= 1;
            self.buf[self.len] = None;
            v
        }
    }
    impl<T: Copy, const N: usize> Default for Queue<T, N> {
        fn default() -> Self {
            Self::new()
        }
    }
}

impl Fw {
    #[must_use]
    pub fn new() -> Self {
        Self {
            settings: SettingsCache::new(),
            volume: 12,
            locked: false,
            pending: heapless_queue::Queue::new(),
            actions: heapless_queue::Queue::new(),
        }
    }
}

impl SettingsService for Fw {
    fn setting(&self, sid: SettingId) -> i16 {
        self.settings.get(sid)
    }
    fn set_setting(&mut self, sid: SettingId, value: i16) {
        if self.settings.set(sid, value) {
            self.pending.push(Event::Service(ServiceEvent::SettingChanged(sid.0)));
        }
    }
    fn factory_reset(&mut self) {
        self.settings.reset();
    }
}

impl PowerService for Fw {
    fn battery(&self) -> BatteryInfo {
        let s = state::snapshot();
        BatteryInfo { percent: s.percent, millivolts: s.millivolts, charge: s.charge }
    }
    fn sleep_timer_left(&self) -> Option<u16> {
        None
    }
    fn power(&mut self, req: PowerRequest) {
        if req == PowerRequest::LockKeys {
            self.locked = true;
        }
        self.actions.push(Action::Power(req));
    }
}

impl ClockService for Fw {
    fn now(&self) -> DateTime {
        let n = atj2157_hal::rtc_clock::now();
        // 2000-01-01 was a Saturday (weekday 5 with Monday = 0).
        let days = atj2157_hal::rtc_clock::epoch2000(n) / 86_400;
        DateTime {
            year: n.year,
            month: n.month,
            day: n.day,
            hour: n.hour,
            minute: n.minute,
            second: n.second,
            weekday: ((days + 5) % 7) as u8,
        }
    }
    fn set_now(&mut self, t: DateTime) {
        let _ = atj2157_hal::rtc_clock::set(atj2157_hal::rtc_clock::DateTime {
            year: t.year,
            month: t.month,
            day: t.day,
            hour: t.hour,
            minute: t.minute,
            second: t.second,
        });
    }
    fn clock_valid(&self) -> bool {
        atj2157_hal::rtc_clock::running() && atj2157_hal::rtc_clock::now().year >= 2026
    }
}

impl StorageService for Fw {
    fn card(&self) -> CardState {
        if state::CARD_READY.load(Ordering::Relaxed) { CardState::Ready } else { CardState::Absent }
    }
    fn card_info(&self) -> Option<CardInfo> {
        None
    }
    fn storage(&mut self, req: StorageRequest) {
        self.actions.push(Action::Storage(req));
    }
}

impl PlaybackService for Fw {
    fn playing(&self) -> Option<Playing<'_>> {
        None
    }
    fn volume_step(&mut self, delta: i8) -> (u8, u8) {
        let max = u8::try_from(self.settings.get(id::VOLUME_LIMIT)).unwrap_or(30);
        let v = i16::from(self.volume) + i16::from(delta);
        self.volume = u8::try_from(v.clamp(0, i16::from(max))).unwrap_or(0);
        (self.volume, max)
    }
}

impl UsbService for Fw {
    fn usb(&self) -> UsbState {
        if state::snapshot().cable { UsbState::Host } else { UsbState::Detached }
    }
    fn select_usb(&mut self, mode: UsbMode) {
        self.actions.push(Action::Usb(mode));
    }
}

impl BluetoothService for Fw {
    fn bt(&self) -> BtState {
        BtState::Off
    }
    fn bt_device(&self) -> &str {
        ""
    }
}

impl SystemService for Fw {
    fn lang(&self) -> Lang {
        Lang::ENGLISH
    }
    fn language_count(&self) -> usize {
        1
    }
    fn language(&self, _i: usize) -> Lang {
        Lang::ENGLISH
    }
    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }
    fn build_id(&self) -> &str {
        "dev"
    }
    fn serial(&self) -> &str {
        "YM03"
    }
    fn hardware(&self) -> &str {
        "Yoton YM03 (ATJ2157)"
    }
    fn app_available(&self, app: App) -> bool {
        matches!(app, App::Settings)
    }
    fn dual_boot(&self) -> bool {
        true
    }
    fn usb_audio_built(&self) -> bool {
        false
    }
    fn keys_locked(&self) -> bool {
        self.locked
    }
}

impl System for Fw {
    fn status(&self) -> Status {
        services::status(self)
    }
    fn volume_step(&mut self, delta: i8) -> (u8, u8) {
        PlaybackService::volume_step(self, delta)
    }
    fn mini_player(&self) -> Option<MiniPlayer<'_>> {
        services::mini_player(self)
    }
}
