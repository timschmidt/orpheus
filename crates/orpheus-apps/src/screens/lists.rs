//! Every list-shaped screen: Settings root and groups (05 §7.11), choice
//! lists, information pages, Home-screen editor, power menu (§3), quick
//! panel (§6.4) and the USB connect dialog (§6.5). One struct, behaviour
//! by [`ListKind`].

use orpheus_power::charger::ChargeState;
use orpheus_ui::event::{Event, Key, KeyPhase, UiEvent};
use orpheus_ui::gfx::Gfx;
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, ScreenId, StripUse, Wants};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;
use orpheus_ui::widgets::dialog::DialogSpec;
use orpheus_ui::widgets::list::{Accessory, ListAction, ListView};

use super::rows::Rows;
use super::{
    BOOKMARKS, CHOICE, DEVELOPER, GOODBYE, GROUP, HOME_APPS, INFO, LIBRARY, LibraryScreen, MUSIC,
    MUSIC_FOLDERS, NOTICE, NOW_PLAYING, Notice, PICKER, POWER_MENU, PROPERTIES, QUICK_PANEL, SETTINGS,
    SLIDER, USB_CONNECT,
};
use crate::App;
use crate::services::{
    BootSlot, FsKind, LibList, LibraryState, PlayerCmd, PowerRequest, Services, StorageRequest, UsbMode,
};
use crate::settings::{self, Action, GROUPS, Info, Kind, SettingId, format_value, id};
use crate::strings::{Lang, Str};

/// Which list this is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ListKind {
    Root,
    Group(u8),
    Choice(SettingId),
    Info(Info),
    HomeApps,
    PowerMenu,
    Quick,
    Usb,
    /// Music root (05 §7.1).
    MusicRoot,
    Bookmarks,
    /// Properties of row `n` of the current listing.
    Properties(u32),
    /// Settings › System › Developer: the battery logger (PWR-5).
    Developer,
}

// Row tags that are not setting keys.
const T_POWER_OFF: u16 = 1;
const T_LOCK: u16 = 2;
const T_RESTART: u16 = 3;
const T_ORIGINAL: u16 = 4;
const T_RECOVERY: u16 = 5;
const T_NOW_PLAYING: u16 = 6;
const T_ALWAYS: u16 = 100;
const T_NP: u16 = 200;
const T_RESUME: u16 = 201;
const T_SHUFFLE_ALL: u16 = 202;
const T_LIBRARY: u16 = 210;
const T_FOLDERS: u16 = 220;
const T_BOOKMARKS: u16 = 221;
const T_BUILDING: u16 = 222;
const T_DRAIN: u16 = 300;
const T_STAY: u16 = 301;
const T_LOG_FILE: u16 = 302;
const T_LOG_ROWS: u16 = 303;
/// OK presses in About that show the developer rows.
const DEV_TAPS: u8 = 5;

/// Music root (05 §7.1): Now Playing, Resume, Shuffle all, the library
/// lists (usable once the tag index exists), Folders, Bookmarks.
fn music_rows<S: Services + ?Sized>(svc: &S, l: Lang, rows: &mut Rows) {
    let p = svc.player();
    if p.queue_len > 0
        && let Some(r) = rows.push(l.get(Str::NowPlaying), T_NP)
    {
        r.icon = Some(&orpheus_ui::icons::PLAY);
        r.acc = Accessory::Chevron;
    }
    if p.source == orpheus_ui::event::Source::Idle
        && let Some(t) = svc.resume_title()
        && let Some(r) = rows.push(l.get(Str::ResumeTitle), T_RESUME)
    {
        r.value.push_str_or_ellipsis(t);
        r.wide_value = true;
    }
    rows.push(l.get(Str::ShuffleAll), T_SHUFFLE_ALL);
    let lib = svc.library();
    // The file count does not fit a 128 px row in every language; the row
    // says that the index is being built.
    if let LibraryState::Building { .. } = lib
        && let Some(r) = rows.push(l.get(Str::BuildingLibrary), T_BUILDING)
    {
        r.dim = true;
    }
    let ready = lib == LibraryState::Ready;
    for (i, s) in [Str::Artists, Str::Albums, Str::Songs, Str::Genres, Str::Playlists, Str::RecentlyAdded]
        .iter()
        .enumerate()
    {
        if let Some(r) = rows.push(l.get(*s), T_LIBRARY + u16::try_from(i).unwrap_or(0)) {
            r.dim = !ready;
            r.acc = Accessory::Chevron;
        }
    }
    if let Some(r) = rows.push(l.get(Str::Folders), T_FOLDERS) {
        r.acc = Accessory::Chevron;
        r.icon = Some(&orpheus_ui::icons::FOLDER);
    }
    if let Some(r) = rows.push(l.get(Str::Bookmarks), T_BOOKMARKS) {
        r.acc = Accessory::Chevron;
    }
}

