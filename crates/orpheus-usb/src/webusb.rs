//! WebUSB and Microsoft OS 2.0 descriptors for the developer-channel
//! interface (WEB-3, 09 §5): a Chromium browser may open the player from
//! the Install page without a driver, Windows binds WinUSB to the interface
//! by itself, and Chrome offers the landing page when the player is
//! plugged in.
//!
//! * BOS: the WebUSB platform capability (vendor code
//!   [`dc::WEBUSB_VENDOR_CODE`], landing page 1) and — written by
//!   embassy-usb from the descriptor set — the MS OS 2.0 platform
//!   capability (vendor code [`dc::MSOS_VENDOR_CODE`]).
//! * MS OS 2.0 descriptor set: compatible id `WINUSB` and the
//!   `DeviceInterfaceGUIDs` registry value, at device level when the
//!   developer interface is the only one, else in a function subset.
//! * `GET_URL` (vendor IN, `bRequest` = vendor code, `wValue` = 1,
//!   `wIndex` = 2): answered by [`crate::devchan::DevChannel`] with
//!   [`url_descriptor`].
//!
//! The ROM's ADFU mode cannot carry any of this; the Install page explains
//! the one-time WinUSB step for it on Windows.

use embassy_usb::Builder;
use embassy_usb::descriptor::capability_type;
use embassy_usb::driver::Driver;
use embassy_usb::msos::{self, windows_version};
use orpheus_proto::devchan as dc;

/// `wIndex` of the WebUSB `GET_URL` request.
pub const GET_URL: u16 = 2;
/// WebUSB URL descriptor type.
pub const URL_DESCRIPTOR: u8 = 3;
/// Index of the landing page URL (`iLandingPage`).
pub const LANDING_INDEX: u16 = 1;

/// WebUSB platform capability UUID `3408b638-09a9-47a0-8bfd-a0768815b665`
/// in the byte order of the descriptor.
pub const WEBUSB_UUID: [u8; 16] =
    [0x38, 0xb6, 0x08, 0x34, 0xa9, 0x09, 0xa0, 0x47, 0x8b, 0xfd, 0xa0, 0x76, 0x88, 0x15, 0xb6, 0x65];
/// MS OS 2.0 platform capability UUID `d8dd60df-4589-4cc7-9cd2-659d9e648a9f`.
pub const MSOS_UUID: [u8; 16] =
    [0xdf, 0x60, 0xdd, 0xd8, 0x89, 0x45, 0xc7, 0x4c, 0x9c, 0xd2, 0x65, 0x9d, 0x9e, 0x64, 0x8a, 0x9f];

/// The WebUSB platform capability's body (after `bDevCapabilityType`):
/// reserved, UUID, `bcdVersion` 1.0, vendor code, landing page index.
#[must_use]
pub fn platform_capability() -> [u8; 21] {
    let mut c = [0u8; 21];
    c[1..17].copy_from_slice(&WEBUSB_UUID);
    c[17] = 0x00;
    c[18] = 0x01;
    c[19] = dc::WEBUSB_VENDOR_CODE;
    c[20] = LANDING_INDEX as u8;
    c
}

/// The landing page as a WebUSB URL descriptor into `buf` (≥ 3 + URL
/// bytes); returns its length, 0 if `buf` is too small.
pub fn url_descriptor(buf: &mut [u8]) -> usize {
    let (scheme, rest) = match dc::LANDING_URL.strip_prefix("https://") {
        Some(r) => (1u8, r),
        None => (0u8, dc::LANDING_URL.strip_prefix("http://").unwrap_or(dc::LANDING_URL)),
    };
    let n = 3 + rest.len();
    if buf.len() < n || n > 255 {
        return 0;
    }
    buf[0] = n as u8;
    buf[1] = URL_DESCRIPTOR;
    buf[2] = scheme;
    buf[3..n].copy_from_slice(rest.as_bytes());
    n
}

