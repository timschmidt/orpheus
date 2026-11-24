//! State shared between tasks: small, copied out under a critical section.

use core::cell::Cell;
use core::sync::atomic::AtomicBool;

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
