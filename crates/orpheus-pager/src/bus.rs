//! Sharing SPI0 (the NOR) between thread-mode reads and the pager
//! (2026-11-26, the boot-time settings read error).
//!
//! The pager's MemManage handler reads a line through SPI0 whenever paged
//! code faults, wherever it runs, and the RT executor's task bodies (level
//! 2) are paged by design and preempt thread mode. A thread-mode NOR read
//! cut by such a fault finds the controller reprogrammed under it: the
//! line read toggles chip select (the chip drops the thread's command),
//! rewrites the byte count and leaves the controller idle. The thread's
//! data phase then waits for bytes that never come (a read error), or, cut
//! between the command and the data phase, clocks out bytes with no
//! command (garbage read as data, no error at all). An RT task that changes
//! the clock level switches SPI0's clock under it as well.
//!
//! The fix is here: every thread-mode read is a sequence of transactions
//! of at most [`CHUNK`] bytes, each run with levels 1–3 masked
//! ([`Bus::masked`], BASEPRI). Nothing that preempts thread mode runs
//! then; a fault can only come from the reader itself, whose code is
//! resident and whose buffer is RAM. [`Bus::epoch`] counts the other SPI0
//! users' turns (the pager's line reads, clock switches): a transaction
//! during which it moved is reported as [`ReadFault::Interleaved`]
//! instead of returning bytes that may be wrong.
//!
//! The device implements [`Bus`] on the registers (`atj2157_hal::nor`);
//! the tests below drive [`read`] against a model of the controller that
//! takes the pager's line read at every step.

/// Largest thread-mode transaction, bytes. It bounds the time interrupts
/// stay masked to about what one pager line read stalls everything for
/// (a byte-polled 256 B READ against a 1 KiB DMA FAST READ, ~175 µs).
pub const CHUNK: usize = 256;

/// Why a thread-mode NOR read failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadFault {
    /// The command phase never finished (TX FIFO not drained or the
    /// controller stayed busy).
    Command = 0,
    /// The data phase timed out (the RX FIFO stayed empty).
    Data = 1,
    /// Another SPI0 user ran during the transaction: the bytes are not
    /// trusted.
    Interleaved = 2,
}

/// Number of [`ReadFault`] kinds (counters indexed by `kind as usize`).
pub const KINDS: usize = 3;

/// SPI0 as a thread-mode reader sees it.
pub trait Bus {
    /// Run `f` with every interrupt that can preempt thread mode masked.
    fn masked<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R;
    /// Turns other SPI0 users have taken so far (wrapping).
    fn epoch(&self) -> u32;
    /// One READ transaction of `dst.len()` (≤ [`CHUNK`]) bytes at `addr`,
    /// chip select released at the end, failed or not.
    fn transaction(&mut self, addr: u32, dst: &mut [u8]) -> Result<(), ReadFault>;
}

