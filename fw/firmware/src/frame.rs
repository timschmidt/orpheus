//! Stack frames of async code (VER-10, `cargo xtask stack`).
//!
//! An `async fn`'s state lives in its future, but the temporaries of its
//! `poll` (and of every child future's `poll` inlined into it) live in the
//! poll function's frame. When a parent awaits several children, one of
//! them with large temporaries and another with a deep call chain, the
//! inlined frame holds the large temporaries above the deep chain. [`Own`]
//! gives a child's `poll` a frame of its own, so the depth is the larger of
//! the two instead of their sum.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

/// `fut`, polled in a function of its own.
pub struct Own<F>(pub F);

impl<F: Future> Future for Own<F> {
    type Output = F::Output;

    #[inline(never)]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        // SAFETY: structural pinning of the only field: `Own` never moves
        // it out and has no `Drop` of its own.
        unsafe { self.map_unchecked_mut(|s| &mut s.0) }.poll(cx)
    }
}
