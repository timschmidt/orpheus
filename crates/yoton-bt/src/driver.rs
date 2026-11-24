//! Glue between the [`Transmitter`] and the hardware, for the firmware's
//! Bluetooth task (WP BT-2). Still sans-I/O: the firmware supplies a
//! [`Port`] (UART receive ring, enable line) and sends the bytes of
//! [`Driver::tx_pending`] itself (by DMA), then calls [`Driver::tx_sent`].
//!
//! * Every transmitter call goes through the driver, which hands
//!   `Output::Enable` to the port at once (UART up before the enable line
//!   rises, so the module's ready event is not lost) and passes every
//!   output on to the caller.
//! * One frame at a time: the transmitter sends at most one frame per
//!   call (one command in flight), and the frame stays in the command
//!   layer's buffer until the next send ([`Transmitter::last_frame`]), so
//!   the TX side is a flag, not a copy; the firmware flushes after each
//!   call. A second send before the flush supersedes the first (counted
//!   in [`Stats::tx_dropped`]); the command layer's resend recovers it.
//! * Player power-off ([`Driver::begin_shutdown`]): the shutdown command
//!   (0x24) when the module is up, then at most [`SHUTDOWN_WAIT_MS`] for
//!   the module's completion event; the enable line drops either way.
//! * Diagnostics ([`Stats`]): bytes and frames both ways, power-ups, the
//!   last bytes received.

use crate::link::Transport;
use crate::transmitter::{Config, Output, State, Transmitter};

/// Longest wait for the module's shutdown to complete before the enable
/// line is dropped anyway (power-off path).
pub const SHUTDOWN_WAIT_MS: u64 = 1000;
/// Bytes of received history kept for diagnostics.
pub const RX_HISTORY: usize = 24;

/// The hardware side.
pub trait Port {
    /// `true`: UART and receive path up, then the module's enable line
    /// high. `false`: enable line low, UART down, pins parked.
    fn set_enabled(&mut self, on: bool);
    /// Bytes received since the last call, at most `buf.len()`.
    fn read(&mut self, buf: &mut [u8]) -> usize;
}

/// "A frame is waiting for the UART" (the bytes are
/// [`Transmitter::last_frame`]).
pub struct TxQueue {
    pending: bool,
    dropped: u32,
}

impl Transport for TxQueue {
    fn send(&mut self, _frame: &[u8]) {
        if self.pending {
            self.dropped += 1;
        }
        self.pending = true;
    }
}

/// Counters for the developer channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub rx_bytes: u32,
    pub tx_bytes: u32,
    pub tx_frames: u32,
    /// Frames superseded because the previous one was not flushed yet.
    pub tx_dropped: u32,
    /// Times the enable line went high.
    pub power_ups: u32,
    /// Shutdown commands sent / completed by the module's event.
    pub shutdowns: u32,
    pub shutdowns_completed: u32,
    /// The last bytes received, oldest first ([`Stats::last_rx`]).
    rx_hist: [u8; RX_HISTORY],
    rx_hist_pos: u8,
}

impl Stats {
    /// The last bytes received (up to [`RX_HISTORY`]), oldest first, as
    /// two slices.
    #[must_use]
    pub fn last_rx(&self) -> (&[u8], &[u8]) {
        let n = (self.rx_bytes as usize).min(RX_HISTORY);
        let pos = usize::from(self.rx_hist_pos).min(RX_HISTORY);
        if n < RX_HISTORY { (&self.rx_hist[..n], &[]) } else { (&self.rx_hist[pos..], &self.rx_hist[..pos]) }
    }

    fn record_rx(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.rx_hist[usize::from(self.rx_hist_pos) % RX_HISTORY] = b;
            self.rx_hist_pos = ((usize::from(self.rx_hist_pos) + 1) % RX_HISTORY) as u8;
        }
        self.rx_bytes = self.rx_bytes.wrapping_add(bytes.len() as u32);
    }
}

pub struct Driver {
    t: Transmitter,
    tx: TxQueue,
    stats: Stats,
    shutdown_until: Option<u64>,
    /// What the port was last told about the enable line.
    enabled: bool,
}

