//! Power logic that needs no hardware: the charger state machine (a faithful
//! reproduction of the stock firmware's algorithm, docs/hw/system.md §3.1)
//! and the battery gauge, the clock governor's decision (PWR-2, PWR-5) and
//! the battery-drain log (PWR-4/PWR-5).

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod charger;
pub mod drain;
pub mod gauge;
pub mod governor;
