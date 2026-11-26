//! orpheus-ui: the UI toolkit of 05-ui-apps §5.
//!
//! `no_std`, no allocator, no hardware. It draws into a 128 × 128 RGB565
//! frame buffer through [`Gfx`] (which is also an `embedded-graphics`
//! `DrawTarget`), turns key edges into the key grammar ([`input`]), keeps the
//! navigation stack and overlays ([`Shell`]), and provides the widgets.
//! The same code runs on the device and in the simulator (`sim/`).
//!
//! RAM use is documented in `README.md`.

#![no_std]
#![forbid(unsafe_code)]

pub mod color;
pub mod event;
pub mod font;
pub mod geom;
pub mod gfx;
pub mod icons;
pub mod input;
pub mod menu;
pub mod nav;
pub mod shell;
pub mod text;
pub mod theme;
pub mod widgets;

pub use color::Color;
pub use event::{Event, Instant, Key, KeyPhase, ServiceEvent, UiEvent};
pub use font::{FontId, GlyphCache, GlyphSource, TextSize};
pub use geom::Rect;
pub use gfx::{Align, DirtySet, Fallback, Gfx};
pub use input::{KeyGate, KeyMapper};
pub use menu::MenuBuilder;
pub use nav::{Arg, Cx, Dirty, Nav, Resume, Screen, ScreenId, ScreenSet, StripUse, Wants};
pub use shell::{Requests, Shell, System};
pub use theme::{Accent, Palette, Role, Theme};
