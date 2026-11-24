//! The Bluetooth app (05 §7.4, 06 §4.2): status and switches, Pair new
//! device (a live inquiry list), Paired devices, the connection progress
//! screen and its failure dialog.
//!
//! The module is an audio transmitter only. It has no command to rename
//! a device or the player, and none that reports the signal strength
//! (docs/hw/bluetooth.md §3.7), so neither is offered; the row icon comes
//! from the class of device of inquiry results.

use orpheus_ui::event::{Event, Instant, Key, KeyPhase, ServiceEvent, UiEvent};
use orpheus_ui::font::FontId;
use orpheus_ui::gfx::{Align, Gfx};
use orpheus_ui::icons::{self as ui_icons, Icon};
use orpheus_ui::menu::MenuBuilder;
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, ScreenId, StripUse, Wants};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;
use orpheus_ui::widgets::dialog::DialogSpec;
use orpheus_ui::widgets::list::{Accessory, ListAction, ListView};
use orpheus_ui::widgets::misc::draw_spinner;

use super::rows::Rows;
use super::{BLUETOOTH, BT_CONNECT, BT_PAIRED, BT_SCAN};
use crate::icons;
use crate::services::{BT_DEVICES, BtDevice, BtKind, BtList, BtPhase, BtRequest, Services, bt_event, other};
use crate::settings::id;
use crate::strings::{Lang, Str};

/// Which list of the app this is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BtView {
    /// The app's first screen: on/off, state, the two lists, reconnect.
    Home,
    /// Pair new device: the inquiry's results, live.
    Scan,
    /// Paired devices.
    Paired,
}

// Row tags. Device rows carry their list index.
const T_POWER: u16 = 100;
const T_STATUS: u16 = 101;
const T_SCAN: u16 = 102;
const T_PAIRED: u16 = 103;
const T_RECONNECT: u16 = 104;
const T_INFO: u16 = 105;
const T_AGAIN: u16 = 106;
const T_DEVICE: u16 = 107;

// Dialogs and menu items.
const DLG_DISCONNECT: u16 = 0x200;
const DLG_FORGET: u16 = 0x201;
const DLG_FAILED: u16 = 0x202;
const M_CONNECT: u16 = 1;
const M_DISCONNECT: u16 = 2;
const M_FORGET: u16 = 3;

/// The phase as a row label.
#[must_use]
pub fn phase_label(p: BtPhase) -> Str {
    match p {
        BtPhase::Off => Str::Off,
        BtPhase::Starting => Str::BtStarting,
        BtPhase::Ready => Str::BtNotConnected,
        BtPhase::Scanning => Str::BtSearching,
        BtPhase::Connecting => Str::BtConnecting,
        BtPhase::Connected => Str::BtConnected,
        BtPhase::Streaming => Str::BtStreaming,
        BtPhase::NotResponding => Str::BtNotResponding,
    }
}

/// Row icon of a device kind.
#[must_use]
pub fn bt_kind_icon(k: BtKind) -> &'static Icon {
    match k {
        BtKind::Headphones => &ui_icons::HEADPHONES,
        BtKind::Speaker => &ui_icons::SPEAKER,
        BtKind::Phone => &ui_icons::PHONE,
        BtKind::Computer => &ui_icons::COMPUTER,
        BtKind::Unknown | BtKind::Other => &ui_icons::BT_DEVICE,
    }
}

/// `Arg` of [`BT_CONNECT`]: a device of a list.
#[must_use]
pub const fn connect_arg(list: BtList, index: usize) -> Arg {
    let l = match list {
        BtList::Found => 0,
        BtList::Paired => 0x100,
    };
    Arg(l | (index as u32 & 0xFF))
}

const fn connect_target(a: Arg) -> (BtList, usize) {
    let list = if a.0 & 0x100 != 0 { BtList::Paired } else { BtList::Found };
    (list, (a.0 & 0xFF) as usize)
}

const fn visible() -> u32 {
    5
}

pub struct BluetoothScreen {
    view: BtView,
    list: ListView,
    rows: Rows,
    lang: Lang,
}

