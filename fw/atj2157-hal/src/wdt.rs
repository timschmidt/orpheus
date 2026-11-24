//! Watchdog and reboot (docs/plan/03-hal-drivers.md §3, 02 §8).
//!
//! `SYSRESETREQ` is ignored by this SoC [HW]; every reboot is a watchdog
//! expiry. The boot stage then reads the mailbox: `"ADFU"` at `0x137FF4`
//! sends it to the boot ROM's USB recovery.

use atj2157_pac::rtc::{self, wd};
use yoton_proto::mailbox;

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
    write_word(mailbox::ADDR_BOOT_COUNT, 0);
    reboot_keep_count(target)
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
/// it (mailbox flag), otherwise a normal reboot.
pub fn reboot_after_fault() -> ! {
    let flags = read_word(mailbox::BASE + mailbox::OFF_FLAGS as u32);
    let test = read_word(mailbox::ADDR_TEST_MODE) == mailbox::TEST_MODE_MAGIC;
    if test && flags & mailbox::FLAG_RETURN_ADFU != 0 { reboot(Target::Adfu) } else { reboot(Target::Normal) }
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
