//! The USB transport every protocol in this crate runs over (WEB-3, 09 §5).
//!
//! One trait, three implementations: `nusb` on the desktop
//! ([`crate::native`]), WebUSB in the browser (`orpheus-install-web`, a
//! thin adapter around the page's `USBDevice`), and a simulated player in
//! the tests ([`crate::mock`]). The ADFU, ymon, NOR-payload and
//! developer-channel code is written once, `async`, against it; on the
//! desktop the futures complete at their first poll and [`block_on`] runs
//! them.

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use anyhow::Result;

/// A vendor control request to the device (recipient device).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Control {
    pub request: u8,
    pub value: u16,
    pub index: u16,
}

/// What a protocol needs from a USB device.
#[allow(async_fn_in_trait)]
pub trait Transport {
    /// Bulk OUT on the claimed interface (ADFU/ymon).
    async fn bulk_out(&mut self, data: &[u8], timeout_ms: u32) -> Result<()>;
    /// One bulk IN transfer of at most `len` bytes (a multiple of
    /// [`Transport::packet_size`]); shorter when the device ends it.
    async fn bulk_in(&mut self, len: usize, timeout_ms: u32) -> Result<Vec<u8>>;
    /// Clear a stall on the bulk IN endpoint.
    async fn clear_halt_in(&mut self) {}
    /// Bulk IN max packet size.
    fn packet_size(&self) -> usize;
    /// Vendor control IN.
    async fn control_in(&mut self, c: Control, len: u16, timeout_ms: u32) -> Result<Vec<u8>>;
    /// Vendor control OUT.
    async fn control_out(&mut self, c: Control, data: &[u8], timeout_ms: u32) -> Result<()>;
    async fn sleep_ms(&mut self, ms: u32);
    /// Milliseconds on a monotonic clock.
    fn now_ms(&self) -> u64;
}

/// Run a future whose I/O completes synchronously (the desktop transport,
/// the mock) to its end.
pub fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::yield_now();
    }
}

/// Progress of a long operation: what, done, total (bytes or sectors).
pub type Progress<'a> = &'a mut dyn FnMut(&str, u64, u64);

/// A progress sink that ignores everything.
pub fn quiet(_: &str, _: u64, _: u64) {}