impl BluetoothScreen {
    #[must_use]
    pub fn new(view: BtView) -> BluetoothScreen {
        BluetoothScreen { view, list: ListView::new(), rows: Rows::default(), lang: Lang::ENGLISH }
    }

    #[must_use]
    pub fn screen_id(&self) -> ScreenId {
        match self.view {
            BtView::Home => BLUETOOTH,
            BtView::Scan => BT_SCAN,
            BtView::Paired => BT_PAIRED,
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        match self.view {
            BtView::Home => "Bluetooth",
            BtView::Scan => "BtScan",
            BtView::Paired => "BtPaired",
        }
    }

    fn title(&self) -> &'static str {
        self.lang.get(match self.view {
            BtView::Home => Str::AppBluetooth,
            BtView::Scan => Str::BtAvailable,
            BtView::Paired => Str::BtPaired,
        })
    }

    fn list_kind(&self) -> BtList {
        if self.view == BtView::Scan { BtList::Found } else { BtList::Paired }
    }

    fn refresh<S: Services + ?Sized>(&mut self, svc: &S) {
        self.lang = svc.lang();
        let l = self.lang;
        let mut rows = Rows::default();
        if let Some(h) = rows.push(self.title(), 0) {
            h.header = true;
        }
        let phase = svc.bt_phase();
        match self.view {
            BtView::Home => {
                if let Some(r) = rows.push(l.get(Str::AppBluetooth), T_POWER) {
                    r.acc = Accessory::Toggle(svc.setting(id::BLUETOOTH) != 0);
                }
                if let Some(r) = rows.push(l.get(phase_label(phase)), T_STATUS) {
                    r.icon = Some(match phase {
                        BtPhase::Connected => &ui_icons::BT_CONNECTED,
                        BtPhase::Streaming => &ui_icons::BT_STREAMING,
                        _ => &ui_icons::BT_ON,
                    });
                    r.dim = matches!(phase, BtPhase::Off | BtPhase::NotResponding);
                }
                // The device connected (or being connected to) on a row of
                // its own: a name does not fit beside the state.
                let name = svc.bt_device();
                if matches!(phase, BtPhase::Connecting | BtPhase::Connected | BtPhase::Streaming)
                    && !name.is_empty()
                    && let Some(r) = rows.push("", T_DEVICE)
                {
                    r.value.push_str_or_ellipsis(name);
                    let paired = (0..svc.bt_count(BtList::Paired))
                        .filter_map(|i| svc.bt_entry(BtList::Paired, i))
                        .find(|d| d.connected);
                    r.icon = Some(bt_kind_icon(paired.map_or(BtKind::Unknown, |d| kind_of(svc, &d))));
                }
                let up = phase.is_up();
                if let Some(r) = rows.push(l.get(Str::BtPairNew), T_SCAN) {
                    r.acc = Accessory::Chevron;
                    r.dim = !up;
                }
                if let Some(r) = rows.push(l.get(Str::BtPaired), T_PAIRED) {
                    r.acc = Accessory::Chevron;
                    r.dim = !up;
                    if up {
                        r.value.push_u32(u32::try_from(svc.bt_count(BtList::Paired)).unwrap_or(0));
                    }
                }
                if let Some(r) = rows.push(l.get(Str::Reconnect), T_RECONNECT) {
                    r.acc = Accessory::Toggle(svc.setting(id::BT_RECONNECT) != 0);
                }
            }
            BtView::Scan | BtView::Paired => {
                let list = self.list_kind();
                let n = svc.bt_count(list).min(BT_DEVICES);
                for i in 0..n {
                    let Some(d) = svc.bt_entry(list, i) else { break };
                    let Some(r) = rows.push("", u16::try_from(i).unwrap_or(0)) else { break };
                    d.push_label(&mut r.value);
                    r.icon = Some(bt_kind_icon(kind_of(svc, &d)));
                    r.acc = if list == BtList::Paired {
                        Accessory::Radio(d.connected)
                    } else if d.connected {
                        Accessory::Radio(true)
                    } else {
                        Accessory::None
                    };
                }
                if self.view == BtView::Scan {
                    if phase == BtPhase::Scanning {
                        if let Some(r) = rows.push(l.get(Str::BtSearching), T_INFO) {
                            r.dim = true;
                        }
                    } else {
                        if n == 0
                            && let Some(r) = rows.push(l.get(Str::BtNoneFound), T_INFO)
                        {
                            r.dim = true;
                        }
                        if let Some(r) = rows.push(l.get(Str::BtSearchAgain), T_AGAIN) {
                            r.dim = phase != BtPhase::Ready;
                        }
                    }
                } else if n == 0 {
                    if let Some(r) = rows.push(l.get(Str::BtNoPaired), T_INFO) {
                        r.dim = true;
                    }
                    if let Some(r) = rows.push(l.get(Str::BtPairNew), T_SCAN) {
                        r.acc = Accessory::Chevron;
                    }
                }
            }
        }
        self.rows = rows;
        self.list.invalidate();
    }

