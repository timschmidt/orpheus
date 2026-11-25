//! orpheus-books: the Books reader's core (05 §7.15, BOOK-1).
//!
//! `no_std`, no allocator, no `core::fmt`, no hardware: host-tested.
//!
//! * [`enc`]: encoding detection (byte-order marks, UTF-16 without one,
//!   XML declarations, UTF-8, Windows-1251 / KOI8-R / Latin-1) and a
//!   streaming decoder.
//! * [`txt`], [`md`], [`fb2`]: the formats' parsers; [`convert`] runs one
//!   over a file in pieces into the normalised stream of [`norm`] (the
//!   book's cached plain form) and reads titles and authors for the
//!   library.
//! * [`layout`]: line breaking and pages over the normalised stream with
//!   the reader's font metrics; [`files`]: the page index, contents,
//!   position and bookmark, and library files under `/.orpheus/`.
//! * [`shelf`]: the Books app's model and its card work in bounded steps
//!   over `orpheus-fs` (scan, convert, lay out, read windows), for the
//!   firmware's card task and the simulator alike.

#![no_std]
#![forbid(unsafe_code)]

pub mod convert;
pub mod enc;
pub mod fb2;
pub mod files;
pub mod layout;
pub mod md;
pub mod norm;
pub mod shelf;
pub mod txt;
