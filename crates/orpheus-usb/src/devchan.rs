//! Developer channel: vendor requests on EP0 (06-connectivity §2.5).
//!
//! | Req | Dir | wValue / wIndex | Data |
//! |---|---|---|---|
//! | INFO 0x01 | IN | — | [`orpheus_proto::devchan::Info`] (32 bytes) |
//! | LOG 0x02 | IN | offset (wIndex:wValue) into the log-ring area | raw ring bytes |
//! | FRAME 0x03 | IN | byte offset (wIndex:wValue) into the frame buffer | RGB565 bytes |
//! | KEYS 0x04 | OUT | — | `[key, down]` pairs |
//! | STATS 0x05 | IN | group | counters (u32 LE) |
//! | REBOOT 0x07 | OUT | target | — |
//! | PEEK 0x0A | IN | address (wIndex:wValue) | memory bytes (development builds) |
//! | UPDATE 0x0D | OUT | operation / chunk (`orpheus_update::wire`) | `.ofw` header, image bytes |
//! | UPDATE 0x0D | IN | — | update status (16 bytes) |
//! | WebUSB `GET_URL` | IN | 1 / 2 | the landing page ([`crate::webusb`]) |

use embassy_usb::Handler;
use embassy_usb::control::{InResponse, OutResponse, Recipient, Request, RequestType};
use orpheus_proto::devchan::{
    self as dc, Info, REQ_FRAME, REQ_INFO, REQ_KEYS, REQ_LOG, REQ_PEEK, REQ_PROVISION, REQ_PUT, REQ_REBOOT,
    REQ_STATS, REQ_TEST, REQ_TIME, REQ_UPDATE, WEBUSB_VENDOR_CODE,
};

/// What the firmware provides to the channel.
pub trait Target {
    fn info(&mut self) -> Info;
    /// Copy `buf.len()` bytes of memory from `addr` (any address the
    /// firmware allows; development builds).
    fn read_mem(&mut self, addr: u32, buf: &mut [u8]) -> bool;
    /// Frame buffer bytes from `offset`.
    fn read_frame(&mut self, offset: u32, buf: &mut [u8]) -> usize;
    /// Log-ring area bytes from `offset`.
    fn read_log(&mut self, offset: u32, buf: &mut [u8]) -> usize;
    /// Counter group `group` into `out`; returns words written.
    fn stats(&mut self, group: u16, out: &mut [u32]) -> usize;
    /// Key events (`key` index, down).
    fn keys(&mut self, events: &[(u8, bool)]);
    /// Reboot after the status stage (target per `devchan::REBOOT_*`).
    fn request_reboot(&mut self, target: u16);
    /// Whether development-only requests (PEEK) are allowed.
    fn development(&self) -> bool;
    /// PUT OUT: data for the file being uploaded. Returns false when busy.
    fn put(&mut self, _data: &[u8]) -> bool {
        false
    }
    /// TIME OUT: set the clock (UTC: year u16 LE, month, day, hour, minute,
    /// second). Returns false if rejected.
    fn set_time(&mut self, _t: [u8; 7]) -> bool {
        false
    }
    /// TIME IN: the clock in the same layout, if set.
    fn time(&mut self) -> Option<[u8; 7]> {
        None
    }
    /// TEST OUT: a command for the firmware's test hooks (text).
    fn test_command(&mut self, _data: &[u8]) -> bool {
        false
    }
    /// TEST IN: the reply of the last command; bytes written.
    fn test_reply(&mut self, _buf: &mut [u8]) -> usize {
        0
    }
    /// PROVISION OUT: one `otpauth://` URI for the authenticator (06 §2.5,
    /// §5.4). Accepted only while the vault is unlocked; the entry is added
    /// after the user confirms it on the player. The target must wipe its
    /// copy of the data once handled.
    fn provision(&mut self, _uri: &[u8]) -> bool {
        false
    }
    /// UPDATE OUT: operation `op` (`wValue`), chunk number `index`
    /// (`wIndex`), data. Returns false if refused (the status says why).
    fn update(&mut self, _op: u16, _index: u16, _data: &[u8]) -> bool {
        false
    }
    /// UPDATE IN: the update status into `buf`; bytes written.
    fn update_status(&mut self, _buf: &mut [u8]) -> usize {
        0
    }
}