    fn focused_tag(&self) -> Option<u16> {
        self.rows.get(self.list.focus).filter(|r| !r.header).map(|r| r.tag)
    }

    /// The focused device row's device.
    fn focused_device<S: Services + ?Sized>(&self, svc: &S) -> Option<(usize, BtDevice)> {
        let t = self.focused_tag().filter(|&t| usize::from(t) < BT_DEVICES)?;
        let i = usize::from(t);
        svc.bt_entry(self.list_kind(), i).map(|d| (i, d))
    }

    fn reload<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        let focus = self.list.focus;
        self.refresh(cx.svc);
        let mut l = self.list;
        l.set_focus(&self.rows, focus, visible(), cx.now);
        self.list = l;
    }

    fn dialog_for<S: Services + ?Sized>(&self, cx: &mut Cx<'_, S>, d: &BtDevice, forget: bool) {
        let l = self.lang;
        let mut text = StrBuf::<24>::new();
        d.push_label(&mut text);
        cx.ui.dialog(if forget {
            let mut t = StrBuf::<96>::new();
            t.push_str(text.as_str());
            t.push_str(" · ");
            t.push_str(l.get(Str::BtForgetText));
            DialogSpec::destructive(
                DLG_FORGET,
                l.get(Str::BtForgetQ),
                t.as_str(),
                l.get(Str::Cancel),
                l.get(Str::BtForgetBtn),
            )
        } else {
            DialogSpec::confirm(
                DLG_DISCONNECT,
                l.get(Str::BtDisconnectQ),
                text.as_str(),
                l.get(Str::Cancel),
                l.get(Str::Ok),
            )
        });
    }

    fn activate<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) -> Nav {
        let Some(tag) = self.focused_tag() else { return Nav::Stay };
        let l = self.lang;
        let phase = cx.svc.bt_phase();
        match tag {
            T_POWER | T_RECONNECT => {
                let sid = if tag == T_POWER { id::BLUETOOTH } else { id::BT_RECONNECT };
                let v = i16::from(cx.svc.setting(sid) == 0);
                cx.svc.set_setting(sid, v);
                self.reload(cx);
                Nav::Stay
            }
            T_STATUS | T_DEVICE => {
                match phase {
                    BtPhase::Connected | BtPhase::Streaming => {
                        let n = cx.svc.bt_device();
                        let n = if n.is_empty() { "—" } else { n };
                        let d = BtDevice::new(Default::default(), BtKind::Unknown, n.as_bytes(), true);
                        self.dialog_for(cx, &d, false);
                    }
                    BtPhase::Connecting => cx.svc.bt_request(BtRequest::CancelConnect),
                    _ => {}
                }
                Nav::Stay
            }
            T_SCAN | T_PAIRED => {
                if phase.is_up() {
                    if tag == T_SCAN {
                        // Pair new device starts an inquiry (here, not in
                        // `enter`: coming back from a connection attempt
                        // rebuilds the list without a new one).
                        if phase == BtPhase::Ready {
                            cx.svc.bt_request(BtRequest::Scan);
                        }
                        return Nav::Push(BT_SCAN, Arg(0));
                    }
                    Nav::Push(BT_PAIRED, Arg(0))
                } else {
                    cx.ui.toast(l.get(Str::BtTurnOn));
                    Nav::Stay
                }
            }
            T_AGAIN => {
                if phase == BtPhase::Ready {
                    cx.svc.bt_request(BtRequest::Scan);
                    self.reload(cx);
                } else if phase.is_connected() {
                    cx.ui.toast(l.get(Str::BtDisconnectFirst));
                }
                Nav::Stay
            }
            T_INFO => Nav::Stay,
            _ => {
                let Some((i, d)) = self.focused_device(cx.svc) else { return Nav::Stay };
                if d.connected && phase.is_connected() {
                    self.dialog_for(cx, &d, false);
                    Nav::Stay
                } else if phase.is_up() {
                    Nav::Push(BT_CONNECT, connect_arg(self.list_kind(), i))
                } else {
                    cx.ui.toast(l.get(Str::BtTurnOn));
                    Nav::Stay
                }
            }
        }
    }
}

