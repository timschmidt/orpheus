//! The persistent area at the top of SRAM0 (`0x137C00..0x138000`): fault
//! record, HIL result block and boot mailbox, laid out by `orpheus-proto`.
//! Volatile word accesses only; the linker script keeps every Rust object
//! out of this area.

use orpheus_proto::{fault, mailbox, result};

#[inline]
fn wr(addr: u32, v: u32) {
    // SAFETY: `addr` lies in the persistent area, which the linker script
    // reserves (no Rust object lives there); word-aligned by construction.
    unsafe { core::ptr::write_volatile(addr as *mut u32, v) }
}

#[inline]
fn rd(addr: u32) -> u32 {
    // SAFETY: as in `wr`.
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn wr_bytes(addr: u32, max: usize, s: &[u8]) {
    let mut word = [0u8; 4];
    for i in (0..max).step_by(4) {
        for (j, b) in word.iter_mut().enumerate() {
            *b = s.get(i + j).copied().unwrap_or(0);
        }
        wr(addr + i as u32, u32::from_le_bytes(word));
    }
}

/// The boot mailbox as the loader left it.
#[must_use]
pub fn mailbox() -> mailbox::Mailbox {
    let mut b = [0u8; mailbox::SIZE];
    for i in (0..mailbox::SIZE).step_by(4) {
        b[i..i + 4].copy_from_slice(&rd(mailbox::BASE + i as u32).to_le_bytes());
    }
    mailbox::Mailbox::from_bytes(&b).unwrap_or_default()
}

/// Whether a host test armed test mode, and which test.
#[must_use]
pub fn test_request() -> Option<(u32, mailbox::Mailbox)> {
    let m = mailbox();
    (m.test_mode == mailbox::TEST_MODE_MAGIC
        && mailbox::Request::from_word(m.request) == mailbox::Request::Test)
        .then_some((m.test_id, m))
}

/// Clear a test request left over from an earlier start
/// ([`mailbox::stale_test`]): call once at start, before
/// [`test_request`] is read.
pub fn forget_stale_test() -> bool {
    let tm = mailbox::ADDR_TEST_MODE;
    let stale = mailbox::stale_test(rd(tm), rd(mailbox::BASE + mailbox::OFF_BOOT_INFO as u32));
    if stale {
        let req = mailbox::BASE + mailbox::OFF_REQUEST as u32;
        if mailbox::Request::from_word(rd(req)) == mailbox::Request::Test {
            wr(req, 0);
        }
        wr(tm, 0);
    }
    stale
}

/// Mark this start as healthy (clears the boot-attempt counter, and with it
/// stage 2's record of tripped slots: the next start tries the first slot
/// again).
pub fn clear_boot_counter() {
    wr(mailbox::ADDR_BOOT_COUNT, 0);
}

/// Stage 2's record of this start (`None` when not started by stage 2).
#[must_use]
pub fn boot_info() -> Option<mailbox::BootInfo> {
    mailbox::BootInfo::from_word(rd(mailbox::BASE + mailbox::OFF_BOOT_INFO as u32))
}

/// `boot crash-test`: make the firmware in `slot` fail its next `n` starts
/// (a reboot without the healthy-start mark), so stage 2's fallback can be
/// tested without writing NOR ([`mailbox::crash_step`]).
pub fn arm_crash_test(slot: u8, n: u32) {
    wr(mailbox::BASE + mailbox::OFF_ARGS as u32, u32::from(slot));
    wr(mailbox::BASE + mailbox::OFF_ARGS as u32 + 4, n);
    wr(mailbox::BASE + mailbox::OFF_REQUEST as u32, mailbox::Request::CrashTest as u32);
}

/// At start-up: whether an armed crash test says this start must fail
/// (the count is used up here; the request is cleared once spent, or when
/// another slot started).
pub fn take_crash_test() -> bool {
    let m = mailbox();
    let req = mailbox::BASE + mailbox::OFF_REQUEST as u32;
    match mailbox::crash_step(m.request, &m.args, boot_info().and_then(mailbox::BootInfo::slot)) {
        mailbox::CrashStep::None => false,
        mailbox::CrashStep::Disarm => {
            wr(req, 0);
            false
        }
        mailbox::CrashStep::Fail { left } => {
            wr(mailbox::BASE + mailbox::OFF_ARGS as u32 + 4, left);
            if left == 0 {
                wr(req, 0);
            }
            true
        }
    }
}

/// Writer for the result block.
pub struct Results {
    n: usize,
    counters: [u32; result::N_COUNTERS],
    seq: u32,
    test_id: u32,
}

impl Results {
    /// Start a result block for `test_id` with verdict `Running`.
    #[must_use]
    pub fn begin(test_id: u32) -> Self {
        let seq = rd(result::BASE + result::OFF_SEQ as u32).wrapping_add(1);
        let r = Self { n: 0, counters: [0; result::N_COUNTERS], seq, test_id };
        r.flush(result::Verdict::Running, b"running");
        r
    }

    /// Set counter `i` (grows the used count).
    pub fn set(&mut self, i: usize, v: u32) {
        if let Some(c) = self.counters.get_mut(i) {
            *c = v;
            self.n = self.n.max(i + 1);
        }
    }

    #[must_use]
    pub fn get(&self, i: usize) -> u32 {
        self.counters.get(i).copied().unwrap_or(0)
    }

    /// Write everything with a verdict and message.
    pub fn flush(&self, verdict: result::Verdict, msg: &[u8]) {
        let b = result::BASE;
        wr(b + result::OFF_TEST_ID as u32, self.test_id);
        wr(b + result::OFF_NCOUNTERS as u32, self.n as u32);
        for (i, c) in self.counters.iter().enumerate() {
            wr(b + (result::OFF_COUNTERS + 4 * i) as u32, *c);
        }
        wr_bytes(b + result::OFF_MESSAGE as u32, result::MESSAGE_LEN, msg);
        wr(b + result::OFF_SEQ as u32, self.seq);
        wr(b + result::OFF_VERDICT as u32, verdict as u32);
        wr(b + result::OFF_MAGIC as u32, result::MAGIC);
    }
}

/// Fill in the fault record from a stacked exception frame.
/// `frame` is the address of the stacked exception frame.
pub fn record_fault(frame: usize, exc_return: u32, ipsr: u32) {
    let scb = |off: usize| {
        // SAFETY: SCB fault status registers.
        unsafe { core::ptr::read_volatile((0xE000_ED00 + off) as *const u32) }
    };
    let f = |i: usize| -> u32 {
        // Only dereference a frame pointer that lies in RAM.
        if (0x0010_0000..0x0013_7F00).contains(&frame) && frame.is_multiple_of(4) {
            // SAFETY: inside SRAM0 and word aligned, as exception frames are.
            unsafe { core::ptr::read_volatile((frame as *const u32).add(i)) }
        } else {
            0xDEAD_DEAD
        }
    };
    let mmfar = scb(0x34);
    let cfsr = scb(0x28);
    let (g0, g1) = crate::rt::stack_guard_range();
    let in_guard = |a: u32| a >= g0 && a < g1;
    // MSTKERR/stacking errors or an MMFAR inside the guard: stack overflow.
    let guard_hit = (cfsr & 0x80 != 0 && in_guard(mmfar)) || in_guard(frame as u32) || cfsr & 0x10 != 0;
    let kind = match ipsr & 0x1FF {
        _ if guard_hit => fault::Kind::StackGuard,
        // DebugMonitor: only the DWT stack watchpoint raises it (PG-2).
        12 => fault::Kind::StackGuard,
        3 => fault::Kind::HardFault,
        4 => fault::Kind::MemManage,
        5 => fault::Kind::BusFault,
        6 => fault::Kind::UsageFault,
        _ => fault::Kind::HardFault,
    };
    let b = fault::BASE;
    let count =
        if rd(b + fault::OFF_MAGIC as u32) == fault::MAGIC { rd(b + fault::OFF_COUNT as u32) } else { 0 };
    wr(b + fault::OFF_KIND as u32, kind as u32);
    wr(b + fault::OFF_PC as u32, f(6));
    wr(b + fault::OFF_LR as u32, f(5));
    wr(b + fault::OFF_XPSR as u32, f(7));
    wr(b + fault::OFF_SP as u32, frame as u32);
    wr(b + fault::OFF_CFSR as u32, scb(0x28));
    wr(b + fault::OFF_HFSR as u32, scb(0x2C));
    wr(b + fault::OFF_MMFAR as u32, scb(0x34));
    wr(b + fault::OFF_BFAR as u32, scb(0x38));
    wr(b + fault::OFF_IPSR as u32, ipsr);
    wr(b + fault::OFF_EXC_RETURN as u32, exc_return);
    for i in 0..5 {
        let r = if i < 4 { f(i) } else { f(4) };
        wr(b + (fault::OFF_REGS + 4 * i) as u32, r);
    }
    wr(b + fault::OFF_COUNT as u32, count.wrapping_add(1));
    wr(b + fault::OFF_MAGIC as u32, fault::MAGIC);
}

/// A panic's text written straight into the fault record (truncated at
/// [`fault::TEXT_LEN`]). BOOK-1/2 (2026-11-27): the panic handler's 96 B
/// line buffer was the bottom of the deepest stack paths (every panic
/// site under the card task's file opens and the codecs' set-up); this
/// writes in place.
pub struct PanicText {
    n: usize,
}

impl PanicText {
    /// The text cleared (zeros, as [`record_panic`] pads).
    #[must_use]
    pub fn start() -> Self {
        for i in (0..fault::TEXT_LEN).step_by(4) {
            wr(fault::BASE + (fault::OFF_TEXT + i) as u32, 0);
        }
        Self { n: 0 }
    }

    fn push_byte(&mut self, b: u8) {
        if self.n < fault::TEXT_LEN {
            let at = fault::BASE + (fault::OFF_TEXT + self.n) as u32;
            // SAFETY: inside the persistent area's text field (as `wr`).
            unsafe { core::ptr::write_volatile(at as *mut u8, b) };
            self.n += 1;
        }
    }

    pub fn push_str(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.push_byte(b);
        }
    }

    pub fn push_u32(&mut self, v: u32) {
        crate::log::decimal(v, |b| self.push_byte(b));
    }

    /// Kind `Panic`, `pc`, and the record made valid.
    pub fn finish(self, pc: u32) {
        let b = fault::BASE;
        wr(b + fault::OFF_KIND as u32, fault::Kind::Panic as u32);
        wr(b + fault::OFF_PC as u32, pc);
        wr(b + fault::OFF_MAGIC as u32, fault::MAGIC);
    }
}

/// Record a panic as a fault of kind `Panic` (message truncated).
pub fn record_panic(msg: &[u8], pc: u32) {
    let b = fault::BASE;
    wr(b + fault::OFF_KIND as u32, fault::Kind::Panic as u32);
    wr(b + fault::OFF_PC as u32, pc);
    wr_bytes(b + fault::OFF_TEXT as u32, fault::TEXT_LEN, msg);
    wr(b + fault::OFF_MAGIC as u32, fault::MAGIC);
}

/// An interrupt line fired with no handler bound.
pub fn note_unhandled_irq(n: u32) {
    let b = fault::BASE;
    if rd(b + fault::OFF_MAGIC as u32) == fault::MAGIC {
        return; // keep the first, more important record
    }
    wr(b + fault::OFF_KIND as u32, fault::Kind::UnhandledIrq as u32);
    wr(b + fault::OFF_IPSR as u32, n + 16);
    wr(b + fault::OFF_MAGIC as u32, fault::MAGIC);
}

/// Pager state at a fatal fault: address being served and loads so far.
pub fn note_pager(addr: u32, loads: u32) {
    wr(fault::BASE + fault::OFF_PAGER as u32, addr);
    wr(fault::BASE + fault::OFF_PAGER as u32 + 4, loads);
}
