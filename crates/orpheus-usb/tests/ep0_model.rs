//! USB-W1: a model of the ATJ2157 EP0 path under embassy-usb's real
//! control state machine (06-connectivity §2.6).
//!
//! The model keeps exactly the hardware state the HAL driver
//! (`fw/atj2157-hal/src/usb.rs`) relies on — a latched SETUP in the CX FIFO,
//! the level-triggered SETUP interrupt and its mask, a DMA that finishes
//! when the test says so — in two flavours: the driver as it was when the
//! soak wedged (`Before`) and as fixed (`After`: the SETUP read unmasks the
//! interrupt however it ends, a bus reset re-arms EP0, a data or status
//! stage overtaken by a newer SETUP is given up). Everything above it is
//! embassy-usb 0.6 itself, so the tests show what embassy does to the
//! driver: it drops the `setup()` future whenever a bus event wins its
//! `select`.

use std::cell::RefCell;
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use embassy_usb::{Builder, Config};
use embassy_usb_driver::{
    Direction, Endpoint, EndpointAddress, EndpointAllocError, EndpointError, EndpointIn, EndpointInfo,
    EndpointOut, EndpointType, Event, Unsupported,
};
use orpheus_proto::devchan::{self as dc, Info, REQ_INFO, REQ_TEST};
use orpheus_usb::devchan::{DevChannel, Target};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Flavour {
    Before,
    After,
}

/// What the device put on the bus, tagged with the host transfer it was
/// meant for and the one the host was running.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Out {
    Data { meant: u32, host: u32, len: usize },
    Done { meant: u32, host: u32 },
    Stall { meant: u32, host: u32 },
}

impl Out {
    fn misdirected(&self) -> bool {
        match *self {
            Out::Data { meant, host, .. } | Out::Done { meant, host } | Out::Stall { meant, host } => {
                meant != host
            }
        }
    }
}

#[derive(Default)]
struct Hw {
    flavour: Option<Flavour>,
    /// The SETUP in the CX FIFO and the host transfer number it starts.
    latched: Option<([u8; 8], u32)>,
    /// Host transfers started (SETUPs sent).
    host: u32,
    /// GIMR0 bit 0.
    masked: bool,
    /// SETUP_READY.
    ready: bool,
    /// The running DMA may finish (tests hold it to land a bus event
    /// mid-read).
    dma_go: bool,
    /// Host OUT data waiting in the FIFO.
    out_data: Option<Vec<u8>>,
    events: Vec<Event>,
    out: Vec<Out>,
    /// The transfer the device is serving.
    serving: u32,
}

impl Hw {
    fn after(&self) -> bool {
        self.flavour == Some(Flavour::After)
    }
    /// The level-triggered SETUP interrupt, as `on_irq` handles it.
    fn isr(&mut self) {
        if self.latched.is_some() && !self.masked {
            self.masked = true;
            self.ready = true;
        }
    }
    /// The host sends a SETUP (a newer one replaces a latched one).
    fn host_setup(&mut self, raw: [u8; 8]) {
        self.host += 1;
        self.latched = Some((raw, self.host));
        self.out_data = None;
        self.isr();
    }
    fn bus_reset(&mut self) {
        if self.after() {
            // `hw_bus_reset` (USB-W1): EP0 re-armed.
            self.ready = false;
            self.masked = false;
        }
        self.events.push(Event::Reset);
        self.isr();
    }
    fn overtaken(&self) -> bool {
        self.after() && self.ready
    }
}

type Shared = Rc<RefCell<Hw>>;

struct Driver(Shared);
struct Bus(Shared);
struct Pipe(Shared);
struct NoEp(EndpointInfo);