/// Properties of a listing row (05 §7.2).
fn properties_rows<S: Services + ?Sized>(svc: &S, l: Lang, row: u32, rows: &mut Rows) {
    let Some(e) = svc.entry(row) else { return };
    let mut add = |label: Str, f: &dyn Fn(&mut StrBuf<24>)| {
        if let Some(r) = rows.push(l.get(label), 0) {
            f(&mut r.value);
            r.wide_value = true;
        }
    };
    add(Str::Name, &|v| {
        v.push_str_or_ellipsis(e.name);
    });
    add(Str::Type, &|v| {
        v.push_str_or_ellipsis(l.get(super::kind_label(e.kind)));
    });
    if e.kind != crate::browse::EntryKind::Dir {
        add(Str::Size, &|v| super::push_size(v, e.size));
    }
    let fmt = svc.setting(id::DATE_FORMAT);
    add(Str::Modified, &|v| {
        let mut d = StrBuf::<24>::new();
        super::system::push_date(&mut d, &e.modified, fmt);
        v.push_str(d.as_str());
    });
}
/// Settings › System › Developer (PWR-5): the battery logger, whether it
/// keeps auto-off away, its file and the rows written.
fn developer_rows<S: Services + ?Sized>(svc: &S, l: Lang, rows: &mut Rows) {
    let d = svc.drain_log();
    if let Some(r) = rows.push(l.get(Str::BatteryLog), T_DRAIN) {
        r.acc = Accessory::Toggle(d.on);
    }
    if let Some(r) = rows.push(l.get(Str::KeepAwake), T_STAY) {
        r.acc = Accessory::Toggle(d.stay);
    }
    if let Some(r) = rows.push(l.get(Str::FileLabel), T_LOG_FILE) {
        if d.on && d.file != 0 {
            r.value.push_str("drain-");
            r.value.push_u32(u32::from(d.file));
        } else {
            r.value.push_str("—");
        }
        r.dim = !d.on;
    }
    if let Some(r) = rows.push(l.get(Str::LogRows), T_LOG_ROWS) {
        r.value.push_u32(d.rows);
        r.dim = !d.on;
    }
}

/// Dialog ids: action number + this.
const DLG: u16 = 0x100;

pub struct ListScreen {
    kind: ListKind,
    list: ListView,
    rows: Rows,
    lang: Lang,
    always: bool,
    /// OK presses in About (the developer rows' unlock).
    taps: u8,
}

fn rows_visible() -> u32 {
    5
}

fn info_from(arg: u32) -> Info {
    match arg {
        0 => Info::Battery,
        1 => Info::CardInfo,
        3 => Info::Diagnostics,
        4 => Info::DeviceName,
        5 => Info::RecFolder,
        _ => Info::About,
    }
}

const fn info_arg(i: Info) -> u32 {
    match i {
        Info::Battery => 0,
        Info::CardInfo => 1,
        Info::About => 2,
        Info::Diagnostics => 3,
        Info::DeviceName => 4,
        Info::RecFolder => 5,
    }
}

const fn action_code(a: Action) -> u16 {
    match a {
        Action::SetTime => 0,
        Action::SetDate => 1,
        Action::RebuildLibrary => 2,
        Action::ClearCaches => 3,
        Action::CheckCard => 4,
        Action::FormatCard => 5,
        Action::UpdateFirmware => 6,
        Action::StartOriginal => 7,
        Action::FactoryReset => 8,
        Action::Recovery => 9,
        Action::Developer => 10,
    }
}

/// Append "<n>.<d> GB" (or MB below 1 GiB).
fn push_size(b: &mut StrBuf<24>, mib: u32) {
    if mib >= 1024 {
        let tenths = mib * 10 / 1024;
        b.push_u32(tenths / 10);
        b.push('.');
        b.push_u32(tenths % 10);
        b.push_str(" GB");
    } else {
        b.push_u32(mib);
        b.push_str(" MB");
    }
}

impl ListScreen {
    #[must_use]
    pub fn new(kind: ListKind) -> ListScreen {
        ListScreen {
            kind,
            list: ListView::new(),
            rows: Rows::default(),
            lang: Lang::ENGLISH,
            always: false,
            taps: 0,
        }
    }