impl Driver {
    /// About 2 KB: build it in place in a static (`MaybeUninit::write` in
    /// a non-async function), not inside a future.
    #[must_use]
    pub fn new(cfg: Config) -> Self {
        Self {
            t: Transmitter::new(cfg),
            tx: TxQueue { pending: false, dropped: 0 },
            stats: Stats::default(),
            shutdown_until: None,
            enabled: false,
        }
    }

    #[must_use]
    pub fn transmitter(&self) -> &Transmitter {
        &self.t
    }

    #[must_use]
    pub fn stats(&self) -> Stats {
        let mut s = self.stats;
        s.tx_dropped = self.tx.dropped;
        s
    }

    #[must_use]
    pub fn state(&self) -> State {
        self.t.state()
    }

    /// Run a request (`start_inquiry`, `connect`, …) and hand the outputs
    /// on. Flush [`Self::tx_pending`] afterwards.
    pub fn request<R>(
        &mut self,
        now: u64,
        port: &mut impl Port,
        out: &mut impl FnMut(Output),
        f: impl FnOnce(&mut Transmitter, u64, &mut TxQueue) -> R,
    ) -> R {
        let r = f(&mut self.t, now, &mut self.tx);
        self.drain(port, out);
        r
    }

    /// Bluetooth on (start-up) or off (enable line low, no shutdown
    /// command).
    pub fn power(&mut self, on: bool, now: u64, port: &mut impl Port, out: &mut impl FnMut(Output)) {
        self.shutdown_until = None;
        self.request(now, port, out, |t, now, tx| {
            if on {
                t.power_on(now, tx);
            } else {
                t.power_off();
            }
        });
    }

    /// Received bytes, timeouts, retries, restarts, the shutdown deadline.
    /// Call every 10 ms while the module is on or starting.
    pub fn poll(&mut self, now: u64, port: &mut impl Port, out: &mut impl FnMut(Output)) {
        // Small chunks bound the outputs one call can produce.
        let mut buf = [0u8; 32];
        loop {
            let n = port.read(&mut buf).min(buf.len());
            if n == 0 {
                break;
            }
            self.stats.record_rx(&buf[..n]);
            let was_shutting = self.t.shutting_down();
            self.t.on_bytes(&buf[..n], now, &mut self.tx);
            if was_shutting && self.t.state() == State::Off {
                self.stats.shutdowns_completed += 1;
            }
            self.drain(port, out);
        }
        self.t.tick(now, &mut self.tx);
        if let Some(d) = self.shutdown_until
            && now >= d
            && self.t.state() != State::Off
        {
            self.t.power_off();
        }
        if self.t.state() == State::Off {
            self.shutdown_until = None;
        }
        self.drain(port, out);
    }

    /// The player's power-off: the shutdown command (0x24) if the module
    /// is up, else the enable line drops at once. Returns whether the
    /// command went out; then keep calling [`Self::poll`] (and flushing)
    /// until [`Self::is_off`], which takes at most [`SHUTDOWN_WAIT_MS`].
    pub fn begin_shutdown(&mut self, now: u64, port: &mut impl Port, out: &mut impl FnMut(Output)) -> bool {
        let sent = self.t.shutdown(now, &mut self.tx).is_ok();
        if sent {
            self.stats.shutdowns += 1;
            self.shutdown_until = Some(now + SHUTDOWN_WAIT_MS);
        } else {
            self.shutdown_until = None;
            self.t.power_off();
        }
        self.drain(port, out);
        sent
    }

    #[must_use]
    pub fn is_off(&self) -> bool {
        self.t.state() == State::Off
    }

    /// The frame to send now (empty if none). It stays put until the
    /// next call into the driver.
    #[must_use]
    pub fn tx_pending(&self) -> &[u8] {
        if self.tx.pending { self.t.last_frame() } else { &[] }
    }

    /// [`Self::tx_pending`] went out.
    pub fn tx_sent(&mut self) {
        if self.tx.pending {
            let n = self.t.last_frame().len() as u32;
            self.stats.tx_bytes = self.stats.tx_bytes.wrapping_add(n);
            self.stats.tx_frames += 1;
        }
        self.tx.pending = false;
    }

    fn drain(&mut self, port: &mut impl Port, out: &mut impl FnMut(Output)) {
        while let Some(o) = self.t.next_output() {
            if let Output::Enable(on) = o {
                self.set_enabled(on, port);
            }
            out(o);
        }
        // Should an `Enable` output ever be lost (output queue full), the
        // state machine's state still decides: the line is high exactly
        // while it is not Off.
        let want = self.t.state() != State::Off;
        if want != self.enabled {
            self.set_enabled(want, port);
        }
    }

