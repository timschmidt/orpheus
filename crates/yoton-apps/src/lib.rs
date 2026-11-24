//! yoton-apps: the screens of 05 §6–7 over the service traits of 05 §5.1.
//!
//! `no_std`, no allocator, no hardware. WP UI-2 provides Home, Settings
//! (all groups) and the system screens (boot, charging, power menu, lock
//! screen, quick panel, notices, USB connect dialog, goodbye). The firmware
//! implements [`services::Services`] and runs
//! `yoton_ui::Shell<screens::AppScreen, Firmware>`.

#![no_std]
#![forbid(unsafe_code)]

pub mod icons;
pub mod screens;
pub mod services;
pub mod settings;
pub mod strings;

pub use screens::AppScreen;
pub use services::Services;
pub use strings::{Lang, Str};

use yoton_ui::icons::Icon2;

/// The eleven apps on Home, in grid order (05 §6.2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum App {
    Music,
    Files,
    Radio,
    Bluetooth,
    Recorder,
    Pictures,
    Steps,
    Stopwatch,
    Authenticator,
    UsbDrive,
    Settings,
}

impl App {
    pub const ALL: [App; 11] = [
        App::Music,
        App::Files,
        App::Radio,
        App::Bluetooth,
        App::Recorder,
        App::Pictures,
        App::Steps,
        App::Stopwatch,
        App::Authenticator,
        App::UsbDrive,
        App::Settings,
    ];

    #[must_use]
    pub const fn label(self) -> Str {
        match self {
            App::Music => Str::AppMusic,
            App::Files => Str::AppFiles,
            App::Radio => Str::AppRadio,
            App::Bluetooth => Str::AppBluetooth,
            App::Recorder => Str::AppRecorder,
            App::Pictures => Str::AppPictures,
            App::Steps => Str::AppSteps,
            App::Stopwatch => Str::AppStopwatch,
            App::Authenticator => Str::AppAuthenticator,
            App::UsbDrive => Str::AppUsbDrive,
            App::Settings => Str::AppSettings,
        }
    }

    #[must_use]
    pub fn icon(self) -> &'static Icon2 {
        match self {
            App::Music => &icons::MUSIC,
            App::Files => &icons::FILES,
            App::Radio => &icons::RADIO,
            App::Bluetooth => &icons::BLUETOOTH,
            App::Recorder => &icons::RECORDER,
            App::Pictures => &icons::PICTURES,
            App::Steps => &icons::STEPS,
            App::Stopwatch => &icons::STOPWATCH,
            App::Authenticator => &icons::AUTHENTICATOR,
            App::UsbDrive => &icons::USB_DRIVE,
            App::Settings => &icons::SETTINGS,
        }
    }

    /// Apps that cannot work without the card (05 §6.6).
    #[must_use]
    pub const fn needs_card(self) -> bool {
        matches!(self, App::Music | App::Files | App::Pictures | App::Recorder | App::UsbDrive)
    }

    #[must_use]
    pub fn from_index(i: u8) -> Option<App> {
        App::ALL.get(usize::from(i)).copied()
    }
}