impl<'a> embassy_usb_driver::Driver<'a> for Driver {
    type EndpointOut = NoEp;
    type EndpointIn = NoEp;
    type ControlPipe = Pipe;
    type Bus = Bus;
    fn alloc_endpoint_out(
        &mut self,
        _t: EndpointType,
        _a: Option<EndpointAddress>,
        _m: u16,
        _i: u8,
    ) -> Result<NoEp, EndpointAllocError> {
        Err(EndpointAllocError)
    }
    fn alloc_endpoint_in(
        &mut self,
        _t: EndpointType,
        _a: Option<EndpointAddress>,
        _m: u16,
        _i: u8,
    ) -> Result<NoEp, EndpointAllocError> {
        Err(EndpointAllocError)
    }
    fn start(self, _mps: u16) -> (Bus, Pipe) {
        (Bus(self.0.clone()), Pipe(self.0))
    }
}

impl Endpoint for NoEp {
    fn info(&self) -> &EndpointInfo {
        &self.0
    }
    async fn wait_enabled(&mut self) {}
}
impl EndpointIn for NoEp {
    async fn write(&mut self, _b: &[u8]) -> Result<(), EndpointError> {
        Err(EndpointError::Disabled)
    }
}
impl EndpointOut for NoEp {
    async fn read(&mut self, _b: &mut [u8]) -> Result<usize, EndpointError> {
        Err(EndpointError::Disabled)
    }
}