/// A paired device's kind from the found list (the paired list carries
/// no class of device).
fn kind_of<S: Services + ?Sized>(svc: &S, d: &BtDevice) -> BtKind {
    if d.kind != BtKind::Unknown {
        return d.kind;
    }
    (0..svc.bt_count(BtList::Found))
        .filter_map(|i| svc.bt_entry(BtList::Found, i))
        .find(|f| f.addr == d.addr)
        .map_or(BtKind::Unknown, |f| f.kind)
}

impl<S: Services + ?Sized> Screen<S> for BluetoothScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.refresh(cx.svc);
        // An empty paired list opens on "Pair new device…".
        let start = if self.view == BtView::Paired { self.rows.find(T_SCAN).unwrap_or(0) } else { 0 };
        let mut l = self.list;
        l.set_focus(&self.rows, start, visible(), cx.now);
        self.list = l;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => {
                if self.view == BtView::Scan && cx.svc.bt_phase() == BtPhase::Scanning {
                    cx.svc.bt_request(BtRequest::StopScan);
                }
                return Nav::Pop;
            }
            Event::Service(_) => {
                self.reload(cx);
                return Nav::Stay;
            }
            Event::Ui(UiEvent::Dialog { id, button }) => {
                if button == 1 {
                    match id {
                        DLG_DISCONNECT => cx.svc.bt_request(BtRequest::Disconnect),
                        DLG_FORGET => {
                            if let Some((_, d)) = self.focused_device(cx.svc) {
                                cx.svc.bt_request(BtRequest::Forget(d.addr));
                                cx.ui.toast(self.lang.get(Str::Done));
                            }
                        }
                        _ => {}
                    }
                    self.reload(cx);
                }
                return Nav::Stay;
            }
            Event::Ui(UiEvent::Menu { id, .. }) => {
                let Some((i, d)) = self.focused_device(cx.svc) else { return Nav::Stay };
                return match id {
                    M_CONNECT => Nav::Push(BT_CONNECT, connect_arg(self.list_kind(), i)),
                    M_DISCONNECT | M_FORGET => {
                        self.dialog_for(cx, &d, id == M_FORGET);
                        Nav::Stay
                    }
                    _ => Nav::Stay,
                };
            }
            _ => {}
        }
        let mut l = self.list;
        let act = l.handle(&self.rows, ev, visible(), cx.now);
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

    fn menu(&self, cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        if self.view != BtView::Paired || out.parent().is_some() {
            return;
        }
        let Some((_, d)) = self.focused_device(cx.svc) else { return };
        let l = self.lang;
        if d.connected && cx.svc.bt_phase().is_connected() {
            out.action(M_DISCONNECT, l.get(Str::BtDisconnect));
        } else {
            out.action(M_CONNECT, l.get(Str::BtConnect));
        }
        out.action(M_FORGET, l.get(Str::BtForget));
    }

    fn wants(&self) -> Wants {
        let strip = match self.view {
            BtView::Scan => StripUse::Hint(self.lang.get(Str::HintBtScan)),
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
        l.set_focus(&self.rows, r.focus, visible(), cx.now);
        self.list = l;
    }
}

/// Give up waiting for the module after this long (it cancels its own
/// attempt after 15 s; a stopped inquiry comes first).
const CONNECT_TIMEOUT_MS: u64 = 40_000;

