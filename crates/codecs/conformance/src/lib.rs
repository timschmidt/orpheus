//! Codec conformance harness (07 §3.3–3.4, WP T-7).
//!
//! * [`drive`]: runs any built decoder through the facade (`orpheus-codecs`)
//!   over a simulated stream ring, checking the decoder contract.
//! * [`checks`]: the per-file checks against the generated corpus
//!   (`tests/corpus/`, made by `cargo xtask test codecs --gen`): reference
//!   comparison, FLAC MD5, gapless length, restartability, seeking,
//!   robustness, arena budget.
//! * [`fuzz`]: fuzz-target bodies shared by `cargo fuzz` and the stable
//!   smoke run.
//!
//! The binary (`src/main.rs`) prints the report; `xtask test codecs` runs it.

pub mod checks;
pub mod compare;
pub mod drive;
pub mod fuzz;
pub mod join;
pub mod rng;

use std::cell::RefCell;

thread_local! {
    static CONTEXT: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Record what this thread is checking, so that a panic (which aborts in
/// release builds) names the file and case.
pub fn set_context(s: &str) {
    CONTEXT.with(|c| {
        if let Ok(mut c) = c.try_borrow_mut() {
            c.clear();
            c.push_str(s);
        }
    });
}

/// Install a panic hook that prints the panicking thread's context first.
pub fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        CONTEXT.with(|c| {
            if let Ok(c) = c.try_borrow() {
                eprintln!("PANIC while checking: {c}");
            }
        });
        prev(info);
    }));
}
