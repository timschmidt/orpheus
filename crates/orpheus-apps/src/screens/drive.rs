//! USB Drive (05 §7.10, WP USB-6): present a disk image from the card to
//! a computer as a CD-ROM or a disk, so that the computer can boot from it.
//!
//! ```text
//! USB Drive           Options               Status
//! │ Images        › │ │ rescue.iso       │ │      [drive]      │
//! │ Test images   › │ │ Size     1.2 GB  │ │    rescue.iso     │
//! │ Choose file…  › │ │ Mode     CD-ROM  │ │ CD-ROM · Read-only│
//!                     │ Read-only     ●  │ │ Connected to a co…│
//!                     │ Connect          │ │ Read 312 MB · 9 MB/s
//! ```
//!
//! The app lists `/Images` and, when it exists, `/.orpheus/images` (test
//! images) and lets the user choose any file on the card; a folder of
//! images is the file browser in [`super::BrowseMode::DiskImages`]. The
//! options default to CD-ROM for `.iso` and a removable disk otherwise;
//! a CD is always read-only, a disk is read-only unless it is a test image
//! and the player is in test mode (CLAUDE.md rule 5; the service checks
//! again). Connect hands the card to the computer; the status screen shows
//! the counters, and Back disconnects (asking first when the computer may
//! still be writing).

use orpheus_ui::event::{Event, Instant, Key, KeyPhase, ServiceEvent, UiEvent};
use orpheus_ui::font::FontId;
use orpheus_ui::gfx::{Align, Gfx};
use orpheus_ui::icons as ui_icons;
use orpheus_ui::menu::MenuBuilder;
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, ScreenId, StripUse, Wants};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;
use orpheus_ui::widgets::dialog::DialogSpec;
use orpheus_ui::widgets::list::{Accessory, ListAction, ListView};

use super::files::push_size;
use super::rows::Rows;
use super::{DRIVE_FILES, DRIVE_OPTIONS, DRIVE_STATUS, USB_DRIVE};
use crate::browse::{DirId, EntryKind, FileRef};
use crate::icons;
use crate::services::{
    DriveInfo, DriveMedium, DrivePhase, DriveRefusal, DriveRequest, Services, drive_event, other,
};
use crate::strings::{Lang, Str};

/// Which screen of the app this is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DriveView {
    /// The app's first screen: the image folders.
    Home,
    /// Mode, read-only, Connect for one image. Arg: its row in the
    /// current listing.
    Options,
    /// The image presented: counters; Back disconnects.
    Status,
}

// Row tags.
const T_CURRENT: u16 = 1;
const T_IMAGES: u16 = 2;
const T_TEST: u16 = 3;
const T_CHOOSE: u16 = 4;
const T_INFO: u16 = 5;
const T_NAME: u16 = 10;
const T_SIZE: u16 = 11;
const T_MODE: u16 = 12;
const T_RO: u16 = 13;
const T_CONNECT: u16 = 14;

const DLG_STOP: u16 = 0x600;

/// How often the rate is measured (ms).
const RATE_MS: u64 = 1000;
/// Shortest time between redraws for the counters alone (ms).
const REDRAW_MS: u64 = 250;

const fn visible() -> u32 {
    5
}

/// The medium's name.
#[must_use]
pub const fn medium_label(m: DriveMedium) -> Str {
    match m {
        DriveMedium::Cd => Str::DriveCd,
        DriveMedium::RemovableDisk => Str::DriveRemovable,
        DriveMedium::FixedDisk => Str::DriveFixed,
    }
}

/// What to tell the user about a refusal.
#[must_use]
pub const fn refusal_text(r: DriveRefusal) -> Str {
    match r {
        DriveRefusal::TooFragmented => Str::DriveFragmented,
        DriveRefusal::TooSmall => Str::DriveTooSmall,
        DriveRefusal::TooLarge => Str::DriveTooLarge,
        DriveRefusal::NotFullyWritten => Str::DriveIncomplete,
        DriveRefusal::Io => Str::DriveCardError,
        DriveRefusal::NotFound => Str::DriveNotFound,
        DriveRefusal::CdReadOnly | DriveRefusal::WriteOutsideOrpheus | DriveRefusal::WriteNeedsTestMode => {
            Str::DriveNotAllowed
        }
    }
}