pub struct DevChannel<T: Target> {
    pub target: T,
    pub requests: u32,
}

impl<T: Target> DevChannel<T> {
    pub fn new(target: T) -> Self {
        Self { target, requests: 0 }
    }

    fn mine(req: &Request) -> bool {
        req.request_type == RequestType::Vendor && req.recipient == Recipient::Device
    }
}

fn addr32(req: &Request) -> u32 {
    u32::from(req.index) << 16 | u32::from(req.value)
}

impl<T: Target> Handler for DevChannel<T> {
    fn control_out(&mut self, req: Request, data: &[u8]) -> Option<OutResponse> {
        if !Self::mine(&req) {
            return None;
        }
        self.requests += 1;
        Some(match req.request {
            REQ_REBOOT => {
                self.target.request_reboot(req.value);
                OutResponse::Accepted
            }
            REQ_TIME => match <[u8; 7]>::try_from(data) {
                Ok(t) if self.target.set_time(t) => OutResponse::Accepted,
                _ => OutResponse::Rejected,
            },
            REQ_PUT => {
                if self.target.put(data) {
                    OutResponse::Accepted
                } else {
                    OutResponse::Rejected
                }
            }
            REQ_TEST => {
                if self.target.test_command(data) {
                    OutResponse::Accepted
                } else {
                    OutResponse::Rejected
                }
            }
            REQ_UPDATE => {
                if self.target.update(req.value, req.index, data) {
                    OutResponse::Accepted
                } else {
                    OutResponse::Rejected
                }
            }
            REQ_PROVISION => {
                if self.target.provision(data) {
                    OutResponse::Accepted
                } else {
                    OutResponse::Rejected
                }
            }
            REQ_KEYS => {
                let mut ev = [(0u8, false); 16];
                let mut n = 0;
                for pair in data.chunks_exact(2).take(16) {
                    ev[n] = (pair[0], pair[1] != 0);
                    n += 1;
                }
                self.target.keys(&ev[..n]);
                OutResponse::Accepted
            }
            _ => OutResponse::Rejected,
        })
    }

    fn control_in<'a>(&'a mut self, req: Request, buf: &'a mut [u8]) -> Option<InResponse<'a>> {
        if !Self::mine(&req) {
            return None;
        }
        self.requests += 1;
        let want = usize::from(req.length).min(buf.len());
        if req.request == WEBUSB_VENDOR_CODE {
            // WebUSB GET_URL: only the landing page exists.
            if req.index == crate::webusb::GET_URL && req.value == crate::webusb::LANDING_INDEX {
                let n = crate::webusb::url_descriptor(buf).min(want);
                return Some(InResponse::Accepted(&buf[..n]));
            }
            return Some(InResponse::Rejected);
        }
        Some(match req.request {
            REQ_INFO => {
                let b = self.target.info().to_bytes();
                let n = want.min(b.len());
                buf[..n].copy_from_slice(&b[..n]);
                InResponse::Accepted(&buf[..n])
            }
            REQ_FRAME => {
                let n = self.target.read_frame(addr32(&req), &mut buf[..want]);
                InResponse::Accepted(&buf[..n])
            }
            REQ_LOG => {
                let n = self.target.read_log(addr32(&req), &mut buf[..want]);
                InResponse::Accepted(&buf[..n])
            }
            REQ_STATS => {
                let mut words = [0u32; 64];
                let n = self.target.stats(req.value, &mut words).min(want / 4);
                for (i, w) in words[..n].iter().enumerate() {
                    buf[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
                }
                InResponse::Accepted(&buf[..4 * n])
            }
            REQ_TIME => match self.target.time() {
                Some(t) => {
                    let n = want.min(7);
                    buf[..n].copy_from_slice(&t[..n]);
                    InResponse::Accepted(&buf[..n])
                }
                None => InResponse::Rejected,
            },
            REQ_TEST => {
                let n = self.target.test_reply(&mut buf[..want]);
                InResponse::Accepted(&buf[..n])
            }
            REQ_UPDATE => {
                let n = self.target.update_status(&mut buf[..want]);
                InResponse::Accepted(&buf[..n])
            }
            REQ_PEEK if self.target.development() => {
                if self.target.read_mem(addr32(&req), &mut buf[..want]) {
                    InResponse::Accepted(&buf[..want])
                } else {
                    InResponse::Rejected
                }
            }
            _ => InResponse::Rejected,
        })
    }
}

/// Re-exported so firmware and host agree on numbers.
pub use dc::{REBOOT_ADFU, REBOOT_NORMAL, REBOOT_OTHER_SLOT, REBOOT_STOCK};

#[cfg(test)]
mod tests {
    use super::*;
    use embassy_usb::control::Request;
    use std::vec::Vec;

