//! The browser installer (WEB-3, docs/plan/09-docs-website.md §5).
//!
//! `orpheus-host`'s protocols and flows — ADFU, ymon, the NOR payloads,
//! the developer channel, backup, first install, update, restore,
//! recovery — compiled to WebAssembly and run over the page's WebUSB
//! transport (`site/install/usb.js`, a [`orpheus_host::transport::Transport`]
//! on the JavaScript side). Nothing here re-implements a protocol; this
//! crate only binds.
//!
//! The simulated player (`orpheus_host::mock`) is exported too
//! (`MockDevice`): the page's demo mode and the Node test suite
//! (`tests/web/install.mjs`) drive the real flows against it through the
//! same WebUSB shape.
//!
//! The JSON the page reads is built here ([`json`]), host-tested.

pub mod json;

#[cfg(target_arch = "wasm32")]
mod web;
