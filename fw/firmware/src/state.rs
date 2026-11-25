//! State shared between tasks: small, copied out under a critical section.

use core::cell::Cell;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32};

use critical_section::Mutex;
use orpheus_power::charger::ChargeState;

#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub percent: u8,
    pub millivolts: u16,
    pub charge: ChargeState,
    pub cable: bool,
}

static STATE: Mutex<Cell<Snapshot>> = Mutex::new(Cell::new(Snapshot {
    percent: 50,
    millivolts: 3800,
    charge: ChargeState::NotPresent,
    cable: false,
}));

pub static CARD_READY: AtomicBool = AtomicBool::new(false);
/// Settings › Storage › Card info, from the card task: the volume's size
/// (MiB) and file system (`CARD_FS_*`, 0 none) at mount, its free space
/// once counted ([`FREE_UNKNOWN`] until then).
pub static CARD_MIB: AtomicU32 = AtomicU32::new(0);
pub static CARD_FREE_MIB: AtomicU32 = AtomicU32::new(FREE_UNKNOWN);
pub static CARD_FS: AtomicU8 = AtomicU8::new(0);
pub const FREE_UNKNOWN: u32 = u32::MAX;
pub const CARD_FS_FAT32: u8 = 1;
pub const CARD_FS_EXFAT: u8 = 2;
pub const CARD_FS_OTHER: u8 = 3;
/// Earphones plugged in (key ladder 1's idle level, debounced by the
/// input task): the radio's antenna.
pub static EARPHONES: AtomicBool = AtomicBool::new(false);

#[must_use]
pub fn snapshot() -> Snapshot {
    critical_section::with(|cs| STATE.borrow(cs).get())
}

pub fn update(f: impl FnOnce(&mut Snapshot)) {
    critical_section::with(|cs| {
        let c = STATE.borrow(cs);
        let mut s = c.get();
        f(&mut s);
        c.set(s);
    });
}

impl Snapshot {
    #[must_use]
    pub fn charge_active(&self) -> bool {
        matches!(self.charge, ChargeState::Charging | ChargeState::Trickle)
    }
}