    #[must_use]
    pub fn screen_id(&self) -> ScreenId {
        match self.kind {
            ListKind::Root => SETTINGS,
            ListKind::Group(_) => GROUP,
            ListKind::Choice(_) => CHOICE,
            ListKind::Info(_) => INFO,
            ListKind::HomeApps => HOME_APPS,
            ListKind::PowerMenu => POWER_MENU,
            ListKind::Quick => QUICK_PANEL,
            ListKind::Usb => USB_CONNECT,
            ListKind::MusicRoot => MUSIC,
            ListKind::Bookmarks => BOOKMARKS,
            ListKind::Properties(_) => PROPERTIES,
            ListKind::Developer => DEVELOPER,
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        match self.kind {
            ListKind::Root => "Settings",
            ListKind::Group(_) => "SettingsGroup",
            ListKind::Choice(_) => "Choice",
            ListKind::Info(_) => "Info",
            ListKind::HomeApps => "HomeApps",
            ListKind::PowerMenu => "PowerMenu",
            ListKind::Quick => "QuickPanel",
            ListKind::Usb => "UsbConnect",
            ListKind::MusicRoot => "Music",
            ListKind::Bookmarks => "Bookmarks",
            ListKind::Properties(_) => "Properties",
            ListKind::Developer => "Developer",
        }
    }

    fn title(&self) -> &'static str {
        let l = self.lang;
        match self.kind {
            ListKind::Root => l.get(Str::AppSettings),
            ListKind::Group(g) => GROUPS.get(usize::from(g)).map_or("", |g| l.get(g.label)),
            ListKind::Choice(sid) => settings::def(sid).map_or("", |d| l.get(d.label)),
            ListKind::Info(Info::Battery) => l.get(Str::Battery),
            ListKind::Info(Info::CardInfo) => l.get(Str::CardInfo),
            ListKind::Info(_) => l.get(Str::About),
            ListKind::HomeApps => l.get(Str::HomeScreen),
            ListKind::PowerMenu => l.get(Str::GPower),
            ListKind::Quick => l.get(Str::QuickSettings),
            ListKind::Usb => l.get(Str::ComputerConnected),
            ListKind::MusicRoot => l.get(Str::AppMusic),
            ListKind::Bookmarks => l.get(Str::Bookmarks),
            ListKind::Properties(_) => l.get(Str::Properties),
            ListKind::Developer => l.get(Str::Developer),
        }
    }