    #[derive(Default)]
    struct Fake {
        reboot: Option<u16>,
        keys: Vec<(u8, bool)>,
        uri: Vec<u8>,
    }

    impl Target for Fake {
        fn info(&mut self) -> Info {
            Info { version: dc::VERSION, uptime_ms: 42, ..Info::default() }
        }
        fn read_mem(&mut self, addr: u32, buf: &mut [u8]) -> bool {
            for (i, b) in buf.iter_mut().enumerate() {
                *b = (addr as usize + i) as u8;
            }
            true
        }
        fn read_frame(&mut self, offset: u32, buf: &mut [u8]) -> usize {
            let n = buf.len().min(32768usize.saturating_sub(offset as usize));
            buf[..n].fill(0xAB);
            n
        }
        fn read_log(&mut self, _offset: u32, _buf: &mut [u8]) -> usize {
            0
        }
        fn stats(&mut self, group: u16, out: &mut [u32]) -> usize {
            out[0] = u32::from(group);
            out[1] = 7;
            2
        }
        fn keys(&mut self, events: &[(u8, bool)]) {
            self.keys.extend_from_slice(events);
        }
        fn provision(&mut self, uri: &[u8]) -> bool {
            self.uri = uri.to_vec();
            uri.starts_with(b"otpauth://")
        }
        fn request_reboot(&mut self, target: u16) {
            self.reboot = Some(target);
        }
        fn development(&self) -> bool {
            true
        }
    }

    fn req(dir_in: bool, request: u8, value: u16, index: u16, length: u16) -> Request {
        let mut raw = [0u8; 8];
        raw[0] = 0x40 | if dir_in { 0x80 } else { 0 };
        raw[1] = request;
        raw[2..4].copy_from_slice(&value.to_le_bytes());
        raw[4..6].copy_from_slice(&index.to_le_bytes());
        raw[6..8].copy_from_slice(&length.to_le_bytes());
        Request::parse(&raw)
    }

    #[test]
    fn info_and_peek() {
        let mut d = DevChannel::new(Fake::default());
        let mut buf = [0u8; 64];
        match d.control_in(req(true, REQ_INFO, 0, 0, 32), &mut buf) {
            Some(InResponse::Accepted(b)) => assert_eq!(Info::from_bytes(b).unwrap().uptime_ms, 42),
            _ => panic!("info"),
        }
        let mut buf = [0u8; 64];
        match d.control_in(req(true, REQ_PEEK, 0x0010, 0x0010, 4), &mut buf) {
            Some(InResponse::Accepted(b)) => assert_eq!(b, &[0x10, 0x11, 0x12, 0x13]),
            _ => panic!("peek"),
        }
    }

    #[test]
    fn frame_clamps_to_wlength_and_end() {
        let mut d = DevChannel::new(Fake::default());
        let mut buf = [0u8; 4096];
        match d.control_in(req(true, REQ_FRAME, 32760, 0, 4096), &mut buf) {
            Some(InResponse::Accepted(b)) => assert_eq!(b.len(), 8),
            _ => panic!("frame"),
        }
    }

