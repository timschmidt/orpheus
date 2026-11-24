//! Typed commands and events (docs/hw/bluetooth.md §3.4–3.6). Meanings
//! marked *inferred* there are named after the inference; parameters whose
//! meaning is unknown are passed through untouched.

use crate::frame::{self, EncodeError, Frame, MAX_TX_PARAMS};

/// A Bluetooth device address as the module sends it: least-significant
/// byte first (HCI order, seen on the player 2026-11-24). Kept in that
/// order, compared and sent back exactly as received; only
/// [`Addr::to_text`] reverses it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Addr(pub [u8; 6]);

impl Addr {
    /// `XX:XX:XX:XX:XX:XX` as the address is written: the module sends it
    /// least-significant byte first (HCI order; seen on the player
    /// 2026-11-24: `E0:AD:47:32:1B:1F` arrives as `1F 1B 32 47 AD E0`), so
    /// the text is the stored bytes reversed. The stored bytes stay in
    /// wire order for commands.
    #[must_use]
    pub fn to_text(&self) -> [u8; 17] {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let mut t = [b':'; 17];
        for (i, b) in self.0.iter().rev().enumerate() {
            t[i * 3] = HEX[usize::from(b >> 4)];
            t[i * 3 + 1] = HEX[usize::from(b & 15)];
        }
        t
    }

    fn from(p: &[u8]) -> Self {
        let mut a = [0u8; 6];
        if let Some(s) = p.get(..6) {
            a.copy_from_slice(s);
        }
        Addr(a)
    }
}

/// Track-metadata field sizes of command 0x31 steps 2–5 (zero-padded).
pub const META_FIELDS: [usize; 4] = [72, 52, 20, 52];

/// Commands the firmware may send automatically. The restricted IDs
/// (0x20–0x24, 0x28, 0x2C, 0x7F) are deliberately absent: they live in
/// [`crate::restricted`]. Commands with unknown meaning (0x27, 0x2D, 0x32,
/// 0x05 subs 0x16/0x17) belong to the BT-3 investigation and are absent
/// too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command<'a> {
    /// 0x00 `0F 0C 03`: start inquiry (the three bytes are the vendor's
    /// constants, meaning unknown).
    StartInquiry,
    /// 0x01 `00`.
    StopInquiry,
    /// 0x02 addr: request a remote name (event 0x05).
    RemoteName(Addr),
    /// 0x03 addr: connect, pairing if needed (events 0x02).
    Connect(Addr),
    /// 0x04 `00`.
    Disconnect,
    /// 0x05 `14 00` / `15 00`: audio output active / stopped.
    Audio(bool),
    /// 0x06 n: identity query (2 = model string, 0 = liveness probe).
    Identity(u8),
    /// 0x25: read the sink's absolute volume (event 0x25).
    ReadAbsVolume,
    /// 0x26 v: set the sink's absolute volume, 0–127.
    SetAbsVolume(u8),
    /// 0x29: cancel a connection attempt (event 0x29).
    CancelConnect,
    /// 0x2A `FF`: read the paired list (events 0x2A).
    ReadPaired,
    /// 0x2B addr: forget one paired device.
    Forget(Addr),
    /// 0x2B `FF`: forget all paired devices.
    ForgetAll,
    /// 0x30 `01 01`: sent once at every start by the vendor; meaning
    /// unknown.
    StartupMagic,
    /// 0x31 `00`: begin track metadata.
    MetaBegin,
    /// 0x31 `01` secs[2]: track length in seconds.
    MetaLength(u16),
    /// 0x31 `02..05` text: field 0–3 (title first), zero-padded to
    /// [`META_FIELDS`]; longer text is cut at a UTF-8 boundary.
    MetaField(u8, &'a str),
    /// 0x31 `FF`: commit metadata.
    MetaEnd,
}