/// Read `dst.len()` bytes at `addr` from thread mode: masked transactions
/// of at most [`CHUNK`] bytes; the first failure ends the read.
#[inline]
pub fn read<B: Bus>(bus: &mut B, addr: u32, dst: &mut [u8]) -> Result<(), ReadFault> {
    let mut a = addr;
    for c in dst.chunks_mut(CHUNK) {
        bus.masked(|b| {
            let e = b.epoch();
            let r = b.transaction(a, c);
            if b.epoch() != e { Err(ReadFault::Interleaved) } else { r }
        })?;
        a = a.wrapping_add(c.len() as u32);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::vec::Vec;

    use super::*;

    /// What the chip does with the bytes clocked out of it.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Chip {
        /// Deselected, or selected without a command: bytes read as 0xFF.
        NoCommand,
        /// A READ in progress at this address.
        Reading(u32),
    }

    /// The controller and the chip as `atj2157_hal::nor` drives them, and
    /// the pager's line read (`nor::read_dma` from the MemManage handler)
    /// arriving after register step `preempt_at` of the thread's read.
    struct Sim {
        nor: Vec<u8>,
        /// Chip select asserted.
        cs: bool,
        chip: Chip,
        /// The controller is in its receive mode with a byte count set.
        rx: bool,
        masked: bool,
        epoch: u32,
        steps: u32,
        preempt_at: Option<u32>,
        /// The fault comes from the reader itself (not maskable).
        unmaskable: bool,
        pending: bool,
        line_reads: u32,
        transactions: Vec<(u32, usize, bool)>,
    }

    impl Sim {
        fn new(preempt_at: Option<u32>) -> Self {
            Self {
                nor: (0..0x4000u32).map(|i| (i.wrapping_mul(7) ^ (i >> 8)) as u8).collect(),
                cs: false,
                chip: Chip::NoCommand,
                rx: false,
                masked: false,
                epoch: 0,
                steps: 0,
                preempt_at,
                unmaskable: false,
                pending: false,
                line_reads: 0,
                transactions: Vec::new(),
            }
        }

        /// One register step of the thread; the interrupt may arrive after it.
        fn step(&mut self) {
            self.steps += 1;
            if self.preempt_at == Some(self.steps) {
                if self.masked && !self.unmaskable {
                    self.pending = true;
                } else {
                    self.line_read();
                }
            }
        }

        /// The pager's line read: CS high then low (the chip drops what it
        /// was doing), FAST READ of 1 KiB, CS high, controller idle.
        fn line_read(&mut self) {
            let at = 0x2000u32;
            self.cs = true;
            self.chip = Chip::Reading(at);
            let mut line = [0u8; 1024];
            for b in &mut line {
                *b = self.byte();
            }
            assert_eq!(&line[..], &self.nor[at as usize..at as usize + 1024], "the pager's line");
            self.cs = false;
            self.chip = Chip::NoCommand;
            self.rx = false;
            self.epoch = self.epoch.wrapping_add(1);
            self.line_reads += 1;
        }

        fn byte(&mut self) -> u8 {
            match self.chip {
                Chip::Reading(a) if self.cs => {
                    self.chip = Chip::Reading(a + 1);
                    self.nor[a as usize]
                }
                _ => 0xFF,
            }
        }
    }

    impl Bus for Sim {
        fn masked<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
            let was = self.masked;
            self.masked = true;
            let r = f(self);
            self.masked = was;
            if !was && self.pending {
                // Taken as soon as BASEPRI drops.
                self.pending = false;
                self.line_read();
            }
            r
        }
        fn epoch(&self) -> u32 {
            self.epoch
        }
        fn transaction(&mut self, addr: u32, dst: &mut [u8]) -> Result<(), ReadFault> {
            self.transactions.push((addr, dst.len(), self.masked));
            // `send`: CS low, the four command bytes.
            self.cs = true;
            self.chip = Chip::NoCommand;
            self.step();
            // Command bytes with chip select released go nowhere.
            self.chip = if self.cs { Chip::Reading(addr) } else { Chip::NoCommand };
            self.step();
            // `recv`: byte count, receive mode, one byte per RX poll.
            self.rx = true;
            self.step();
            let mut r = Ok(());
            for d in dst.iter_mut() {
                if !self.rx {
                    // The RX FIFO stays empty: `wait(4, 0)` times out.
                    r = Err(ReadFault::Data);
                    break;
                }
                *d = self.byte();
                self.step();
            }
            // `cs_high`.
            self.cs = false;
            self.rx = false;
            self.chip = Chip::NoCommand;
            r
        }
    }

    /// Regression (2026-11-26): `settings: store mounted after 1 read
    /// errors` at boot (and once `store=none`, the player on defaults). The
    /// settings store's thread-mode NOR read ran unmasked while the RT
    /// executor mounted the card; a page fault there ran the pager's line
    /// read on SPI0 in the middle of it, which timed out the data phase or,
    /// between command and data, handed back bytes without a command. Now
    /// the pager's read waits for the end of the transaction wherever it
    /// arrives, and both reads return the right bytes.
    #[test]
    fn regress_store_read_cut_by_a_page_fault() {
        let len = 300; // two transactions
        let total = {
            let mut s = Sim::new(None);
            let mut b = std::vec![0u8; len];
            read(&mut s, 0x100, &mut b).unwrap();
            s.steps
        };
        for k in 1..=total {
            let mut s = Sim::new(Some(k));
            let mut b = std::vec![0u8; len];
            assert_eq!(read(&mut s, 0x100, &mut b), Ok(()), "fault after step {k}");
            assert_eq!(&b[..], &s.nor[0x100..0x100 + len], "fault after step {k}");
            assert_eq!(s.line_reads, 1, "the pager's read was served (step {k})");
        }
    }

    /// A transaction another SPI0 user cut into (a fault no mask holds
    /// off) is an error, never bytes that may be wrong.
    #[test]
    fn an_unmaskable_cut_is_reported_not_returned() {
        let len = 40;
        for k in 1..=(len as u32 + 3) {
            let mut s = Sim::new(Some(k));
            s.unmaskable = true;
            let mut b = std::vec![0u8; len];
            assert_eq!(read(&mut s, 0x80, &mut b), Err(ReadFault::Interleaved), "step {k}");
        }
    }

    /// Transactions cover the buffer in order, each at most `CHUNK` bytes
    /// and each masked; an empty read sends nothing.
    #[test]
    fn transactions_are_masked_chunks_in_order() {
        let mut s = Sim::new(None);
        let mut b = std::vec![0u8; 2 * CHUNK + 3];
        read(&mut s, 0x1F0, &mut b).unwrap();
        assert_eq!(s.transactions, [(0x1F0, CHUNK, true), (0x2F0, CHUNK, true), (0x3F0, 3, true)]);
        assert_eq!(&b[..], &s.nor[0x1F0..0x1F0 + b.len()]);
        let mut s = Sim::new(None);
        read(&mut s, 0, &mut []).unwrap();
        assert!(s.transactions.is_empty());
    }

    /// The first failed transaction ends the read; the kinds index the
    /// firmware's counters.
    #[test]
    fn a_failed_transaction_ends_the_read() {
        struct Fails(u32);
        impl Bus for Fails {
            fn masked<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
                f(self)
            }
            fn epoch(&self) -> u32 {
                0
            }
            fn transaction(&mut self, _: u32, _: &mut [u8]) -> Result<(), ReadFault> {
                self.0 += 1;
                Err(ReadFault::Command)
            }
        }
        let mut f = Fails(0);
        assert_eq!(read(&mut f, 0, &mut [0u8; 600]), Err(ReadFault::Command));
        assert_eq!(f.0, 1);
        let kinds = [ReadFault::Command, ReadFault::Data, ReadFault::Interleaved];
        assert!(kinds.iter().enumerate().all(|(i, &k)| k as usize == i && i < KINDS));
    }
}
