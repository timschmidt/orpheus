//! orpheus-apps: the screens of 05 §6–7 over the service traits of 05 §5.1.
//!
//! `no_std`, no allocator, no hardware. WP UI-2 provides Home, Settings
//! (all groups) and the system screens (boot, charging, power menu, lock
//! screen, quick panel, notices, USB connect dialog, goodbye). The firmware
//! implements [`services::Services`] and runs
//! `orpheus_ui::Shell<screens::AppScreen, Firmware>`.

#![no_std]
#![forbid(unsafe_code)]

pub mod browse;
#[cfg(feature = "games")]
pub mod games;
pub mod icons;
#[cfg(test)]
mod id_tests;
pub mod screens;
pub mod services;
pub mod settings;
pub mod strings;
pub mod tools;

pub use screens::AppScreen;
pub use services::Services;
pub use strings::{Lang, Str};

use orpheus_ui::icons::Icon2;

/// True if `word` fits in `w` pixels, or is CJK (which breaks anywhere).
#[doc(hidden)]
pub fn strings_fit(g: &mut orpheus_ui::Gfx<'_>, word: &str, font: orpheus_ui::FontId, w: i16) -> bool {
    word.chars().any(orpheus_ui::text::is_cjk) || g.measure(word, font) <= w
}

/// The apps on Home, in grid order (05 §6.2): eleven, and Games (M14) as
/// the twelfth with the `games` feature (not on the player's Home yet:
/// [`App::on_player`]).
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
    /// M14 (05 §7.13). After Settings so that the other apps keep their
    /// Home-screen bits; its bit (11) is off in the default mask, as the
    /// player's Home has no Games yet (the simulator's `set:games=on`
    /// turns it on).
    #[cfg(feature = "games")]
    Games,
}

impl App {
    /// How many apps there are.
    pub const COUNT: usize = if cfg!(feature = "games") { 12 } else { 11 };

    pub const ALL: [App; App::COUNT] = [
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
        #[cfg(feature = "games")]
        App::Games,
    ];

    /// Home's grid order: [`App::ALL`] with Settings last (Prev on Home's
    /// first cell reaches it).
    pub const GRID: [App; App::COUNT] = [
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
        #[cfg(feature = "games")]
        App::Games,
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
            #[cfg(feature = "games")]
            App::Games => Str::AppGames,
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
            #[cfg(feature = "games")]
            App::Games => &icons::GAMES,
        }
    }

    /// Apps that cannot work without the card (05 §6.6).
    #[must_use]
    pub const fn needs_card(self) -> bool {
        matches!(self, App::Music | App::Files | App::Pictures | App::Recorder | App::UsbDrive)
    }

    /// Built and verified on the player, so the firmware offers it on Home
    /// (its `SystemService::app_available`; all of them since 2026-11-24).
    /// Exhaustive on purpose: a new app is a decision here.
    #[must_use]
    pub const fn on_player(self) -> bool {
        match self {
            App::Music
            | App::Files
            | App::Radio
            | App::Bluetooth
            | App::Recorder
            | App::Pictures
            | App::Steps
            | App::Stopwatch
            | App::Authenticator
            | App::UsbDrive
            | App::Settings => true,
            // Not until the firmware has the stack for it (M14 package
            // note in docs/STATUS.md).
            #[cfg(feature = "games")]
            App::Games => false,
        }
    }

    #[must_use]
    pub fn from_index(i: u8) -> Option<App> {
        App::ALL.get(usize::from(i)).copied()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use orpheus_ui::theme::{Accent, Theme};
    use orpheus_ui::{DirtySet, FontId, Gfx};

    /// Regression: Recorder and USB Drive were missing from Home on the
    /// player (VER-7, 2026-11-24, first on-device suite run): the
    /// firmware's `app_available` still listed only the apps that existed
    /// when it was last edited. It now asks [`App::on_player`].
    #[test]
    fn regress_every_app_is_on_the_players_home() {
        for a in App::ALL {
            #[cfg(feature = "games")]
            if a == App::Games {
                // M14: built on the host, not yet integrated.
                assert!(!a.on_player());
                continue;
            }
            assert!(a.on_player(), "{a:?} missing from the player's Home");
        }
    }

    /// Regression: System › About on the player shortened its Hardware row
    /// to "Har…" (VER-7, 2026-11-24, shot system/about): the firmware's
    /// hardware string "Yoton YM03 (ATJ2157)" left the label no room. Label
    /// and value must fit side by side in a row of a scrolling list
    /// (`ListView::draw_row`: label at x 7, value right-aligned 7 px from
    /// the edge, 6 px between them, 4 px for the scroll thumb).
    #[test]
    fn regress_about_hardware_row_fits() {
        let mut fb = std::vec![0u16; orpheus_ui::gfx::FB_PIXELS];
        let mut d = DirtySet::new();
        let mut g = Gfx::new(&mut fb, &mut d, Theme::Dark.palette(Accent::Blue), None);
        let avail = orpheus_ui::gfx::SCREEN_W - 4 - 7 - 7 - 6;
        let fits = |g: &mut Gfx<'_>, value: &str| {
            g.measure(Lang::ENGLISH.get(Str::Hardware), FontId::Ui) + g.measure(value, FontId::Ui) <= avail
        };
        assert!(fits(&mut g, services::PLAYER_HARDWARE));
        assert!(!fits(&mut g, "Yoton YM03 (ATJ2157)"), "the old string");
    }
}