/// Connecting to a device: spinner, then back on success (a toast says
/// so), or the failure dialog (Cancel / Try again). Back cancels.
#[derive(Default)]
pub struct BtConnectScreen {
    lang: Lang,
    dev: BtDevice,
    since: Instant,
    /// The failure dialog is open.
    failed: bool,
    drawn: bool,
}

impl BtConnectScreen {
    fn is_connected<S: Services + ?Sized>(&self, svc: &S) -> bool {
        svc.bt_phase().is_connected() && svc.bt_device() == self.dev.name()
    }

    fn start<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        self.failed = false;
        self.since = cx.now;
        cx.svc.bt_request(BtRequest::Connect(self.dev.addr));
    }

    fn fail<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        if self.failed {
            return;
        }
        self.failed = true;
        let l = self.lang;
        let mut t = StrBuf::<96>::new();
        self.dev.push_label(&mut t);
        t.push_str(" · ");
        t.push_str(l.get(Str::BtFailedText));
        cx.ui.dialog(DialogSpec::confirm(
            DLG_FAILED,
            l.get(Str::BtFailed),
            t.as_str(),
            l.get(Str::Cancel),
            l.get(Str::BtTryAgain),
        ));
    }
}

impl<S: Services + ?Sized> Screen<S> for BtConnectScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.lang = cx.svc.lang();
        let (list, i) = connect_target(arg);
        self.dev = cx.svc.bt_entry(list, i).unwrap_or_default();
        self.drawn = false;
        self.start(cx);
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => {
                cx.svc.bt_request(BtRequest::CancelConnect);
                Nav::Pop
            }
            Event::Ui(UiEvent::Dialog { button, .. }) => {
                if button == 1 && cx.svc.bt_phase().is_up() {
                    self.start(cx);
                    self.drawn = false;
                    Nav::Stay
                } else {
                    Nav::Pop
                }
            }
            Event::Service(ServiceEvent::Other { kind: other::BLUETOOTH, value }) if !self.failed => {
                match value {
                    bt_event::CONNECTED => Nav::Pop,
                    bt_event::CONNECT_FAILED => {
                        self.fail(cx);
                        Nav::Stay
                    }
                    _ => {
                        let p = cx.svc.bt_phase();
                        if self.is_connected(cx.svc) {
                            Nav::Pop
                        } else if !p.is_up() {
                            self.fail(cx);
                            Nav::Stay
                        } else {
                            Nav::Stay
                        }
                    }
                }
            }
            Event::Tick(now) if !self.failed => {
                if self.is_connected(cx.svc) {
                    // Rebuilt after a cover (quick panel) while it connected.
                    Nav::Pop
                } else if now.since(self.since) >= CONNECT_TIMEOUT_MS {
                    cx.svc.bt_request(BtRequest::CancelConnect);
                    self.fail(cx);
                    Nav::Stay
                } else {
                    Nav::Stay
                }
            }
            _ => Nav::Stay,
        }
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let (w, _) = g.size();
        if dirty == Dirty::All || !self.drawn {
            g.clear(Role::Bg);
            let acc = g.c(Role::Accent);
            g.icon2((w - 24) / 2, 6, &icons::BLUETOOTH, acc);
            let text = g.c(Role::Text);
            let dim = g.c(Role::TextDim);
            let l = self.lang;
            let head = if self.failed { Str::BtFailed } else { Str::BtConnecting };
            g.text_in(4, 48, w - 8, l.get(head), FontId::UiBold, text, Align::Center);
            let mut name = StrBuf::<24>::new();
            self.dev.push_label(&mut name);
            g.text_in(4, 64, w - 8, name.as_str(), FontId::Ui, dim, Align::Center);
            self.drawn = true;
        }
        if !self.failed {
            draw_spinner(g, (w - 14) / 2, 74, cx.now, Role::Bg);
        }
    }

    fn wants(&self) -> Wants {
        Wants {
            ticks: !self.failed,
            title: Some(self.lang.get(Str::AppBluetooth)),
            strip: StripUse::Hint(self.lang.get(Str::HintBtConnect)),
            ..Wants::default()
        }
    }
}