    /// Rebuild the rows from the services.
    fn refresh<S: Services + ?Sized>(&mut self, svc: &S) {
        self.lang = svc.lang();
        let l = self.lang;
        let mut rows = Rows::default();
        if self.kind != ListKind::PowerMenu
            && let Some(h) = rows.push(self.title(), 0)
        {
            h.header = true;
        }
        match self.kind {
            ListKind::Root => {
                for (i, g) in GROUPS.iter().enumerate() {
                    if let Some(r) = rows.push(l.get(g.label), u16::try_from(i).unwrap_or(0)) {
                        r.acc = Accessory::Chevron;
                    }
                }
            }
            ListKind::Group(g) => {
                let Some(grp) = GROUPS.get(usize::from(g)) else { return };
                for s in grp.items {
                    if settings::hidden(s.id) {
                        continue;
                    }
                    if s.kind == Kind::Action(Action::StartOriginal) && !svc.dual_boot() {
                        continue;
                    }
                    if s.kind == Kind::Action(Action::Developer) && !svc.developer() {
                        continue;
                    }
                    let v = svc.setting(s.id);
                    let Some(r) = rows.push(l.get(s.label), s.id.0) else { break };
                    match s.kind {
                        Kind::Toggle => r.acc = Accessory::Toggle(v != 0),
                        Kind::Choice(_) | Kind::Range { .. } => {
                            format_value(s, v, l, &mut r.value);
                            r.acc = Accessory::Chevron;
                        }
                        Kind::Language => {
                            r.value.push_str(svc.lang().name());
                            r.acc = Accessory::Chevron;
                        }
                        Kind::Info(Info::DeviceName) => {
                            let d = svc.bt_device();
                            r.value.push_str_or_ellipsis(if d.is_empty() { "—" } else { d });
                        }
                        Kind::Info(Info::RecFolder) => {
                            r.value.push_str("/Recordings");
                        }
                        Kind::Info(Info::Battery) => {
                            r.value.push_u32(u32::from(svc.battery().percent));
                            r.value.push_str(" %");
                            r.acc = Accessory::Chevron;
                        }
                        Kind::Action(
                            Action::FormatCard
                            | Action::FactoryReset
                            | Action::UpdateFirmware
                            | Action::CheckCard
                            | Action::RebuildLibrary
                            | Action::ClearCaches,
                        ) => {}
                        _ => r.acc = Accessory::Chevron,
                    }
                }
            }
            ListKind::Choice(sid) => {
                let Some(d) = settings::def(sid) else { return };
                let v = svc.setting(sid);
                match d.kind {
                    Kind::Choice(cs) => {
                        for (i, c) in cs.iter().enumerate() {
                            let mut text = StrBuf::<40>::new();
                            settings::format_choice(*c, l, &mut text);
                            // Choices render through the value buffer when
                            // they are not plain strings.
                            let label = match c {
                                settings::Choice::S(s) => l.get(*s),
                                settings::Choice::Lit(t) => t,
                                _ => "",
                            };
                            let Some(r) = rows.push(label, u16::try_from(i).unwrap_or(0)) else { break };
                            if label.is_empty() {
                                r.value = StrBuf::from(text.as_str());
                            }
                            r.acc = Accessory::Radio(usize::try_from(v).is_ok_and(|vv| vv == i));
                        }
                    }
                    Kind::Language => {
                        let cur = svc.lang().code();
                        for i in 0..svc.language_count().min(super::MAX_ROWS - 1) {
                            let lg = svc.language(i);
                            if let Some(r) = rows.push(lg.name(), u16::try_from(i).unwrap_or(0)) {
                                r.acc = Accessory::Radio(lg.code() == cur);
                            }
                        }
                    }
                    _ => {}
                }
            }
            ListKind::Info(info) => self.info_rows(svc, info, &mut rows),
            ListKind::HomeApps => {
                let hidden = svc.setting(id::HOME_HIDDEN);
                for (i, app) in App::ALL.iter().enumerate() {
                    if !svc.app_available(*app) {
                        continue;
                    }
                    let Some(r) = rows.push(l.get(app.label()), u16::try_from(i).unwrap_or(0)) else { break };
                    let fixed = *app == App::Settings;
                    r.acc = Accessory::Check(fixed || hidden & (1 << i) == 0);
                    r.dim = fixed;
                }
            }
            ListKind::PowerMenu => {
                rows.push(l.get(Str::PowerOff), T_POWER_OFF);
                if let Some(r) = rows.push(l.get(Str::SleepTimer), id::SLEEP_TIMER.0)
                    && let Some(d) = settings::def(id::SLEEP_TIMER)
                {
                    format_value(d, svc.setting(id::SLEEP_TIMER), l, &mut r.value);
                    r.acc = Accessory::Chevron;
                }
                rows.push(l.get(Str::LockKeysOnly), T_LOCK);
                rows.push(l.get(Str::Restart), T_RESTART);
                if svc.dual_boot() {
                    rows.push(l.get(Str::StartOriginal), T_ORIGINAL);
                }
                rows.push(l.get(Str::RecoveryMode), T_RECOVERY);
            }
            ListKind::Quick => {
                if let Some(p) = svc.playing()
                    && let Some(r) = rows.push("", T_NOW_PLAYING)
                {
                    // The title itself is the row ("Now Playing" is implied
                    // by the play icon).
                    r.value.push_str_or_ellipsis(p.title);
                    r.icon = Some(&orpheus_ui::icons::PLAY);
                    r.acc = Accessory::Chevron;
                }
                rows.push(l.get(Str::Lock), T_LOCK);
                for sid in [
                    id::BLUETOOTH,
                    id::OUTPUT,
                    id::SLEEP_TIMER,
                    id::BRIGHTNESS,
                    id::SHUFFLE,
                    id::REPEAT,
                    id::EQUALISER,
                ] {
                    let Some(d) = settings::def(sid) else { continue };
                    let v = svc.setting(sid);
                    let label = if sid == id::EQUALISER { l.get(Str::Equaliser) } else { l.get(d.label) };
                    let Some(r) = rows.push(label, sid.0) else { break };
                    match d.kind {
                        Kind::Toggle => r.acc = Accessory::Toggle(v != 0),
                        _ => format_value(d, v, l, &mut r.value),
                    }
                }
            }
            ListKind::Usb => {
                let mut modes: [(Str, u16); 5] = [
                    (Str::ChargeOnly, 0),
                    (Str::FileTransfer, 1),
                    (Str::DiskImage, 2),
                    (Str::Keyboard, 3),
                    (Str::UsbAudio, 4),
                ];
                if !svc.usb_audio_built() {
                    modes[4].1 = u16::MAX;
                }
                let ro = crate::services::usb_card_read_only(svc);
                for (s, tag) in modes {
                    if tag != u16::MAX
                        && let Some(r) = rows.push(l.get(s), tag)
                        && tag == 1
                        && ro
                    {
                        // Settings › USB › Read-only card (VER-13): the
                        // computer gets the card write-protected (a lock
                        // before the row).
                        r.icon = Some(&orpheus_ui::icons::LOCK);
                    }
                }
                if let Some(r) = rows.push(l.get(Str::AlwaysDoThis), T_ALWAYS) {
                    r.acc = Accessory::Toggle(self.always);
                }
            }
            ListKind::MusicRoot => music_rows(svc, l, &mut rows),
            ListKind::Bookmarks => {
                let n = svc.bookmark_count();
                if n == 0
                    && let Some(r) = rows.push(l.get(Str::NoBookmarks), u16::MAX)
                {
                    r.dim = true;
                }
                for i in 0..n.min(u32::try_from(super::MAX_ROWS - 1).unwrap_or(0)) {
                    let Some(b) = svc.bookmark(i) else { break };
                    let Some(r) = rows.push("", u16::try_from(i).unwrap_or(0)) else { break };
                    r.value.push_duration(b.position_ms / 1000);
                    r.value.push_str("  ");
                    r.value.push_str_or_ellipsis(b.title);
                    r.icon = Some(&orpheus_ui::icons::NOTE);
                }
            }
            ListKind::Properties(row) => properties_rows(svc, l, row, &mut rows),
            ListKind::Developer => developer_rows(svc, l, &mut rows),
        }
        self.rows = rows;
        self.list.invalidate();
    }

