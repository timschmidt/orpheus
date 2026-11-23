//! USB personalities and classes (06-connectivity §2). Hardware-free: the
//! controller driver is `atj2157-hal::usb`; target-specific actions go
//! through small traits.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod devchan;