/// The next medium when Mode is pressed.
const fn next_medium(m: DriveMedium) -> DriveMedium {
    match m {
        DriveMedium::Cd => DriveMedium::RemovableDisk,
        DriveMedium::RemovableDisk => DriveMedium::FixedDisk,
        DriveMedium::FixedDisk => DriveMedium::Cd,
    }
}

/// What the status screen shows (redrawn when it changes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Shown {
    info: DriveInfo,
    rate: u64,
    lang: &'static str,
}

pub struct UsbDriveScreen {
    view: DriveView,
    list: ListView,
    rows: Rows,
    lang: Lang,
    /// Options: the image chosen.
    name: StrBuf<64>,
    file: Option<FileRef>,
    size: u64,
    medium: DriveMedium,
    write: bool,
    /// The image is in the test folder.
    test_image: bool,
    /// Status: what is drawn; the rate's last sample.
    shown: Option<Shown>,
    sample: (Instant, u64),
    rate: u64,
    /// The export was refused: Back only goes back.
    failed: bool,
    drawn_at: Instant,
}

impl UsbDriveScreen {
    #[must_use]
    pub fn new(view: DriveView) -> UsbDriveScreen {
        UsbDriveScreen {
            view,
            list: ListView::new(),
            rows: Rows::default(),
            lang: Lang::ENGLISH,
            name: StrBuf::new(),
            file: None,
            size: 0,
            medium: DriveMedium::RemovableDisk,
            write: false,
            test_image: false,
            shown: None,
            sample: (Instant::default(), 0),
            rate: 0,
            failed: false,
            drawn_at: Instant::default(),
        }
    }

    #[must_use]
    pub fn screen_id(&self) -> ScreenId {
        match self.view {
            DriveView::Home => USB_DRIVE,
            DriveView::Options => DRIVE_OPTIONS,
            DriveView::Status => DRIVE_STATUS,
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        match self.view {
            DriveView::Home => "UsbDrive",
            DriveView::Options => "DriveOptions",
            DriveView::Status => "DriveStatus",
        }
    }

    /// Writable is allowed for this image (the service decides again).
    fn may_write<S: Services + ?Sized>(&self, svc: &S) -> bool {
        !self.medium.is_cd() && self.test_image && svc.drive_test_mode()
    }

    fn refresh<S: Services + ?Sized>(&mut self, svc: &mut S) {
        self.lang = svc.lang();
        let l = self.lang;
        let mut rows = Rows::default();
        if let Some(h) = rows.push(l.get(Str::AppUsbDrive), 0) {
            h.header = true;
        }
        match self.view {
            DriveView::Home => {
                let info = svc.drive();
                if info.phase.is_active()
                    && let Some(r) = rows.push(l.get(Str::DrivePresenting), T_CURRENT)
                {
                    r.value.push_str_or_ellipsis(svc.drive_name());
                    r.icon = Some(&ui_icons::DISC);
                    r.acc = Accessory::Chevron;
                }
                let (images, test) = svc.drive_dirs();
                if let Some(r) = rows.push(l.get(Str::DriveImages), T_IMAGES) {
                    r.icon = Some(&ui_icons::FOLDER);
                    r.acc = Accessory::Chevron;
                    r.dim = images.is_none();
                }
                if test.is_some()
                    && let Some(r) = rows.push(l.get(Str::DriveTestImages), T_TEST)
                {
                    r.icon = Some(&ui_icons::FOLDER);
                    r.acc = Accessory::Chevron;
                }
                if let Some(r) = rows.push(l.get(Str::DriveChoose), T_CHOOSE) {
                    r.icon = Some(&ui_icons::FILE);
                    r.acc = Accessory::Chevron;
                }
                if images.is_none()
                    && let Some(r) = rows.push(l.get(Str::DriveNoFolder), T_INFO)
                {
                    r.dim = true;
                }
            }
            DriveView::Options => {
                if self.file.is_some() {
                    if let Some(r) = rows.push("", T_NAME) {
                        r.value.push_str_or_ellipsis(self.name.as_str());
                        r.icon = Some(&ui_icons::DISC);
                    }
                    if let Some(r) = rows.push(l.get(Str::Size), T_SIZE) {
                        push_size(&mut r.value, self.size);
                        r.dim = true;
                    }
                    if let Some(r) = rows.push(l.get(Str::DriveMode), T_MODE) {
                        r.value.push_str_or_ellipsis(l.get(medium_label(self.medium)));
                        r.wide_value = true;
                    }
                    let may = self.may_write(svc);
                    if let Some(r) = rows.push(l.get(Str::DriveReadOnly), T_RO) {
                        r.acc = Accessory::Toggle(!(self.write && may));
                        r.dim = !may;
                    }
                    if let Some(r) = rows.push(l.get(Str::DriveConnect), T_CONNECT) {
                        r.icon = Some(&ui_icons::COMPUTER);
                    }
                } else if let Some(r) = rows.push(l.get(Str::DriveNotFound), T_INFO) {
                    r.dim = true;
                }
            }
            DriveView::Status => {}
        }
        self.rows = rows;
        self.list.invalidate();
    }

    fn reload<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        let focus = self.list.focus;
        self.refresh(cx.svc);
        let mut l = self.list;
        l.set_focus(&self.rows, focus, visible(), cx.now);
        self.list = l;
    }