    fn info_rows<S: Services + ?Sized>(&self, svc: &S, info: Info, rows: &mut Rows) {
        let l = self.lang;
        let mut add = |label: Str, f: &dyn Fn(&mut StrBuf<24>)| {
            if let Some(r) = rows.push(l.get(label), 0) {
                f(&mut r.value);
                r.wide_value = true;
            }
        };
        match info {
            Info::Battery => {
                let b = svc.battery();
                add(Str::Level, &|v| {
                    v.push_u32(u32::from(b.percent));
                    v.push_str(" %");
                });
                add(Str::Voltage, &|v| {
                    let mv = u32::from(b.millivolts);
                    v.push_u32(mv / 1000);
                    v.push('.');
                    v.push_u32_padded((mv % 1000) / 10, 2);
                    v.push_str(" V");
                });
                let st = match b.charge {
                    ChargeState::Charging | ChargeState::Trickle | ChargeState::Detecting => Str::Charging,
                    ChargeState::Full => Str::Charged,
                    ChargeState::NoBattery => Str::NoBattery,
                    ChargeState::Fault => Str::ChargerFault,
                    ChargeState::NotPresent => Str::NotCharging,
                };
                add(Str::State, &|v| {
                    v.push_str(l.get(st));
                });
            }
            Info::CardInfo => match svc.card_info() {
                Some(ci) => {
                    add(Str::Capacity, &|v| push_size(v, ci.capacity_mib));
                    add(Str::Free, &|v| push_size(v, ci.free_mib));
                    add(Str::FileSystem, &|v| {
                        v.push_str(match ci.fs {
                            FsKind::Fat32 => "FAT32",
                            FsKind::ExFat => "exFAT",
                            FsKind::Other => "?",
                        });
                    });
                }
                None => add(Str::NoCard, &|_| {}),
            },
            _ => {
                add(Str::Version, &|v| {
                    v.push_str_or_ellipsis(svc.version());
                });
                add(Str::Build, &|v| {
                    v.push_str_or_ellipsis(svc.build_id());
                });
                // T-6: the slot stage 2 started, and the ones it skipped
                // because they kept failing to start.
                if let Some(b) = svc.boot_slot() {
                    add(Str::FwSlot, &|v| {
                        v.push_str(BootSlot::name(b.running));
                    });
                    if b.failed != 0 {
                        add(Str::SlotFailed, &|v| {
                            let mut first = true;
                            for i in 0..3u8 {
                                if b.failed & 1 << i != 0 {
                                    if !first {
                                        v.push_str(", ");
                                    }
                                    v.push_str(BootSlot::name(i));
                                    first = false;
                                }
                            }
                        });
                    }
                }
                add(Str::Serial, &|v| {
                    v.push_str_or_ellipsis(svc.serial());
                });
                add(Str::Hardware, &|v| {
                    v.push_str_or_ellipsis(svc.hardware());
                });
                add(Str::Licences, &|v| {
                    v.push_str("MIT/Apache-2.0");
                });
            }
        }
    }

    fn focused_tag(&self) -> Option<u16> {
        self.rows.get(self.list.focus).filter(|r| !r.header).map(|r| r.tag)
    }

