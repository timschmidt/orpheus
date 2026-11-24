//! The screens and their dispatch enum (static, no heap: only the top
//! screen holds state; covered screens are rebuilt from resume records).

mod edit;
mod home;
mod lists;
mod rows;
mod system;

pub use edit::{PickerScreen, SliderScreen};
pub use home::HomeScreen;
pub use lists::{ListKind, ListScreen};
pub use rows::{MAX_ROWS, RowData, Rows};
pub use system::{BootScreen, ChargingScreen, GoodbyeScreen, LockScreen, Notice, NoticeScreen};

use yoton_ui::event::{Event, ServiceEvent};
use yoton_ui::gfx::Gfx;
use yoton_ui::menu::MenuBuilder;
use yoton_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, ScreenId, ScreenSet, Wants};

use crate::services::{Services, UsbMode};
use crate::settings::id;
use crate::strings::Str;

pub const HOME: ScreenId = ScreenId(0);
pub const BOOT: ScreenId = ScreenId(1);
pub const SETTINGS: ScreenId = ScreenId(2);
/// Arg: group index.
pub const GROUP: ScreenId = ScreenId(3);
/// Arg: setting key.
pub const CHOICE: ScreenId = ScreenId(4);
/// Arg: setting key.
pub const SLIDER: ScreenId = ScreenId(5);
/// Arg: 0 time, 1 date.
pub const PICKER: ScreenId = ScreenId(6);
/// Arg: `Info as u32`.
pub const INFO: ScreenId = ScreenId(7);
pub const HOME_APPS: ScreenId = ScreenId(8);
pub const POWER_MENU: ScreenId = ScreenId(9);
pub const LOCK: ScreenId = ScreenId(10);
pub const QUICK_PANEL: ScreenId = ScreenId(11);
/// Arg: [`Notice::arg`].
pub const NOTICE: ScreenId = ScreenId(12);
pub const CHARGING: ScreenId = ScreenId(13);
pub const USB_CONNECT: ScreenId = ScreenId(14);
pub const GOODBYE: ScreenId = ScreenId(15);

/// One variant per screen. Only the top screen exists, so the enum's size
/// (its largest variant, the list screen) is the UI's screen RAM.
#[allow(clippy::large_enum_variant)]
pub enum AppScreen {
    Boot(BootScreen),
    Home(HomeScreen),
    List(ListScreen),
    Slider(SliderScreen),
    Picker(PickerScreen),
    Lock(LockScreen),
    Notice(NoticeScreen),
    Charging(ChargingScreen),
    Goodbye(GoodbyeScreen),
}

macro_rules! each {
    ($self:ident, $s:ident => $e:expr) => {
        match $self {
            AppScreen::Boot($s) => $e,
            AppScreen::Home($s) => $e,
            AppScreen::List($s) => $e,
            AppScreen::Slider($s) => $e,
            AppScreen::Picker($s) => $e,
            AppScreen::Lock($s) => $e,
            AppScreen::Notice($s) => $e,
            AppScreen::Charging($s) => $e,
            AppScreen::Goodbye($s) => $e,
        }
    };
}

impl<S: Services + ?Sized> Screen<S> for AppScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        each!(self, s => s.enter(cx, arg));
    }
    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        each!(self, s => s.event(cx, ev))
    }
    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        each!(self, s => s.draw(cx, g, dirty));
    }
    fn menu(&self, cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        each!(self, s => s.menu(cx, out));
    }
    fn wants(&self) -> Wants {
        each!(self, s => Screen::<S>::wants(s))
    }
    fn save(&self) -> Resume {
        each!(self, s => Screen::<S>::save(s))
    }
    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        each!(self, s => s.restore(cx, r));
    }
}

impl<S: Services + ?Sized> ScreenSet<S> for AppScreen {
    const HOME: ScreenId = HOME;
    const QUICK_PANEL: Option<ScreenId> = Some(QUICK_PANEL);
    const LOCK_SCREEN: Option<ScreenId> = Some(LOCK);
    const POWER_MENU: Option<ScreenId> = Some(POWER_MENU);

