//! Regression (2026-11-25, soak finding): the card's pacing sleep ran
//! inside `card::spin` (a synchronous library session), where the timer
//! never fired, and the player hung until the watchdog. `spin` must mark
//! itself so block devices skip waits that need time to pass.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};

struct SeesSync;

impl Future for SeesSync {
    type Output = bool;
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<bool> {
        Poll::Ready(orpheus_fs::SyncPoll::active())
    }
}

#[test]
fn regress_spin_marks_a_synchronous_poller() {
    let mut cx = Context::from_waker(Waker::noop());
    assert!(!orpheus_fs::SyncPoll::active());
    assert!(orpheus_index::card::spin(&mut cx, SeesSync));
    assert!(!orpheus_fs::SyncPoll::active(), "the mark ends with the poller");
}