/// Add the developer-channel interface (vendor class; the channel is EP0
/// vendor requests) with its WebUSB capability and MS OS 2.0 descriptors.
/// `alone`: it is the device's only interface (device-level MS OS
/// features) and gets the bulk OUT endpoint of uploads
/// (`orpheus_proto::devchan::upload::BULK_CMD`, returned); otherwise it is
/// one function of a composite device, without endpoints. Call before any
/// other function is added.
pub fn dev_interface<'d, D: Driver<'d>>(b: &mut Builder<'d, D>, alone: bool) -> Option<D::EndpointOut> {
    b.msos_descriptor(windows_version::WIN8_1, dc::MSOS_VENDOR_CODE);
    if alone {
        b.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
        b.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
            "DeviceInterfaceGUIDs",
            msos::PropertyData::RegMultiSz(&[dc::DEVICE_INTERFACE_GUID]),
        ));
    }
    let mut func = b.function(0xFF, 0, 0);
    if !alone {
        func.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
        func.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
            "DeviceInterfaceGUIDs",
            msos::PropertyData::RegMultiSz(&[dc::DEVICE_INTERFACE_GUID]),
        ));
    }
    let mut iface = func.interface();
    let mut alt = iface.alt_setting(0xFF, 0, 0, None);
    alt.bos_capability(capability_type::PLATFORM, &platform_capability());
    alone.then(|| alt.endpoint_bulk_out(Some(dc::upload::BULK_EP.into()), 512))
}

#[cfg(test)]
mod tests {
    //! A fake controller drives embassy-usb's real control handling with
    //! the requests a browser and Windows send, so these check the bytes
    //! the player puts on the wire.

    use super::*;
    use crate::devchan::{DevChannel, Target};
    use core::future::Future;
    use core::pin::pin;
    use core::task::{Context, Poll};
    use embassy_usb::driver::{
        Bus, ControlPipe, Endpoint, EndpointAddress, EndpointAllocError, EndpointError, EndpointIn,
        EndpointInfo, EndpointOut, EndpointType, Event, Unsupported,
    };
    use orpheus_proto::devchan::Info;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::vec::Vec;

    #[derive(Default)]
    struct Wire {
        setups: Vec<[u8; 8]>,
        /// Data stages sent (one entry per request) and rejections.
        replies: Vec<Option<Vec<u8>>>,
        done: bool,
    }

    struct FakeDriver(Rc<RefCell<Wire>>);
    struct FakeBus(Rc<RefCell<Wire>>);
    struct FakePipe(Rc<RefCell<Wire>>);
    struct FakeEp(EndpointInfo);

    fn pending<T>() -> impl Future<Output = T> {
        core::future::pending()
    }

    impl Endpoint for FakeEp {
        fn info(&self) -> &EndpointInfo {
            &self.0
        }
        async fn wait_enabled(&mut self) {
            pending::<()>().await;
        }
    }
    impl EndpointIn for FakeEp {
        async fn write(&mut self, _buf: &[u8]) -> Result<(), EndpointError> {
            pending().await
        }
    }
    impl EndpointOut for FakeEp {
        async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, EndpointError> {
            pending().await
        }
    }

    impl<'a> Driver<'a> for FakeDriver {
        type EndpointOut = FakeEp;
        type EndpointIn = FakeEp;
        type ControlPipe = FakePipe;
        type Bus = FakeBus;
        fn alloc_endpoint_out(
            &mut self,
            ep_type: EndpointType,
            ep_addr: Option<EndpointAddress>,
            max_packet_size: u16,
            interval_ms: u8,
        ) -> Result<FakeEp, EndpointAllocError> {
            let addr = ep_addr.unwrap_or(EndpointAddress::from(1));
            Ok(FakeEp(EndpointInfo { addr, ep_type, max_packet_size, interval_ms }))
        }
        fn alloc_endpoint_in(
            &mut self,
            ep_type: EndpointType,
            ep_addr: Option<EndpointAddress>,
            max_packet_size: u16,
            interval_ms: u8,
        ) -> Result<FakeEp, EndpointAllocError> {
            let addr = ep_addr.unwrap_or(EndpointAddress::from(0x81));
            Ok(FakeEp(EndpointInfo { addr, ep_type, max_packet_size, interval_ms }))
        }
        fn start(self, _mps: u16) -> (FakeBus, FakePipe) {
            (FakeBus(self.0.clone()), FakePipe(self.0))
        }
    }

