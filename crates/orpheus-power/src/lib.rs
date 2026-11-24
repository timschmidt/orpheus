//! Power logic that needs no hardware: the charger state machine (a faithful
//! reproduction of the stock firmware's algorithm, docs/hw/system.md §3.1)
//! and the battery gauge.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod charger;
pub mod gauge;
