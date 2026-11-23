//! Widgets (05 §5.2). Each is a small `Copy`-able state struct (or a pair
//! of free draw functions) drawn through [`Gfx`](crate::Gfx); none allocates.

pub mod bars;
pub mod dialog;
pub mod list;
pub mod misc;
pub mod overlay;