    impl Bus for FakeBus {
        async fn enable(&mut self) {}
        async fn disable(&mut self) {}
        async fn poll(&mut self) -> Event {
            // Suspend once every request was answered.
            let w = self.0.clone();
            core::future::poll_fn(
                |_| if w.borrow().done { Poll::Ready(Event::Suspend) } else { Poll::Pending },
            )
            .await
        }
        fn endpoint_set_enabled(&mut self, _a: EndpointAddress, _e: bool) {}
        fn endpoint_set_stalled(&mut self, _a: EndpointAddress, _s: bool) {}
        fn endpoint_is_stalled(&mut self, _a: EndpointAddress) -> bool {
            false
        }
        async fn remote_wakeup(&mut self) -> Result<(), Unsupported> {
            Err(Unsupported)
        }
    }

    impl ControlPipe for FakePipe {
        fn max_packet_size(&self) -> usize {
            64
        }
        async fn setup(&mut self) -> [u8; 8] {
            let next = {
                let mut w = self.0.borrow_mut();
                if w.setups.is_empty() {
                    w.done = true;
                    None
                } else {
                    w.replies.push(Some(Vec::new()));
                    Some(w.setups.remove(0))
                }
            };
            match next {
                Some(s) => s,
                None => pending().await,
            }
        }
        async fn data_out(&mut self, _buf: &mut [u8], _f: bool, _l: bool) -> Result<usize, EndpointError> {
            Ok(0)
        }
        async fn data_in(&mut self, data: &[u8], _f: bool, _l: bool) -> Result<(), EndpointError> {
            if let Some(Some(r)) = self.0.borrow_mut().replies.last_mut() {
                r.extend_from_slice(data);
            }
            Ok(())
        }
        async fn accept(&mut self) {}
        async fn reject(&mut self) {
            if let Some(r) = self.0.borrow_mut().replies.last_mut() {
                *r = None;
            }
        }
        async fn accept_set_address(&mut self, _addr: u8) {}
    }

    struct Quiet;
    impl Target for Quiet {
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
    }