    fn after_change<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, sid: SettingId) {
        if [id::THEME, id::ACCENT, id::LANGUAGE].contains(&sid) {
            cx.ui.invalidate_all();
        }
        self.refresh(cx.svc);
    }

    fn set<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, sid: SettingId, v: i16) {
        cx.svc.set_setting(sid, v);
        self.after_change(cx, sid);
    }

    fn step_in_row<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, dir: i16) -> bool {
        let Some(tag) = self.focused_tag() else { return false };
        let sid = SettingId(tag);
        let Some(d) = settings::def(sid) else { return false };
        if !matches!(d.kind, Kind::Toggle | Kind::Choice(_) | Kind::Range { .. }) {
            return false;
        }
        let v = d.step(cx.svc.setting(sid), dir);
        self.set(cx, sid, v);
        true
    }

    fn confirm(lang: Lang, a: Action) -> Option<DialogSpec> {
        let id = DLG + action_code(a);
        let (title, text, act, danger) = match a {
            Action::FactoryReset => (Str::FactoryResetQ, Str::FactoryResetText, Str::Reset, true),
            Action::FormatCard => (Str::FormatQ, Str::FormatText, Str::Format, true),
            Action::RebuildLibrary => (Str::RebuildQ, Str::RebuildText, Str::Rebuild, false),
            Action::ClearCaches => (Str::ClearQ, Str::ClearText, Str::Clear, false),
            Action::StartOriginal => (Str::StartOriginalQ, Str::StartOriginalText, Str::Start, false),
            Action::Recovery => (Str::RecoveryQ, Str::RecoveryText, Str::Restart, false),
            _ => return None,
        };
        let (title, text, cancel, act) =
            (lang.get(title), lang.get(text), lang.get(Str::Cancel), lang.get(act));
        Some(if danger {
            DialogSpec::destructive(id, title, text, cancel, act)
        } else {
            DialogSpec::confirm(id, title, text, cancel, act)
        })
    }

    fn activate<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) -> Nav {
        let Some(tag) = self.focused_tag() else { return Nav::Stay };
        let lang = self.lang;
        match self.kind {
            ListKind::Root => Nav::Push(GROUP, Arg(u32::from(tag))),
            ListKind::Group(_) => {
                let sid = SettingId(tag);
                let Some(d) = settings::def(sid) else { return Nav::Stay };
                match d.kind {
                    Kind::Toggle => {
                        self.step_in_row(cx, 1);
                        Nav::Stay
                    }
                    Kind::Choice(_) | Kind::Language => Nav::Push(CHOICE, Arg(u32::from(tag))),
                    Kind::Range { .. } => Nav::Push(SLIDER, Arg(u32::from(tag))),
                    Kind::HomeApps => Nav::Push(HOME_APPS, Arg(0)),
                    Kind::Info(Info::DeviceName | Info::RecFolder) => Nav::Stay,
                    Kind::Info(Info::Diagnostics) => Nav::Push(NOTICE, Arg(Notice::NotYetSettings.arg())),
                    Kind::Info(i) => Nav::Push(INFO, Arg(info_arg(i))),
                    Kind::Action(Action::SetTime) => Nav::Push(PICKER, Arg(0)),
                    Kind::Action(Action::SetDate) => Nav::Push(PICKER, Arg(1)),
                    Kind::Action(Action::Developer) => Nav::Push(DEVELOPER, Arg(0)),
                    Kind::Action(Action::CheckCard) => {
                        cx.svc.storage(StorageRequest::CheckCard);
                        cx.ui.toast(lang.get(Str::Started));
                        Nav::Stay
                    }
                    Kind::Action(Action::UpdateFirmware) => Nav::Push(super::UPDATE, Arg(0)),
                    Kind::Action(a) => {
                        if let Some(dl) = Self::confirm(lang, a) {
                            cx.ui.dialog(dl);
                        }
                        Nav::Stay
                    }
                }
            }
            ListKind::Choice(sid) => {
                let v = if sid == id::LANGUAGE {
                    settings::lang_code(cx.svc.language(usize::from(tag)).code())
                } else {
                    i16::try_from(tag).unwrap_or(0)
                };
                self.set(cx, sid, v);
                Nav::Pop
            }
            ListKind::Info(Info::About) => {
                self.about_ok(cx);
                Nav::Stay
            }
            ListKind::Info(_) => Nav::Stay,
            ListKind::Developer => {
                self.developer_ok(cx, tag);
                Nav::Stay
            }
            ListKind::HomeApps => {
                if App::from_index(u8::try_from(tag).unwrap_or(0)) != Some(App::Settings) {
                    let hidden = cx.svc.setting(id::HOME_HIDDEN) ^ (1 << tag);
                    self.set(cx, id::HOME_HIDDEN, hidden);
                }
                Nav::Stay
            }
            ListKind::PowerMenu => match tag {
                T_POWER_OFF => Nav::Replace(GOODBYE, Arg(0)),
                T_LOCK => {
                    cx.svc.power(PowerRequest::LockKeys);
                    Nav::Pop
                }
                T_RESTART => {
                    cx.svc.power(PowerRequest::Restart);
                    Nav::Stay
                }
                T_ORIGINAL | T_RECOVERY => {
                    let a = if tag == T_ORIGINAL { Action::StartOriginal } else { Action::Recovery };
                    if let Some(dl) = Self::confirm(lang, a) {
                        cx.ui.dialog(dl);
                    }
                    Nav::Stay
                }
                _ => Nav::Push(CHOICE, Arg(u32::from(tag))),
            },
            ListKind::Quick => match tag {
                T_NOW_PLAYING => Nav::Push(NOTICE, Arg(Notice::NotYet(App::Music).arg())),
                T_LOCK => {
                    cx.svc.power(PowerRequest::LockKeys);
                    Nav::Pop
                }
                _ => {
                    self.step_in_row(cx, 1);
                    Nav::Stay
                }
            },
            ListKind::MusicRoot => match tag {
                T_NP => Nav::Push(NOW_PLAYING, Arg(0)),
                T_RESUME => {
                    cx.svc.command(PlayerCmd::Resume);
                    Nav::Push(NOW_PLAYING, Arg(0))
                }
                T_SHUFFLE_ALL => {
                    cx.svc.command(PlayerCmd::ShuffleAll);
                    Nav::Push(NOW_PLAYING, Arg(0))
                }
                T_FOLDERS => Nav::Push(MUSIC_FOLDERS, Arg(0)),
                T_BOOKMARKS => Nav::Push(BOOKMARKS, Arg(0)),
                T_BUILDING => Nav::Stay,
                t if cx.svc.library() == LibraryState::Ready && (T_LIBRARY..T_LIBRARY + 6).contains(&t) => {
                    let list = match t - T_LIBRARY {
                        0 => LibList::Artists,
                        1 => LibList::Albums,
                        2 => LibList::Songs,
                        3 => LibList::Genres,
                        5 => LibList::Recent,
                        // Playlists (later package).
                        _ => return Nav::Push(NOTICE, Arg(Notice::NotYet(App::Music).arg())),
                    };
                    Nav::Push(LIBRARY, LibraryScreen::arg(list))
                }
                T_LIBRARY..T_FOLDERS if matches!(cx.svc.library(), LibraryState::Building { .. }) => {
                    cx.ui.toast(lang.get(Str::BuildingLibrary));
                    Nav::Stay
                }
                _ => Nav::Push(NOTICE, Arg(Notice::NotYet(App::Music).arg())),
            },
            ListKind::Bookmarks => {
                if tag == u16::MAX {
                    return Nav::Stay;
                }
                cx.svc.command(PlayerCmd::PlayBookmark(u32::from(tag)));
                Nav::Push(NOW_PLAYING, Arg(0))
            }
            ListKind::Properties(_) => Nav::Stay,
            ListKind::Usb => {
                if tag == T_ALWAYS {
                    self.always = !self.always;
                    self.refresh(cx.svc);
                    return Nav::Stay;
                }
                if tag == 2 {
                    // Present disk image…: the USB Drive app chooses it.
                    if self.always {
                        cx.svc.set_setting(id::USB_WHEN_CONNECTED, 4);
                    }
                    return Nav::Replace(super::USB_DRIVE, Arg(0));
                }
                let mode = match tag {
                    1 => UsbMode::FileTransfer,
                    3 => UsbMode::Keyboard,
                    4 => UsbMode::Audio,
                    _ => UsbMode::ChargeOnly,
                };
                cx.svc.select_usb(mode);
                if self.always {
                    let v = match mode {
                        UsbMode::ChargeOnly => Some(1),
                        UsbMode::FileTransfer => Some(2),
                        UsbMode::Keyboard => Some(3),
                        UsbMode::DiskImage => Some(4),
                        UsbMode::Audio => None,
                    };
                    if let Some(v) = v {
                        cx.svc.set_setting(id::USB_WHEN_CONNECTED, v);
                    }
                }
                Nav::Pop
            }
        }
    }

    /// OK in About: five presses show the developer rows (until restart).
    /// Not inlined: its frame stays out of the screen dispatch's.
    #[inline(never)]
    fn about_ok<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        self.taps = self.taps.saturating_add(1);
        if self.taps >= DEV_TAPS {
            cx.svc.show_developer();
            cx.ui.toast(self.lang.get(Str::DeveloperOn));
        }
    }

    /// OK on a Developer row (PWR-5): the battery logger on/off, Keep awake.
    #[inline(never)]
    fn developer_ok<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, tag: u16) {
        let d = cx.svc.drain_log();
        match tag {
            T_DRAIN => cx.svc.set_drain_log(!d.on, d.stay),
            T_STAY => cx.svc.set_drain_log(d.on, !d.stay),
            _ => return,
        }
        self.refresh(cx.svc);
    }

    fn dialog_done<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, dlg: u16, button: u8) {
        if button != 1 {
            return;
        }
        let lang = self.lang;
        match dlg.wrapping_sub(DLG) {
            2 => {
                cx.svc.storage(StorageRequest::RebuildLibrary);
                cx.ui.toast(lang.get(Str::Started));
            }
            3 => {
                cx.svc.storage(StorageRequest::ClearCaches);
                cx.ui.toast(lang.get(Str::Done));
            }
            5 => {
                cx.svc.storage(StorageRequest::Format);
                cx.ui.toast(lang.get(Str::Started));
            }
            7 => cx.svc.power(PowerRequest::StartOriginal),
            8 => {
                cx.svc.factory_reset();
                cx.ui.invalidate_all();
                self.refresh(cx.svc);
                cx.ui.toast(self.lang.get(Str::Done));
            }
            9 => cx.svc.power(PowerRequest::Recovery),
            _ => {}
        }
    }
}

