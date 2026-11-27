//! Watchdog and reboot (docs/plan/03-hal-drivers.md §3, 02 §8).
//!
//! `SYSRESETREQ` is ignored by this SoC [HW]; every reboot is a watchdog
//! expiry. The boot stage then reads the mailbox: `"ADFU"` at `0x137FF4`
//! sends it to the boot ROM's USB recovery.

use atj2157_pac::rtc::{self, wd};
use orpheus_proto::mailbox;

/// Feed the watchdog.
#[inline]
pub fn feed() {
    rtc::WD_CTL.set_bits(wd::CLR);
}

/// Arm with period code `code` (01-hardware §7; 5 = 5.6 s) in reset mode.
pub fn start(code: u32) {
    rtc::WD_CTL.write(wd::CLKSEL.val(code) | wd::EN | wd::CLR);
}

/// Watchdog off (only for tests that measure without it).
pub fn stop() {
    rtc::WD_CTL.write(wd::CLR);
}

fn write_word(addr: u32, v: u32) {
    // SAFETY: `addr` is a word in the persistent mailbox (SRAM0), which no
    // Rust object overlaps (the linker script ends the stack below it).
    unsafe { core::ptr::write_volatile(addr as *mut u32, v) }
}

fn read_word(addr: u32) -> u32 {
    // SAFETY: as in `write_word`.
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

/// What the boot stage should start after the reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The installed firmware (normal boot).
    Normal,
    /// The boot ROM's USB recovery (ADFU).
    Adfu,
}

/// Reboot through a short watchdog expiry. Never returns.
pub fn reboot(target: Target) -> ! {
    // A deliberate reboot is not a failed start (fault reboots go through
    // `reboot_after_fault` and keep the count).
    reboot_as(mailbox::deliberate_reboot(target == Target::Adfu))
}

/// What `r` asks of the mailbox, then the reset. Inlined, so each caller
/// keeps only its own case.
#[inline(always)]
fn reboot_as(r: mailbox::Reboot) -> ! {
    if r.clear_count {
        write_word(mailbox::ADDR_BOOT_COUNT, 0);
    }
    reboot_keep_count(if r.adfu { Target::Adfu } else { Target::Normal })
}

fn reboot_keep_count(target: Target) -> ! {
    if target == Target::Adfu {
        write_word(mailbox::ADDR_ADFU, mailbox::ADFU_MAGIC);
    }
    // SAFETY: masking everything below level 0 before the final spin.
    unsafe { cortex_m::register::basepri::write(crate::interrupt::CS_BASEPRI) };
    rtc::WD_CTL.write(wd::CLKSEL.val(0) | wd::EN | wd::CLR);
    loop {
        core::hint::spin_loop();
    }
}

/// After a fault record was written: back to ADFU when a host test asked for
/// it (mailbox flag), otherwise a normal reboot. Keeps the boot counter
/// (`mailbox::fault_reboot`, host-tested): a start that faults before it was
/// confirmed healthy counts as failed, so stage 2 falls back to the next slot
/// after three (until T-6 this went through `reboot` and cleared the
/// counter, so only hangs counted).
pub fn reboot_after_fault() -> ! {
    let flags = read_word(mailbox::BASE + mailbox::OFF_FLAGS as u32);
    let test = read_word(mailbox::ADDR_TEST_MODE);
    reboot_as(mailbox::fault_reboot(test, flags))
}

/// Progress counters for the firmware's `watchdog` task (02 §8): the RT
/// executor, the main executor and the UI loop each bump theirs.
pub static RT_BEAT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static UI_BEAT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Bump a progress counter.
#[inline]
pub fn beat(b: &core::sync::atomic::AtomicU32) {
    b.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// Set by the firmware's USB liveness check (USB-W1,
/// `orpheus_usb::liveness`) when the USB controller cannot be brought back
/// by re-initialising it: VBUS present, the host's SETUPs unanswered after
/// two recoveries, or a requested recovery never carried out. Never set
/// while unplugged or while the host is merely idle.
pub static USB_WEDGED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Health check for the `watchdog` task (which itself proves the main
/// executor runs): feeds only when the RT executor and the UI loop have
/// made progress since the last feed and USB is not wedged. Returns
/// whether it fed.
pub struct Health {
    rt: u32,
    ui: u32,
}

impl Health {
    #[must_use]
    pub const fn new() -> Self {
        Self { rt: u32::MAX, ui: u32::MAX }
    }

    pub fn check(&mut self) -> bool {
        // The stack canary of a paged image (PG-2: MPU region 7 is SRAM3's
        // RAM half there, no longer the guard).
        if !crate::rt::guard_intact() {
            crate::rt::guard_broken();
        }
        let rt = RT_BEAT.load(core::sync::atomic::Ordering::Relaxed);
        let ui = UI_BEAT.load(core::sync::atomic::Ordering::Relaxed);
        let usb_ok = !USB_WEDGED.load(core::sync::atomic::Ordering::Relaxed);
        if rt != self.rt && ui != self.ui && usb_ok {
            self.rt = rt;
            self.ui = ui;
            feed();
            true
        } else {
            false
        }
    }
}

impl Default for Health {
    fn default() -> Self {
        Self::new()
    }
}

static FEED_FROM_IDLE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Let the main executor's idle loop feed the watchdog (bring-up images).
/// The firmware instead feeds from its `watchdog` task when every executor
/// made progress (02 §8).
pub fn feed_from_idle(on: bool) {
    FEED_FROM_IDLE.store(on, core::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn feed_if_allowed() {
    if FEED_FROM_IDLE.load(core::sync::atomic::Ordering::Relaxed) {
        feed();
    }
}