impl embassy_usb_driver::Bus for Bus {
    async fn enable(&mut self) {}
    async fn disable(&mut self) {}
    async fn poll(&mut self) -> Event {
        let hw = self.0.clone();
        poll_fn(move |_| match hw.borrow_mut().events.pop() {
            Some(e) => Poll::Ready(e),
            None => Poll::Pending,
        })
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

/// `SetupRead` of the fixed driver: unmask however the read ends.
struct Unmask(Shared);
impl Drop for Unmask {
    fn drop(&mut self) {
        let mut h = self.0.borrow_mut();
        h.masked = false;
        h.isr();
    }
}

impl Pipe {
    /// A DMA that finishes when the test lets it.
    async fn dma(&self) {
        let hw = self.0.clone();
        poll_fn(move |_| if hw.borrow().dma_go { Poll::Ready(()) } else { Poll::Pending }).await;
    }
}

impl embassy_usb_driver::ControlPipe for Pipe {
    fn max_packet_size(&self) -> usize {
        64
    }

    async fn setup(&mut self) -> [u8; 8] {
        loop {
            let hw = self.0.clone();
            poll_fn(move |_| if hw.borrow().ready { Poll::Ready(()) } else { Poll::Pending }).await;
            self.0.borrow_mut().ready = false;
            let guard = self.0.borrow().after().then(|| Unmask(self.0.clone()));
            self.dma().await;
            let got = self.0.borrow_mut().latched.take();
            match guard {
                Some(g) => drop(g),
                None => {
                    // Before USB-W1: unmasked only here, after the read.
                    let mut h = self.0.borrow_mut();
                    h.masked = false;
                    h.isr();
                }
            }
            if let Some((raw, n)) = got {
                self.0.borrow_mut().serving = n;
                return raw;
            }
        }
    }

    async fn data_out(&mut self, buf: &mut [u8], _f: bool, _l: bool) -> Result<usize, EndpointError> {
        if self.0.borrow().overtaken() {
            return Err(EndpointError::Disabled);
        }
        // The OUT DMA runs until the CX FIFO holds something.
        let hw = self.0.clone();
        poll_fn(move |_| {
            let h = hw.borrow();
            if h.dma_go && (h.out_data.is_some() || h.latched.is_some()) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        let mut h = self.0.borrow_mut();
        // The OUT DMA drains the CX FIFO: the host's data, or the newer
        // SETUP when the host gave up and moved on.
        let bytes = match h.out_data.take() {
            Some(d) => d,
            None => match h.latched {
                Some((raw, _)) if !h.after() => {
                    h.latched = None;
                    raw.to_vec()
                }
                _ => return Err(EndpointError::Disabled),
            },
        };
        if h.overtaken() {
            return Err(EndpointError::Disabled);
        }
        let n = bytes.len().min(buf.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(n)
    }

    async fn data_in(&mut self, data: &[u8], _f: bool, last: bool) -> Result<(), EndpointError> {
        let mut h = self.0.borrow_mut();
        if h.overtaken() {
            return Err(EndpointError::Disabled);
        }
        let (meant, host) = (h.serving, h.host);
        h.out.push(Out::Data { meant, host, len: data.len() });
        if last {
            h.out.push(Out::Done { meant, host });
        }
        Ok(())
    }

    async fn accept(&mut self) {
        let mut h = self.0.borrow_mut();
        if h.overtaken() {
            return;
        }
        let (meant, host) = (h.serving, h.host);
        h.out.push(Out::Done { meant, host });
    }

    async fn reject(&mut self) {
        let mut h = self.0.borrow_mut();
        if h.overtaken() {
            return;
        }
        let (meant, host) = (h.serving, h.host);
        h.out.push(Out::Stall { meant, host });
    }

    async fn accept_set_address(&mut self, _a: u8) {
        self.accept().await;
    }
}

struct Fw;
impl Target for Fw {
    fn info(&mut self) -> Info {
        Info { version: dc::VERSION, ..Info::default() }
    }
    fn read_mem(&mut self, _a: u32, b: &mut [u8]) -> bool {
        b.fill(0);
        true
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
        true
    }
    fn test_command(&mut self, _d: &[u8]) -> bool {
        true
    }
}

fn vendor(dir_in: bool, request: u8, length: u16) -> [u8; 8] {
    let l = length.to_le_bytes();
    [0x40 | if dir_in { 0x80 } else { 0 }, request, 0, 0, 0, 0, l[0], l[1]]
}

/// Run embassy-usb's device over the model; `script` acts between polls.
fn run(flavour: Flavour, script: impl FnOnce(&Shared, &mut dyn FnMut())) -> Vec<Out> {
    let hw: Shared = Rc::new(RefCell::new(Hw { flavour: Some(flavour), dma_go: true, ..Hw::default() }));
    let mut cfg_desc = [0u8; 256];
    let mut bos = [0u8; 256];
    let mut msos = [0u8; 256];
    let mut ctl = [0u8; 1024];
    let mut chan = DevChannel::new(Fw);
    let mut b = Builder::new(
        Driver(hw.clone()),
        Config::new(dc::DEV_VID, dc::DEV_PID),
        &mut cfg_desc,
        &mut bos,
        &mut msos,
        &mut ctl,
    );
    b.handler(&mut chan);
    let mut dev = b.build();
    let mut fut = pin!(dev.run());
    let mut cx = Context::from_waker(Waker::noop());
    let mut step = || {
        for _ in 0..8 {
            assert!(fut.as_mut().poll(&mut cx).is_pending());
        }
    };
    hw.borrow_mut().events.push(Event::PowerDetected);
    step();
    hw.borrow_mut().bus_reset();
    step();
    script(&hw, &mut step);
    hw.borrow().out.clone()
}

/// The host asks INFO; true when the device answered it.
fn info_answered(hw: &Shared, step: &mut dyn FnMut()) -> bool {
    let n = hw.borrow().out.len();
    hw.borrow_mut().host_setup(vendor(true, REQ_INFO, 32));
    step();
    let h = hw.borrow();
    let host = h.host;
    h.out[n..].iter().any(|o| matches!(o, Out::Done { meant, host: hh } if *meant == host && *hh == host))
}

/// A bus event (suspend, then resume) while the SETUP read's DMA runs.
fn event_during_setup_read(hw: &Shared, step: &mut dyn FnMut()) {
    hw.borrow_mut().dma_go = false;
    hw.borrow_mut().host_setup(vendor(true, REQ_INFO, 32));
    step();
    hw.borrow_mut().events.push(Event::Suspend);
    step();
    hw.borrow_mut().events.push(Event::Resume);
    step();
    hw.borrow_mut().dma_go = true;
    step();
}

#[test]
fn both_flavours_answer_normally() {
    for f in [Flavour::Before, Flavour::After] {
        run(f, |hw, step| {
            for _ in 0..5 {
                assert!(info_answered(hw, step), "{f:?}");
            }
        });
    }
}

#[test]
fn before_w1_a_bus_event_during_the_setup_read_deafens_ep0_for_good() {
    let out = run(Flavour::Before, |hw, step| {
        assert!(info_answered(hw, step));
        event_during_setup_read(hw, step);
        // The read future was dropped with the interrupt masked: SETUP
        // packets are latched, never read; a bus reset (re-plug) does not
        // help.
        assert!(hw.borrow().masked);
        assert!(!info_answered(hw, step));
        hw.borrow_mut().bus_reset();
        step();
        assert!(!info_answered(hw, step));
        assert!(!info_answered(hw, step));
    });
    assert_eq!(out.iter().filter(|o| matches!(o, Out::Done { .. })).count(), 1);
}

/// Regression: the USB stack went deaf until a restart (bug USB-W1,
/// 2026-11-24, found by the first soak: "error -71" on set-address even
/// after the cable went in again). A bus event during the SETUP read left
/// its interrupt masked for good (the `Before` flavour:
/// `before_w1_a_bus_event_during_the_setup_read_deafens_ep0_for_good`).
#[test]
fn regress_usb_w1_setup_read_is_drop_safe() {
    run(Flavour::After, |hw, step| {
        assert!(info_answered(hw, step));
        event_during_setup_read(hw, step);
        assert!(!hw.borrow().masked);
        assert!(info_answered(hw, step));
        assert!(info_answered(hw, step));
    });
}

/// The host gives up on a TEST OUT the device is slow with (its data
/// stage not read yet) and sends the next request, an IN.
fn cancelled_out_then_in(hw: &Shared, step: &mut dyn FnMut()) {
    // The device reads the SETUP; the data stage has not reached it yet.
    hw.borrow_mut().host_setup(vendor(false, REQ_TEST, 6));
    step();
    // Host timeout: the next SETUP replaces the transfer.
    hw.borrow_mut().host_setup(vendor(true, REQ_INFO, 32));
    step();
}

#[test]
fn before_w1_a_late_status_lands_on_the_next_transfer() {
    let out = run(Flavour::Before, |hw, step| {
        assert!(info_answered(hw, step));
        cancelled_out_then_in(hw, step);
    });
    // The old OUT DMA swallowed the INFO SETUP as its data and finished
    // the old transfer while the host ran the new one: the host sees a
    // status stage where it wants data (the soak's PEEK "Stall").
    assert!(out.iter().any(Out::misdirected), "{out:?}");
}

/// Regression: USB-W1 (2026-11-24): the soak's PEEK stalled after a
/// cancelled TEST OUT, because the old transfer's status stage landed on
/// the next one (`before_w1_a_late_status_lands_on_the_next_transfer`).
#[test]
fn regress_usb_w1_overtaken_transfer_is_given_up() {
    let out = run(Flavour::After, |hw, step| {
        assert!(info_answered(hw, step));
        cancelled_out_then_in(hw, step);
        assert!(info_answered(hw, step));
    });
    assert!(!out.iter().any(Out::misdirected), "{out:?}");
    // The INFO that overtook the TEST was answered too.
    let host = 3;
    assert!(out.contains(&Out::Done { meant: host, host }), "{out:?}");
}

/// Regression: USB-W1 (2026-11-24): a bus reset (the cable out and in
/// again) did not bring a deaf EP0 back.
#[test]
fn regress_usb_w1_bus_reset_re_arms_ep0() {
    run(Flavour::After, |hw, step| {
        // Whatever left the interrupt masked, a reset clears it.
        hw.borrow_mut().masked = true;
        assert!(!info_answered(hw, step));
        hw.borrow_mut().bus_reset();
        step();
        assert!(info_answered(hw, step));
    });
}

#[test]
fn endpoints_are_unused() {
    let e = NoEp(EndpointInfo {
        addr: EndpointAddress::from_parts(1, Direction::In),
        ep_type: EndpointType::Bulk,
        max_packet_size: 64,
        interval_ms: 0,
    });
    assert_eq!(e.info().max_packet_size, 64);
}