impl Command<'_> {
    #[must_use]
    pub fn id(&self) -> u8 {
        match self {
            Command::StartInquiry => 0x00,
            Command::StopInquiry => 0x01,
            Command::RemoteName(_) => 0x02,
            Command::Connect(_) => 0x03,
            Command::Disconnect => 0x04,
            Command::Audio(_) => 0x05,
            Command::Identity(_) => 0x06,
            Command::ReadAbsVolume => 0x25,
            Command::SetAbsVolume(_) => 0x26,
            Command::CancelConnect => 0x29,
            Command::ReadPaired => 0x2A,
            Command::Forget(_) | Command::ForgetAll => 0x2B,
            Command::StartupMagic => 0x30,
            Command::MetaBegin | Command::MetaLength(_) | Command::MetaField(..) | Command::MetaEnd => 0x31,
        }
    }

    /// Encode the frame into `out` (at least [`frame::MAX_TX_FRAME`]
    /// bytes for every command).
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, EncodeError> {
        let mut p = [0u8; MAX_TX_PARAMS];
        let n = match *self {
            Command::StartInquiry => {
                p[..3].copy_from_slice(&[0x0F, 0x0C, 0x03]);
                3
            }
            Command::StopInquiry | Command::Disconnect => 1,
            Command::RemoteName(a) | Command::Connect(a) | Command::Forget(a) => {
                p[..6].copy_from_slice(&a.0);
                6
            }
            Command::Audio(on) => {
                p[0] = if on { 0x14 } else { 0x15 };
                2
            }
            Command::Identity(n) => {
                p[0] = n;
                1
            }
            Command::ReadAbsVolume | Command::CancelConnect | Command::MetaBegin => {
                // MetaBegin carries one 00 parameter, the others none.
                usize::from(matches!(self, Command::MetaBegin))
            }
            Command::SetAbsVolume(v) => {
                p[0] = v.min(127);
                1
            }
            Command::ReadPaired | Command::ForgetAll => {
                p[0] = 0xFF;
                1
            }
            Command::StartupMagic => {
                p[..2].copy_from_slice(&[0x01, 0x01]);
                2
            }
            Command::MetaLength(s) => {
                p[0] = 0x01;
                p[1..3].copy_from_slice(&s.to_be_bytes());
                3
            }
            Command::MetaField(i, text) => {
                let i = usize::from(i.min(3));
                let size = META_FIELDS[i];
                p[0] = 0x02 + i as u8;
                let mut end = text.len().min(size);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                if let (Some(d), Some(s)) = (p.get_mut(1..1 + end), text.as_bytes().get(..end)) {
                    d.copy_from_slice(s);
                }
                1 + size
            }
            Command::MetaEnd => {
                p[0] = 0xFF;
                1
            }
        };
        frame::encode(self.id(), p.get(..n).unwrap_or(&[]), out)
    }
}

/// AVRCP pass-through key from the headset (AV/C operation IDs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Play,
    Stop,
    Pause,
    Next,
    Previous,
    /// Ignored by the vendor; passed on in case the module forwards more.
    Other(u8),
}

impl Key {
    #[must_use]
    pub fn from_code(c: u8) -> Self {
        match c {
            0x44 => Key::Play,
            0x45 => Key::Stop,
            0x46 => Key::Pause,
            0x4B => Key::Next,
            0x4C => Key::Previous,
            o => Key::Other(o),
        }
    }
}

/// Link-state code of event 0x02.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Connected,
    Disconnected,
    /// st 2: second channel up (probably AVRCP; arms a volume sync).
    ControlUp,
    /// st 3: second channel down.
    ControlDown,
    Failed,
    Connecting,
    Other(u8),
}