    fn set_enabled(&mut self, on: bool, port: &mut impl Port) {
        // Nothing queued for the previous power cycle goes out.
        self.tx.pending = false;
        if on {
            self.stats.power_ups += 1;
        }
        self.enabled = on;
        port.set_enabled(on);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{Decoder, encode_unchecked, is_restricted};
    use crate::proto::Addr;
    use std::vec::Vec;

    const SPEAKER: Addr = Addr([0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6]);

    /// A module that answers like the one in the RAM dump (bluetooth.md
    /// §4.1, §4.2), behind a mock UART.
    #[derive(Default)]
    struct Module {
        powered: bool,
        /// Bytes on their way to the player.
        to_host: Vec<u8>,
        /// Commands received (IDs).
        seen: Vec<u8>,
        /// Answers nothing (a dead module or swapped lines).
        silent: bool,
        decoder: Decoder,
        enables: Vec<bool>,
    }

    impl Module {
        fn ev(&mut self, id: u8, p: &[u8]) {
            let mut b = [0u8; 64];
            let n = encode_unchecked(id, p, &mut b).unwrap();
            self.to_host.extend_from_slice(&b[..n]);
        }

        /// Bytes from the player's UART.
        fn receive(&mut self, bytes: &[u8]) {
            if !self.powered {
                return;
            }
            let mut frames = Vec::new();
            self.decoder.feed(bytes, |f| frames.push((f.id, f.params().to_vec())));
            for (id, p) in frames {
                self.seen.push(id);
                if self.silent {
                    continue;
                }
                self.ev(0x00, &[id]);
                match id {
                    0x06 if p == [2] => self.ev(0x06, b"ATS2851\0"),
                    0x2A => {
                        let mut r = std::vec![1, 0];
                        r.extend_from_slice(&SPEAKER.0);
                        r.extend_from_slice(b"Speaker\0");
                        self.ev(0x2A, &r);
                    }
                    0x00 => {
                        let mut r = SPEAKER.0.to_vec();
                        r.extend_from_slice(&[0xC5, 0x24, 0x04, 0x14]);
                        r.extend_from_slice(b"Speaker");
                        self.ev(0x03, &r);
                        self.ev(0x04, &[]);
                    }
                    0x03 => {
                        let mut r = std::vec![5, 0];
                        r.extend_from_slice(&p[..6]);
                        self.ev(0x02, &r);
                        r[0] = 0;
                        self.ev(0x02, &r);
                    }
                    0x24 => {
                        self.ev(0x29, &[0x01]);
                        self.ev(0x24, &[]);
                    }
                    _ => {}
                }
            }
        }
    }

    impl Port for Module {
        fn set_enabled(&mut self, on: bool) {
            self.enables.push(on);
            if on && !self.powered && !self.silent {
                // Stray byte at power-up, then ready (RAM dump).
                self.to_host.extend_from_slice(&[0x00, 0xAA, 0x00, 0x01, 0x23, 0xDC]);
            }
            if !on {
                self.to_host.clear();
                self.decoder.reset();
            }
            self.powered = on;
        }
        fn read(&mut self, buf: &mut [u8]) -> usize {
            // Small chunks, as a DMA ring polled every 10 ms would give.
            let n = self.to_host.len().min(buf.len()).min(7);
            buf[..n].copy_from_slice(&self.to_host[..n]);
            self.to_host.drain(..n);
            n
        }
    }

    /// The firmware task's loop body: poll, flush the frame to the module.
    fn run(d: &mut Driver, m: &mut Module, from: u64, to: u64, outs: &mut Vec<Output>) {
        let mut now = from;
        while now <= to {
            d.poll(now, m, &mut |o| outs.push(o));
            flush(d, m);
            now += 10;
        }
    }

    fn flush(d: &mut Driver, m: &mut Module) {
        let f = d.tx_pending().to_vec();
        if !f.is_empty() {
            m.receive(&f);
            d.tx_sent();
        }
    }

    #[test]
    fn startup_scan_connect_over_mock_uart() {
        let mut d = Driver::new(Config { auto_reconnect: false, ..Config::default() });
        let mut m = Module::default();
        let mut outs = Vec::new();
        d.power(true, 0, &mut m, &mut |o| outs.push(o));
        assert_eq!(m.enables, [true]);
        run(&mut d, &mut m, 0, 200, &mut outs);
        assert_eq!(d.state(), State::Idle);
        assert_eq!(d.transmitter().identity(), b"ATS2851\0");
        assert_eq!(d.transmitter().paired().len(), 1);
        assert_eq!(m.seen, [0x06, 0x30, 0x2A]);
        assert!(outs.contains(&Output::PairedList));
        // Inquiry.
        d.request(300, &mut m, &mut |o| outs.push(o), |t, now, tx| t.start_inquiry(now, tx)).unwrap();
        flush(&mut d, &mut m);
        run(&mut d, &mut m, 310, 400, &mut outs);
        assert_eq!(d.state(), State::Idle);
        assert_eq!(d.transmitter().found()[0].name(), b"Speaker");
        assert!(outs.contains(&Output::InquiryDone));
        // Connect.
        d.request(500, &mut m, &mut |o| outs.push(o), |t, now, tx| t.connect(SPEAKER, now, tx)).unwrap();
        flush(&mut d, &mut m);
        run(&mut d, &mut m, 510, 600, &mut outs);
        assert_eq!(d.state(), State::Connected);
        assert!(outs.contains(&Output::Connected(SPEAKER)));
        let s = d.stats();
        assert_eq!(s.tx_frames, 5);
        assert_eq!(s.power_ups, 1);
        assert_eq!(s.tx_dropped, 0);
        assert!(s.rx_bytes > 40);
        // Nothing restricted on any of these paths.
        assert!(m.seen.iter().all(|&id| !is_restricted(id)));
        // The history ends with the last frame received (link connected).
        let (a, b) = s.last_rx();
        let mut h = a.to_vec();
        h.extend_from_slice(b);
        assert_eq!(h.len(), RX_HISTORY);
        assert_eq!(&h[RX_HISTORY - 13..RX_HISTORY - 9], &[0xAA, 0x00, 0x09, 0x02]);
    }

    #[test]
    fn power_off_shutdown_completes() {
        let mut d = Driver::new(Config::default());
        let mut m = Module::default();
        let mut outs = Vec::new();
        d.power(true, 0, &mut m, &mut |o| outs.push(o));
        run(&mut d, &mut m, 0, 200, &mut outs);
        assert!(d.transmitter().module_up());
        assert!(d.begin_shutdown(1000, &mut m, &mut |o| outs.push(o)));
        flush(&mut d, &mut m);
        run(&mut d, &mut m, 1010, 1100, &mut outs);
        assert!(d.is_off());
        assert_eq!(*m.seen.last().unwrap(), 0x24);
        assert_eq!(m.enables, [true, false]);
        assert_eq!((d.stats().shutdowns, d.stats().shutdowns_completed), (1, 1));
        // Only one restricted command ever, and only on this path.
        assert_eq!(m.seen.iter().filter(|&&id| is_restricted(id)).count(), 1);
    }

    #[test]
    fn power_off_shutdown_bounded_when_module_hangs() {
        let mut d = Driver::new(Config::default());
        let mut m = Module::default();
        let mut outs = Vec::new();
        d.power(true, 0, &mut m, &mut |o| outs.push(o));
        run(&mut d, &mut m, 0, 200, &mut outs);
        // The module stops answering just before the power-off.
        m.silent = true;
        assert!(d.begin_shutdown(1000, &mut m, &mut |o| outs.push(o)));
        flush(&mut d, &mut m);
        run(&mut d, &mut m, 1010, 1000 + SHUTDOWN_WAIT_MS, &mut outs);
        assert!(d.is_off());
        assert_eq!(m.enables, [true, false]);
        assert_eq!(m.seen.iter().filter(|&&id| id == 0x24).count(), 1);
        assert_eq!(d.stats().shutdowns_completed, 0);
        // No restart afterwards.
        run(&mut d, &mut m, 2000, 10_000, &mut outs);
        assert_eq!(m.enables, [true, false]);
    }

    #[test]
    fn power_off_while_starting_drops_the_line_only() {
        let mut d = Driver::new(Config::default());
        let mut m = Module { silent: true, ..Module::default() };
        let mut outs = Vec::new();
        d.power(true, 0, &mut m, &mut |o| outs.push(o));
        run(&mut d, &mut m, 0, 100, &mut outs);
        assert_eq!(d.state(), State::Starting);
        assert!(!d.begin_shutdown(110, &mut m, &mut |o| outs.push(o)));
        assert!(d.is_off());
        assert!(d.tx_pending().is_empty());
        assert_eq!(m.enables, [true, false]);
        assert!(m.seen.is_empty());
    }

    #[test]
    fn dead_module_power_cycles_then_gives_up() {
        let mut d = Driver::new(Config::default());
        let mut m = Module { silent: true, ..Module::default() };
        let mut outs = Vec::new();
        d.power(true, 0, &mut m, &mut |o| outs.push(o));
        run(&mut d, &mut m, 0, 60_000, &mut outs);
        assert!(d.is_off());
        // First start plus `max_restarts` (2) restarts, each probing with
        // 06 00 and one resend.
        assert_eq!(m.enables, [true, false, true, false, true, false]);
        assert_eq!(m.seen, [0x06; 6]);
        assert!(outs.contains(&Output::LinkDown { restarting: false }));
        assert!(m.seen.iter().all(|&id| !is_restricted(id)));
    }

    /// UART framing over a noisy, oddly chunked line: garbage, a stray
    /// `AA`, a corrupted frame and byte-by-byte delivery do not stop the
    /// start-up exchange.
    #[test]
    fn startup_over_noisy_line() {
        struct Noisy(Module, usize);
        impl Port for Noisy {
            fn set_enabled(&mut self, on: bool) {
                self.0.set_enabled(on);
                if on {
                    // Line noise ahead of the module's own bytes.
                    let mut v = std::vec![0xFF, 0xAA, 0x00, 0x7F, 0x13, 0xAA, 0x00, 0x02, 0x00, 0x06, 0x00];
                    v.append(&mut self.0.to_host);
                    self.0.to_host = v;
                }
            }
            fn read(&mut self, buf: &mut [u8]) -> usize {
                // 1, 2, 3, 1, 2, 3 … bytes per poll.
                self.1 = self.1 % 3 + 1;
                let n = self.1.min(buf.len());
                self.0.read(&mut buf[..n])
            }
        }
        let mut d = Driver::new(Config { auto_reconnect: false, ..Config::default() });
        let mut p = Noisy(Module::default(), 0);
        let mut outs = Vec::new();
        d.power(true, 0, &mut p, &mut |o| outs.push(o));
        let mut now = 0;
        while now <= 2000 && d.state() != State::Idle {
            d.poll(now, &mut p, &mut |o| outs.push(o));
            let f = d.tx_pending().to_vec();
            if !f.is_empty() {
                p.0.receive(&f);
                d.tx_sent();
            }
            now += 10;
        }
        assert_eq!(d.state(), State::Idle);
        assert_eq!(d.transmitter().identity(), b"ATS2851\0");
        let ds = d.transmitter().decode_stats();
        assert!(ds.skipped > 0 && ds.bad_checksum + ds.bad_length > 0, "{ds:?}");
    }

    /// The enable line follows the state machine even if an `Enable`
    /// output were lost: the driver re-checks after every drain.
    #[test]
    fn enable_line_follows_state() {
        #[derive(Default)]
        struct Line(Vec<bool>);
        impl Port for Line {
            fn set_enabled(&mut self, on: bool) {
                self.0.push(on);
            }
            fn read(&mut self, _: &mut [u8]) -> usize {
                0
            }
        }
        let mut d = Driver::new(Config::default());
        let mut l = Line::default();
        d.power(true, 0, &mut l, &mut |_| {});
        // Swallow the transmitter's outputs behind the driver's back.
        while d.t.next_output().is_some() {}
        d.t.power_off();
        while d.t.next_output().is_some() {}
        d.poll(10, &mut l, &mut |_| {});
        assert_eq!(l.0, [true, false]);
        assert!(d.tx_pending().is_empty());
    }

    #[test]
    fn footprint() {
        let n = core::mem::size_of::<Driver>();
        std::println!("Driver: {n} bytes");
        assert!(n <= 2048, "{n}");
    }
}
