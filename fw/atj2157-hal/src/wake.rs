//! A wake-up at a time, for futures that must not run `embassy_time`'s
//! code (SYN-3, 2026-11-26).
//!
//! `embassy_time::Timer`'s poll and `_embassy_time_now` are paged code
//! wherever the linker puts them; a future polled every few milliseconds
//! on a path whose code must fit the code cache (the synth's rests) calls
//! this instead, from its own poll. Not in [`crate::time`], whose code is
//! resident by name: this is thread-side code and pages like its callers.

use core::task::Waker;

/// Wake `waker` at `at_us` (µs since `time::init`, [`crate::time::now_us`]),
/// as `embassy_time::Timer` does when it is polled before its deadline.
#[inline(never)]
pub fn wake_at(at_us: u64, waker: &Waker) {
    crate::time::schedule_wake(at_us, waker);
}