    #[test]
    fn reboot_keys_and_foreign_requests() {
        let mut d = DevChannel::new(Fake::default());
        assert!(matches!(d.control_out(req(false, REQ_REBOOT, 1, 0, 0), &[]), Some(OutResponse::Accepted)));
        assert_eq!(d.target.reboot, Some(1));
        assert!(matches!(
            d.control_out(req(false, REQ_KEYS, 0, 0, 4), &[3, 1, 3, 0]),
            Some(OutResponse::Accepted)
        ));
        assert_eq!(d.target.keys, [(3, true), (3, false)]);
        let mut raw = [0u8; 8];
        raw[0] = 0x80; // standard request: not ours
        let r = Request::parse(&raw);
        let mut buf = [0u8; 8];
        assert!(d.control_in(r, &mut buf).is_none());
    }

    #[test]
    fn stats_words() {
        let mut d = DevChannel::new(Fake::default());
        let mut buf = [0u8; 64];
        match d.control_in(req(true, REQ_STATS, 5, 0, 64), &mut buf) {
            Some(InResponse::Accepted(b)) => assert_eq!(b, &[5, 0, 0, 0, 7, 0, 0, 0]),
            _ => panic!("stats"),
        }
    }

    #[test]
    fn update_reaches_the_target_and_url_answers() {
        #[derive(Default)]
        struct Upd {
            got: Vec<(u16, u16, usize)>,
        }
        impl Target for Upd {
            fn info(&mut self) -> Info {
                Info::default()
            }
            fn read_mem(&mut self, _a: u32, _b: &mut [u8]) -> bool {
                false
            }
            fn read_frame(&mut self, _o: u32, _b: &mut [u8]) -> usize {
                0
            }
            fn read_log(&mut self, _o: u32, _b: &mut [u8]) -> usize {
                0
            }
            fn stats(&mut self, _g: u16, _o: &mut [u32]) -> usize {
                0
            }
            fn keys(&mut self, _e: &[(u8, bool)]) {}
            fn request_reboot(&mut self, _t: u16) {}
            fn development(&self) -> bool {
                false
            }
            fn update(&mut self, op: u16, index: u16, data: &[u8]) -> bool {
                self.got.push((op, index, data.len()));
                op != 9
            }
            fn update_status(&mut self, buf: &mut [u8]) -> usize {
                let n = buf.len().min(16);
                buf[..n].fill(7);
                n
            }
        }
        let mut d = DevChannel::new(Upd::default());
        assert!(matches!(
            d.control_out(req(false, REQ_UPDATE, 1, 42, 1024), &[0; 1024]),
            Some(OutResponse::Accepted)
        ));
        assert!(matches!(d.control_out(req(false, REQ_UPDATE, 9, 0, 0), &[]), Some(OutResponse::Rejected)));
        assert_eq!(d.target.got, [(1, 42, 1024), (9, 0, 0)]);
        let mut buf = [0u8; 64];
        match d.control_in(req(true, REQ_UPDATE, 0, 0, 16), &mut buf) {
            Some(InResponse::Accepted(b)) => assert_eq!(b, &[7; 16]),
            _ => panic!("status"),
        }
        let mut buf = [0u8; 256];
        match d.control_in(req(true, WEBUSB_VENDOR_CODE, 1, 2, 255), &mut buf) {
            Some(InResponse::Accepted(b)) => assert_eq!(b[1], 3),
            _ => panic!("url"),
        }
        let mut buf = [0u8; 256];
        assert!(matches!(
            d.control_in(req(true, WEBUSB_VENDOR_CODE, 1, 1, 255), &mut buf),
            Some(InResponse::Rejected)
        ));
    }

    #[test]
    fn provision_reaches_the_target() {
        let mut d = DevChannel::new(Fake::default());
        let uri = b"otpauth://totp/A?secret=GEZDGNBV";
        assert!(matches!(
            d.control_out(req(false, REQ_PROVISION, 0, 0, uri.len() as u16), uri),
            Some(OutResponse::Accepted)
        ));
        assert_eq!(d.target.uri, uri);
        assert!(matches!(
            d.control_out(req(false, REQ_PROVISION, 0, 0, 3), b"bad"),
            Some(OutResponse::Rejected)
        ));
    }
}
