//! Developer channel: vendor requests on EP0 (06-connectivity §2.5).
//!
//! | Req | Dir | wValue / wIndex | Data |
//! |---|---|---|---|
//! | INFO 0x01 | IN | — | [`yoton_proto::devchan::Info`] (32 bytes) |
//! | LOG 0x02 | IN | offset (wIndex:wValue) into the log-ring area | raw ring bytes |
//! | FRAME 0x03 | IN | byte offset (wIndex:wValue) into the frame buffer | RGB565 bytes |
//! | KEYS 0x04 | OUT | — | `[key, down]` pairs |
//! | STATS 0x05 | IN | group | counters (u32 LE) |
//! | REBOOT 0x07 | OUT | target | — |
//! | PEEK 0x0A | IN | address (wIndex:wValue) | memory bytes (development builds) |

use embassy_usb::Handler;
use embassy_usb::control::{InResponse, OutResponse, Recipient, Request, RequestType};
use yoton_proto::devchan::{
    self as dc, Info, REQ_FRAME, REQ_INFO, REQ_KEYS, REQ_LOG, REQ_PEEK, REQ_REBOOT, REQ_STATS,
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
}