    fn block_on<F: Future>(f: F) -> F::Output {
        let mut f = pin!(f);
        let mut cx = Context::from_waker(core::task::Waker::noop());
        for _ in 0..10_000 {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
        panic!("the device never finished");
    }

    fn setup(rt: u8, req: u8, value: u16, index: u16, len: u16) -> [u8; 8] {
        let mut s = [rt, req, 0, 0, 0, 0, 0, 0];
        s[2..4].copy_from_slice(&value.to_le_bytes());
        s[4..6].copy_from_slice(&index.to_le_bytes());
        s[6..8].copy_from_slice(&len.to_le_bytes());
        s
    }

    /// Build a device like the firmware's developer personality (`alone`)
    /// or keyboard personality (one more function after it), send
    /// `setups`, return the replies.
    fn ask(alone: bool, setups: &[[u8; 8]]) -> Vec<Option<Vec<u8>>> {
        let wire = Rc::new(RefCell::new(Wire { setups: setups.to_vec(), ..Wire::default() }));
        let mut cfg = [0u8; 256];
        let mut bos = [0u8; 256];
        let mut msos = [0u8; 256];
        let mut ctl = [0u8; 1024];
        let mut chan = DevChannel::new(Quiet);
        let mut config = embassy_usb::Config::new(dc::DEV_VID, dc::DEV_PID);
        config.max_packet_size_0 = 64;
        let mut b = Builder::new(FakeDriver(wire.clone()), config, &mut cfg, &mut bos, &mut msos, &mut ctl);
        b.handler(&mut chan);
        let _ = dev_interface(&mut b, alone);
        if !alone {
            let mut f = b.function(0x03, 1, 1);
            let mut i = f.interface();
            let _ = i.alt_setting(0x03, 1, 1, None);
        }
        let mut dev = b.build();
        block_on(dev.run_until_suspend());
        drop(dev);
        wire.borrow().replies.clone()
    }

    /// The BOS descriptor's platform capabilities: (UUID, body after it).
    fn platform_caps(bos: &[u8]) -> Vec<([u8; 16], Vec<u8>)> {
        assert_eq!(bos[1], 0x0F, "BOS type");
        assert_eq!(usize::from(u16::from_le_bytes([bos[2], bos[3]])), bos.len(), "wTotalLength");
        let mut out = Vec::new();
        let mut i = 5;
        while i < bos.len() {
            let len = usize::from(bos[i]);
            assert_eq!(bos[i + 1], 0x10, "device capability");
            if bos[i + 2] == capability_type::PLATFORM {
                let uuid: [u8; 16] = bos[i + 4..i + 20].try_into().unwrap();
                out.push((uuid, bos[i + 20..i + len].to_vec()));
            }
            i += len;
        }
        out
    }

    #[test]
    fn bos_url_and_msos_as_a_browser_and_windows_read_them() {
        for alone in [true, false] {
            let r = ask(
                alone,
                &[
                    setup(0x80, 6, 0x0F00, 0, 255),
                    setup(0xC0, dc::WEBUSB_VENDOR_CODE, LANDING_INDEX, GET_URL, 255),
                    setup(0xC0, dc::MSOS_VENDOR_CODE, 0, 7, 512),
                    setup(0xC0, dc::WEBUSB_VENDOR_CODE, 2, GET_URL, 255),
                    setup(0x80, 6, 0x0100, 0, 18),
                ],
            );
            let bos = r[0].as_ref().expect("BOS");
            let caps = platform_caps(bos);
            let web = caps.iter().find(|c| c.0 == WEBUSB_UUID).expect("WebUSB capability");
            assert_eq!(web.1, [0x00, 0x01, dc::WEBUSB_VENDOR_CODE, 1]);
            let ms = caps.iter().find(|c| c.0 == MSOS_UUID).expect("MS OS 2.0 capability");
            // Windows 8.1, set length, vendor code, no alternate enumeration.
            assert_eq!(&ms.1[..4], &windows_version::WIN8_1.to_le_bytes());
            let set_len = usize::from(u16::from_le_bytes([ms.1[4], ms.1[5]]));
            assert_eq!((ms.1[6], ms.1[7]), (dc::MSOS_VENDOR_CODE, 0));

            let url = r[1].as_ref().expect("landing page");
            assert_eq!((url[1], url[2]), (URL_DESCRIPTOR, 1));
            assert_eq!(usize::from(url[0]), url.len());
            assert_eq!(
                core::str::from_utf8(&url[3..]).unwrap(),
                dc::LANDING_URL.trim_start_matches("https://")
            );

            let set = r[2].as_ref().expect("MS OS 2.0 set");
            assert_eq!(set.len(), set_len);
            assert_eq!(u16::from_le_bytes([set[2], set[3]]), 0, "set header");
            assert_eq!(usize::from(u16::from_le_bytes([set[8], set[9]])), set.len());
            let has = |pat: &[u8]| set.windows(pat.len()).any(|w| w == pat);
            assert!(has(b"WINUSB\0\0"), "compatible id");
            let guid: Vec<u8> = dc::DEVICE_INTERFACE_GUID.encode_utf16().flat_map(u16::to_le_bytes).collect();
            assert!(has(&guid), "interface GUID");
            // Function subset (type 2) only on the composite device.
            let subset = set.windows(4).any(|w| w == [8, 0, 2, 0]);
            assert_eq!(subset, !alone);

            assert_eq!(r[3], None, "unknown URL index rejected");
            let dev = r[4].as_ref().unwrap();
            assert_eq!(u16::from_le_bytes([dev[2], dev[3]]), 0x0210, "bcdUSB 2.1 so hosts read the BOS");
        }
    }

    #[test]
    fn url_descriptor_needs_room() {
        let mut small = [0u8; 10];
        assert_eq!(url_descriptor(&mut small), 0);
        let mut b = [0u8; 255];
        let n = url_descriptor(&mut b);
        assert_eq!(n, 3 + dc::LANDING_URL.len() - "https://".len());
    }
}