/// A decoded event. Text fields borrow from the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event<'a> {
    /// 0x00: acknowledge of command `cmd`.
    Ack { cmd: u8 },
    /// 0x01.
    Key(Key),
    /// 0x02 st, ?, addr.
    Link { state: LinkState, extra: u8, addr: Addr },
    /// 0x03 addr, ?, class of device, name.
    InquiryResult { addr: Addr, extra: u8, cod: [u8; 3], name: &'a [u8] },
    /// 0x04.
    InquiryDone,
    /// 0x05 addr, name.
    RemoteName { addr: Addr, name: &'a [u8] },
    /// 0x06 model string (this unit: `ATS2851\0`).
    Identity(&'a [u8]),
    /// 0x20/0x21/0x22: upgrade replies (only after restricted commands).
    Upgrade { id: u8, status: u8, value: u16 },
    /// 0x23: module started and ready.
    Ready,
    /// 0x24: shutdown complete.
    ShutdownComplete,
    /// 0x25: `Some(v)` if the sink supports absolute volume.
    AbsVolume(Option<u8>),
    /// 0x26: the sink changed its volume (0–127).
    SinkVolume(u8),
    /// 0x29: answer to cancel-connect.
    CancelConnect { status: u8 },
    /// 0x2A total, ?, addr, name: one paired-list record.
    Paired { total: u8, extra: u8, addr: Addr, name: &'a [u8] },
    /// 0x2C baud index, TX power.
    Settings { baud_index: u8, power: u8 },
    /// Anything else, including too-short frames of known IDs.
    Other { id: u8, params: &'a [u8] },
}

/// Strip zero padding from a name.
fn name(b: &[u8]) -> &[u8] {
    b.split(|&c| c == 0).next().unwrap_or(b)
}

impl<'a> Event<'a> {
    #[must_use]
    pub fn parse(f: &'a Frame) -> Self {
        let p = f.params();
        let other = Event::Other { id: f.id, params: p };
        match (f.id, p.len()) {
            (0x00, 1..) => Event::Ack { cmd: p[0] },
            (0x01, 1..) => Event::Key(Key::from_code(p[0])),
            (0x02, 8..) => {
                let state = match p[0] {
                    0 => LinkState::Connected,
                    1 => LinkState::Disconnected,
                    2 => LinkState::ControlUp,
                    3 => LinkState::ControlDown,
                    4 => LinkState::Failed,
                    5 => LinkState::Connecting,
                    o => LinkState::Other(o),
                };
                Event::Link { state, extra: p[1], addr: Addr::from(&p[2..8]) }
            }
            (0x03, 10..) => Event::InquiryResult {
                addr: Addr::from(p),
                extra: p[6],
                cod: [p[7], p[8], p[9]],
                name: name(&p[10..]),
            },
            (0x04, _) => Event::InquiryDone,
            (0x05, 6..) => Event::RemoteName { addr: Addr::from(p), name: name(&p[6..]) },
            (0x06, _) => Event::Identity(p),
            (0x20 | 0x22, 3..) => {
                Event::Upgrade { id: f.id, status: p[0], value: u16::from_be_bytes([p[1], p[2]]) }
            }
            (0x20..=0x22, 1..) => Event::Upgrade { id: f.id, status: p[0], value: 0 },
            (0x23, _) => Event::Ready,
            (0x24, _) => Event::ShutdownComplete,
            (0x25, 1..) => Event::AbsVolume((p[0] < 0x80).then_some(p[0])),
            (0x26, 1..) => Event::SinkVolume(p[0]),
            (0x29, 1..) => Event::CancelConnect { status: p[0] },
            (0x2A, 8..) => {
                Event::Paired { total: p[0], extra: p[1], addr: Addr::from(&p[2..8]), name: name(&p[8..]) }
            }
            (0x2C, 2..) => Event::Settings { baud_index: p[0], power: p[1] },
            _ => other,
        }
    }
}

/// Player volume (0–31) to the sink's absolute volume (0–127), as the
/// vendor scales it (bluetooth.md §3.5).
#[must_use]
pub const fn volume_to_sink(local: u8) -> u8 {
    if local >= 31 { 127 } else { (local as u16 * 128 / 31) as u8 }
}

/// Sink volume (0–127) to the player's 0–31 scale; 0 only for 0.
#[must_use]
pub const fn volume_from_sink(v: u8) -> u8 {
    if v == 0 {
        return 0;
    }
    let l = ((v as u16 + 1) * 31) >> 7;
    if l == 0 { 1 } else { l as u8 }
}

/// The player's volume steps (0–30, the UI's scale) to the module's 0–31
/// scale of [`volume_to_sink`].
#[must_use]
pub const fn local_from_ui(v: u8) -> u8 {
    let v = if v > 30 { 30 } else { v };
    ((v as u16 * 31 + 15) / 30) as u8
}

/// The module's 0–31 scale back to the UI's 0–30.
#[must_use]
pub const fn ui_from_local(l: u8) -> u8 {
    let l = if l > 31 { 31 } else { l };
    ((l as u16 * 30 + 15) / 31) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A UI volume sent to the sink and echoed back lands on itself (no
    /// drift when the sink confirms every change).
    #[test]
    fn ui_volume_round_trip() {
        for v in 0..=30u8 {
            let raw = volume_to_sink(local_from_ui(v));
            assert_eq!(ui_from_local(volume_from_sink(raw)), v, "{v} -> {raw}");
        }
        assert_eq!(local_from_ui(30), 31);
        assert_eq!(ui_from_local(31), 30);
        assert_eq!(ui_from_local(volume_from_sink(127)), 30);
    }
    use crate::frame::MAX_TX_FRAME;

    fn bytes(c: Command<'_>) -> std::vec::Vec<u8> {
        let mut b = [0u8; MAX_TX_FRAME];
        let n = c.encode(&mut b).unwrap();
        b[..n].to_vec()
    }

    #[test]
    fn command_bytes() {
        assert_eq!(bytes(Command::Identity(2)), [0xAA, 0, 2, 0x06, 0x02, 0xF6]);
        assert_eq!(bytes(Command::StartupMagic), [0xAA, 0, 3, 0x30, 1, 1, 0xCB]);
        assert_eq!(bytes(Command::ReadPaired), [0xAA, 0, 2, 0x2A, 0xFF, 0xD5]);
        assert_eq!(bytes(Command::StartInquiry), [0xAA, 0, 4, 0, 0x0F, 0x0C, 0x03, 0xDE]);
        assert_eq!(bytes(Command::StopInquiry), [0xAA, 0, 2, 1, 0, 0xFD]);
        assert_eq!(bytes(Command::Audio(true)), [0xAA, 0, 3, 5, 0x14, 0, 0xE4]);
        assert_eq!(bytes(Command::Audio(false)), [0xAA, 0, 3, 5, 0x15, 0, 0xE3]);
        assert_eq!(bytes(Command::SetAbsVolume(0x40)), [0xAA, 0, 2, 0x26, 0x40, 0x98]);
        assert_eq!(bytes(Command::ReadAbsVolume), [0xAA, 0, 1, 0x25, 0xDA]);
        assert_eq!(bytes(Command::Disconnect), [0xAA, 0, 2, 4, 0, 0xFA]);
        assert_eq!(bytes(Command::CancelConnect), [0xAA, 0, 1, 0x29, 0xD6]);
        let a = Addr([1, 2, 3, 4, 5, 6]);
        assert_eq!(bytes(Command::Connect(a)), [0xAA, 0, 7, 3, 1, 2, 3, 4, 5, 6, 0xE1]);
        assert_eq!(bytes(Command::ForgetAll), [0xAA, 0, 2, 0x2B, 0xFF, 0xD4]);
        assert_eq!(bytes(Command::MetaBegin), [0xAA, 0, 2, 0x31, 0, 0xCD]);
        assert_eq!(bytes(Command::MetaLength(0x0102)), [0xAA, 0, 4, 0x31, 1, 1, 2, 0xC7]);
        assert_eq!(bytes(Command::MetaEnd), [0xAA, 0, 2, 0x31, 0xFF, 0xCE]);
        let t = bytes(Command::MetaField(0, "Title"));
        assert_eq!(t.len(), 5 + 73);
        assert_eq!(&t[..10], &[0xAA, 0, 74, 0x31, 2, b'T', b'i', b't', b'l', b'e']);
        assert!(t[10..77].iter().all(|&b| b == 0));
        assert_eq!(crate::frame::checksum(&t[1..]), 0);
        for (i, &size) in META_FIELDS.iter().enumerate() {
            let long = "é".repeat(100);
            let f = bytes(Command::MetaField(i as u8, &long));
            assert_eq!(f.len(), 6 + size);
            let text = &f[5..5 + size];
            let end = text.iter().position(|&b| b == 0).unwrap_or(size);
            assert!(core::str::from_utf8(&text[..end]).is_ok());
        }
    }

    #[test]
    fn events() {
        let f = Frame::new(0x06, b"ATS2851\0");
        assert_eq!(Event::parse(&f), Event::Identity(b"ATS2851\0"));
        let f = Frame::new(0x2C, &[6, 7]);
        assert_eq!(Event::parse(&f), Event::Settings { baud_index: 6, power: 7 });
        let f = Frame::new(0x29, &[1]);
        assert_eq!(Event::parse(&f), Event::CancelConnect { status: 1 });
        let f = Frame::new(0x02, &[0, 9, 1, 2, 3, 4, 5, 6]);
        assert_eq!(
            Event::parse(&f),
            Event::Link { state: LinkState::Connected, extra: 9, addr: Addr([1, 2, 3, 4, 5, 6]) }
        );
        let f = Frame::new(0x03, b"\x01\x02\x03\x04\x05\x06\xC8\x04\x04\x24Head\0\0");
        assert_eq!(
            Event::parse(&f),
            Event::InquiryResult {
                addr: Addr([1, 2, 3, 4, 5, 6]),
                extra: 0xC8,
                cod: [4, 4, 0x24],
                name: b"Head"
            }
        );
        assert_eq!(Event::parse(&Frame::new(0x25, &[0x81])), Event::AbsVolume(None));
        assert_eq!(Event::parse(&Frame::new(0x25, &[0x30])), Event::AbsVolume(Some(0x30)));
        assert_eq!(Event::parse(&Frame::new(0x01, &[0x4B])), Event::Key(Key::Next));
        // Short frames of known IDs are passed through as Other.
        assert_eq!(Event::parse(&Frame::new(0x02, &[0])), Event::Other { id: 0x02, params: &[0] });
        // Written most-significant byte first: the wire order reversed.
        assert_eq!(Addr([0xDE, 0xAD, 0, 1, 2, 0xFF]).to_text(), *b"FF:02:01:00:AD:DE");
        assert_eq!(Addr([0x1F, 0x1B, 0x32, 0x47, 0xAD, 0xE0]).to_text(), *b"E0:AD:47:32:1B:1F");
    }

    #[test]
    fn volume_scaling() {
        assert_eq!(volume_to_sink(0), 0);
        assert_eq!(volume_to_sink(16), 66);
        assert_eq!(volume_to_sink(31), 127);
        assert_eq!(volume_to_sink(40), 127);
        assert_eq!(volume_from_sink(0), 0);
        assert_eq!(volume_from_sink(1), 1);
        assert_eq!(volume_from_sink(127), 31);
        for l in 0..=31 {
            assert_eq!(volume_from_sink(volume_to_sink(l)), l, "{l}");
        }
    }
}