    fn focused_tag(&self) -> Option<u16> {
        self.rows.get(self.list.focus).filter(|r| !r.header).map(|r| r.tag)
    }

    fn activate<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) -> Nav {
        let Some(tag) = self.focused_tag() else { return Nav::Stay };
        let l = self.lang;
        match tag {
            T_CURRENT => Nav::Push(DRIVE_STATUS, Arg(0)),
            T_IMAGES | T_TEST => {
                let (images, test) = cx.svc.drive_dirs();
                match if tag == T_IMAGES { images } else { test } {
                    Some(d) => Nav::Push(DRIVE_FILES, Arg(d.0)),
                    None => {
                        cx.ui.toast(l.get(Str::DriveNoFolder));
                        Nav::Stay
                    }
                }
            }
            T_CHOOSE => Nav::Push(DRIVE_FILES, Arg(DirId::ROOT.0)),
            T_MODE => {
                self.medium = next_medium(self.medium);
                if self.medium.is_cd() {
                    self.write = false;
                }
                self.reload(cx);
                Nav::Stay
            }
            T_RO => {
                if self.may_write(cx.svc) {
                    self.write = !self.write;
                    self.reload(cx);
                } else {
                    cx.ui.toast(l.get(Str::DriveForcedRo));
                }
                Nav::Stay
            }
            T_CONNECT => {
                let Some(file) = self.file else { return Nav::Stay };
                let write = self.write && self.may_write(cx.svc);
                cx.svc.drive_start(DriveRequest { file, medium: self.medium, write }, self.name.as_str());
                Nav::Replace(DRIVE_STATUS, Arg(0))
            }
            _ => Nav::Stay,
        }
    }

    /// Take the image of row `row` of the current listing.
    fn choose<S: Services + ?Sized>(&mut self, svc: &mut S, row: u32) {
        self.file = None;
        let test = svc.drive_dirs().1;
        let Some(e) = svc.entry(row) else { return };
        if e.kind != EntryKind::DiskImage {
            return;
        }
        self.name.clear();
        self.name.push_str_or_ellipsis(e.name);
        self.file = Some(e.file);
        self.size = e.size;
        self.medium = DriveMedium::for_name(e.name);
        self.write = false;
        self.test_image = test.is_some_and(|d| d.0 == e.file.dir);
    }

    fn update_rate(&mut self, now: Instant, read: u64) {
        let (t0, b0) = self.sample;
        let dt = now.since(t0);
        if dt >= RATE_MS {
            self.rate = read.saturating_sub(b0) * 1000 / dt.max(1);
            self.sample = (now, read);
        }
    }

    fn stop<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) -> Nav {
        cx.svc.drive_stop();
        Nav::Pop
    }

    fn draw_status<S: Services + ?Sized>(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let info = cx.svc.drive();
        let shown = Shown { info, rate: self.rate, lang: cx.svc.lang().code() };
        if dirty == Dirty::Changed && self.shown == Some(shown) {
            return;
        }
        // While only the counters move, at most four redraws a second
        // (the UI shares the executor with the USB transfer).
        let counters_only = self
            .shown
            .is_some_and(|s| (s.info.phase, s.info.host, s.lang) == (info.phase, info.host, shown.lang));
        if dirty == Dirty::Changed && counters_only && cx.now.since(self.drawn_at) < REDRAW_MS {
            return;
        }
        self.drawn_at = cx.now;
        self.shown = Some(shown);
        self.failed = info.phase == DrivePhase::Failed;
        let l = cx.svc.lang();
        self.lang = l;
        let (w, h) = g.size();
        g.clear(Role::Bg);
        let failed = info.phase == DrivePhase::Failed;
        let ic = g.c(if failed { Role::Warning } else { Role::Accent });
        g.icon2((w - 24) / 2, 0, &icons::USB_DRIVE, ic);
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        g.text_in(4, 30, w - 8, cx.svc.drive_name(), FontId::UiBold, text, Align::Center);
        let mut b = StrBuf::<64>::new();
        if failed {
            let warn = g.c(Role::Warning);
            g.text_in(4, 46, w - 8, l.get(Str::DriveFailed), FontId::UiBold, warn, Align::Center);
            let mut y = 62;
            let why = l.get(info.refusal.map_or(Str::DriveCardError, refusal_text));
            let pitch = g.line_pitch(why, FontId::Ui);
            g.wrap(why, FontId::Ui, w - 8, |g, line| {
                if y <= h - 4 {
                    g.text_in(4, y, w - 8, line, FontId::Ui, dim, Align::Center);
                }
                y += pitch;
            });
            return;
        }
        b.push_str(l.get(medium_label(info.medium)));
        b.push_str(" · ");
        b.push_str(l.get(if info.read_only { Str::DriveReadOnly } else { Str::DriveReadWrite }));
        g.text_in(4, 45, w - 8, b.as_str(), FontId::Ui, dim, Align::Center);
        let state = match info.phase {
            DrivePhase::Attached if info.host => Str::DriveInUse,
            DrivePhase::Attached => Str::DriveWaiting,
            _ => Str::DrivePreparing,
        };
        g.text_in(4, 62, w - 8, l.get(state), FontId::Ui, text, Align::Center);
        b.clear();
        b.push_str(l.get(Str::DriveRead));
        b.push(' ');
        push_size(&mut b, info.bytes_read);
        g.text_in(4, 78, w - 8, b.as_str(), FontId::Ui, dim, Align::Center);
        // The last line: what was written (a writable image), else the
        // rate while the computer reads.
        b.clear();
        if !info.read_only {
            b.push_str(l.get(Str::DriveWritten));
            b.push(' ');
            push_size(&mut b, info.bytes_written);
        } else if self.rate > 0 {
            push_size(&mut b, self.rate);
            b.push_str("/s");
        }
        g.text_in(4, 93, w - 8, b.as_str(), FontId::Ui, dim, Align::Center);
    }
}