    fn create(sid: ScreenId) -> Self {
        match sid {
            BOOT => AppScreen::Boot(BootScreen::default()),
            SETTINGS => AppScreen::List(ListScreen::new(ListKind::Root)),
            GROUP => AppScreen::List(ListScreen::new(ListKind::Group(0))),
            CHOICE => AppScreen::List(ListScreen::new(ListKind::Choice(id::THEME))),
            INFO => AppScreen::List(ListScreen::new(ListKind::Info(crate::settings::Info::About))),
            HOME_APPS => AppScreen::List(ListScreen::new(ListKind::HomeApps)),
            POWER_MENU => AppScreen::List(ListScreen::new(ListKind::PowerMenu)),
            QUICK_PANEL => AppScreen::List(ListScreen::new(ListKind::Quick)),
            USB_CONNECT => AppScreen::List(ListScreen::new(ListKind::Usb)),
            SLIDER => AppScreen::Slider(SliderScreen::default()),
            PICKER => AppScreen::Picker(PickerScreen::default()),
            LOCK => AppScreen::Lock(LockScreen::default()),
            NOTICE => AppScreen::Notice(NoticeScreen::default()),
            CHARGING => AppScreen::Charging(ChargingScreen::default()),
            GOODBYE => AppScreen::Goodbye(GoodbyeScreen::default()),
            _ => AppScreen::Home(HomeScreen::default()),
        }
    }

    fn id(&self) -> ScreenId {
        match self {
            AppScreen::Boot(_) => BOOT,
            AppScreen::Home(_) => HOME,
            AppScreen::List(l) => l.screen_id(),
            AppScreen::Slider(_) => SLIDER,
            AppScreen::Picker(_) => PICKER,
            AppScreen::Lock(_) => LOCK,
            AppScreen::Notice(_) => NOTICE,
            AppScreen::Charging(_) => CHARGING,
            AppScreen::Goodbye(_) => GOODBYE,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            AppScreen::Boot(_) => "Boot",
            AppScreen::Home(_) => "Home",
            AppScreen::List(l) => l.name(),
            AppScreen::Slider(_) => "Slider",
            AppScreen::Picker(_) => "Picker",
            AppScreen::Lock(_) => "Lock",
            AppScreen::Notice(_) => "Notice",
            AppScreen::Charging(_) => "Charging",
            AppScreen::Goodbye(_) => "Goodbye",
        }
    }

    /// Low battery (05 §6.1), computer connected (§6.5), card removed,
    /// theme and language changes.
    fn global(cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        let Event::Service(se) = *ev else { return Nav::Stay };
        let lang = cx.svc.lang();
        match se {
            ServiceEvent::Battery { percent, charging: false } => match percent {
                15 | 10 => {
                    let mut b = yoton_ui::text::StrBuf::<48>::new();
                    b.push_str(lang.get(Str::BatteryLow));
                    b.push_str(" · ");
                    b.push_u32(u32::from(percent));
                    b.push_str(" %");
                    cx.ui.toast(b.as_str());
                    Nav::Stay
                }
                5 => Nav::Push(NOTICE, Arg(Notice::BatteryLow.arg())),
                _ => Nav::Stay,
            },
            ServiceEvent::Usb { host: true } => match cx.svc.setting(id::USB_WHEN_CONNECTED) {
                0 => Nav::Push(USB_CONNECT, Arg(0)),
                n => {
                    let mode = match n {
                        2 => UsbMode::FileTransfer,
                        3 => UsbMode::Keyboard,
                        4 => UsbMode::DiskImage,
                        _ => UsbMode::ChargeOnly,
                    };
                    cx.svc.select_usb(mode);
                    Nav::Stay
                }
            },
            ServiceEvent::CardRemoved => {
                cx.ui.toast(lang.get(Str::CardRemoved));
                Nav::Stay
            }
            ServiceEvent::SettingChanged(k) if [id::THEME.0, id::ACCENT.0, id::LANGUAGE.0].contains(&k) => {
                cx.ui.invalidate_all();
                Nav::Stay
            }
            _ => Nav::Stay,
        }
    }
}