impl<S: Services + ?Sized> Screen<S> for ListScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.kind = match self.kind {
            ListKind::Group(_) => ListKind::Group(u8::try_from(arg.0).unwrap_or(0)),
            ListKind::Choice(_) => ListKind::Choice(SettingId(u16::try_from(arg.0).unwrap_or(0))),
            ListKind::Info(_) => ListKind::Info(info_from(arg.0)),
            ListKind::Properties(_) => ListKind::Properties(arg.0),
            k => k,
        };
        self.refresh(cx.svc);
        let mut l = self.list;
        // Choice lists open on the current value.
        let start = match self.kind {
            ListKind::Choice(_) => (0..self.rows_len())
                .find(|&i| matches!(self.rows.get(i).map(|r| r.acc), Some(Accessory::Radio(true))))
                .unwrap_or(0),
            _ => 0,
        };
        l.set_focus(&self.rows, start, rows_visible(), cx.now);
        self.list = l;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => return Nav::Pop,
            Event::Key(Key::Prev | Key::Next, KeyPhase::Down | KeyPhase::Repeat)
                if self.kind == ListKind::Quick =>
            {
                let dir = if matches!(*ev, Event::Key(Key::Next, _)) { 1 } else { -1 };
                self.step_in_row(cx, dir);
                return Nav::Stay;
            }
            Event::Key(Key::Prev | Key::Next, _) if self.kind == ListKind::Quick => return Nav::Stay,
            Event::Ui(UiEvent::Dialog { id, button }) => {
                self.dialog_done(cx, id, button);
                return Nav::Stay;
            }
            Event::Service(_) => {
                let focus = self.list.focus;
                self.refresh(cx.svc);
                let mut l = self.list;
                l.set_focus(&self.rows, focus, rows_visible(), cx.now);
                self.list = l;
                return Nav::Stay;
            }
            _ => {}
        }
        let mut l = self.list;
        let act = l.handle(&self.rows, ev, rows_visible(), cx.list());
        self.list = l;
        match act {
            ListAction::Activated(_) => self.activate(cx),
            _ => Nav::Stay,
        }
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let mut l = self.list;
        l.draw(&self.rows, g, dirty, cx.now, Role::Bg);
        self.list = l;
    }

    fn wants(&self) -> Wants {
        let strip = match self.kind {
            ListKind::Quick => StripUse::Hint(self.lang.get(Str::HintQuick)),
            _ => StripUse::Auto,
        };
        Wants { ticks: self.list.wants_ticks(), title: Some(self.title()), strip, ..Wants::default() }
    }

    fn save(&self) -> Resume {
        Resume { focus: self.list.focus, top: self.list.top, ..Resume::default() }
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        let mut l = self.list;
        l.top = r.top;
        l.set_focus(&self.rows, r.focus, rows_visible(), cx.now);
        self.list = l;
    }

    #[cfg(feature = "screen-reader")]
    fn describe(&self, _cx: &Cx<'_, S>, d: &mut orpheus_ui::a11y::Desc) {
        d.title(self.title());
        d.list(&self.rows, &self.list, "empty");
        // The quick panel's rows change with Prev and Next.
        if self.kind == ListKind::Quick
            && let Some(f) = d.focus.as_mut()
            && !f.value.is_empty()
        {
            f.role = orpheus_ui::a11y::Role::Slider;
        }
    }
}

impl ListScreen {
    fn rows_len(&self) -> u32 {
        orpheus_ui::widgets::list::ListSource::len(&self.rows)
    }
}