impl<S: Services + ?Sized> Screen<S> for UsbDriveScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.lang = cx.svc.lang();
        match self.view {
            DriveView::Options => self.choose(cx.svc, arg.0),
            DriveView::Status => {
                self.shown = None;
                self.sample = (cx.now, cx.svc.drive().bytes_read);
                self.rate = 0;
            }
            DriveView::Home => {}
        }
        self.refresh(cx.svc);
        // Options open on Connect.
        let start = if self.view == DriveView::Options { self.rows.find(T_CONNECT).unwrap_or(1) } else { 1 };
        let mut l = self.list;
        l.set_focus(&self.rows, start, visible(), cx.now);
        self.list = l;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        if self.view == DriveView::Status {
            return match *ev {
                Event::Key(Key::Back, KeyPhase::Up { held: false }) => {
                    let info = cx.svc.drive();
                    if info.phase.is_active() && !info.read_only && info.bytes_written > 0 {
                        let l = self.lang;
                        cx.ui.dialog(DialogSpec::confirm(
                            DLG_STOP,
                            l.get(Str::DriveStopQ),
                            l.get(Str::DriveStopText),
                            l.get(Str::Cancel),
                            l.get(Str::DriveStop),
                        ));
                        Nav::Stay
                    } else {
                        self.stop(cx)
                    }
                }
                Event::Ui(UiEvent::Dialog { id: DLG_STOP, button }) => {
                    if button == 1 {
                        self.stop(cx)
                    } else {
                        Nav::Stay
                    }
                }
                Event::Service(ServiceEvent::Other { kind: other::USB_DRIVE, value: drive_event::STATE })
                    if cx.svc.drive().phase == DrivePhase::Off =>
                {
                    // Ended elsewhere (ejected on the computer, a command).
                    Nav::Pop
                }
                Event::Service(ServiceEvent::Other { kind: other::USB_DRIVE, value: drive_event::STATE })
                | Event::Tick(_) => {
                    let read = cx.svc.drive().bytes_read;
                    self.update_rate(cx.now, read);
                    cx.ui.redraw();
                    Nav::Stay
                }
                _ => Nav::Stay,
            };
        }
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => return Nav::Pop,
            Event::Service(_) => {
                self.reload(cx);
                return Nav::Stay;
            }
            _ => {}
        }
        let mut l = self.list;
        let act = l.handle(&self.rows, ev, visible(), cx.list());
        self.list = l;
        match act {
            ListAction::Activated(_) => self.activate(cx),
            _ => Nav::Stay,
        }
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        if self.view == DriveView::Status {
            self.draw_status(cx, g, dirty);
            return;
        }
        let mut l = self.list;
        l.draw(&self.rows, g, dirty, cx.now, Role::Bg);
        self.list = l;
    }

    fn menu(&self, _cx: &Cx<'_, S>, _out: &mut MenuBuilder) {}

    fn wants(&self) -> Wants {
        let title = Some(self.lang.get(Str::AppUsbDrive));
        match self.view {
            DriveView::Status => Wants {
                ticks: true,
                title,
                strip: if self.failed {
                    StripUse::Auto
                } else {
                    StripUse::Hint(self.lang.get(Str::HintDriveStatus))
                },
                ..Wants::default()
            },
            _ => Wants { ticks: self.list.wants_ticks(), title, strip: StripUse::Auto, ..Wants::default() },
        }
    }

    fn save(&self) -> Resume {
        let mut r = Resume { focus: self.list.focus, top: self.list.top, ..Resume::default() };
        r.data[0] = match self.medium {
            DriveMedium::Cd => 0,
            DriveMedium::RemovableDisk => 1,
            DriveMedium::FixedDisk => 2,
        };
        r.data[1] = u8::from(self.write);
        r
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        if self.view == DriveView::Options && self.file.is_some() {
            self.medium = match r.data[0] {
                0 => DriveMedium::Cd,
                2 => DriveMedium::FixedDisk,
                _ => DriveMedium::RemovableDisk,
            };
            self.write = r.data[1] != 0 && self.may_write(cx.svc);
            self.refresh(cx.svc);
        }
        let mut l = self.list;
        l.top = r.top;
        l.set_focus(&self.rows, r.focus, visible(), cx.now);
        self.list = l;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_cycle_and_texts() {
        let mut m = DriveMedium::Cd;
        for _ in 0..3 {
            m = next_medium(m);
        }
        assert_eq!(m, DriveMedium::Cd);
        assert_eq!(refusal_text(DriveRefusal::TooFragmented), Str::DriveFragmented);
        assert_eq!(medium_label(DriveMedium::FixedDisk), Str::DriveFixed);
    }
}
