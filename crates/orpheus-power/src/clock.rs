//! The core clock levels and their supply voltages (03 §2.1), shared by
//! the HAL (which switches them) and the governor (which chooses them), and
//! the order of a level switch (PWR-5): the voltage goes up before the
//! clock and down after it. Host-tested here; `atj2157_hal::clock` carries
//! it out.

/// Clock levels, in rising order: rows of the vendor's own level table
/// (system.md §1.4), never interpolated.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    /// 24 MHz from HOSC, COREPLL off.
    Low,
    /// 48 MHz, COREPLL N = 8.
    Base,
    /// 96 MHz, N = 16.
    Mid,
    /// 144 MHz, N = 24.
    High,
    /// 192 MHz, N = 32, core supply 1.1 V.
    Turbo,
    /// 288 MHz, N = 48, core supply 1.3 V, `SYSCLK` bit 8 — the vendor's
    /// level 29 and the chip's rated maximum ([DS] §1.2 "Up to 288MHz").
    /// The governor chooses it when the load needs it (decision D-4 as
    /// revised 2026-11-26; High performance pins it).
    Max,
}

/// Core supply code of the highest level and the ceiling of every write
/// to `VOUT_CTL[4:0]`: `0x18` = 1.300 V. [DS] §2.1 gives an absolute
/// maximum of 1.35 V for VDD and §2.2 a recommended range of 0.9–1.35 V;
/// codes `0x19`/`0x1A` (1.325/1.35 V) would sit at that limit, and 288 MHz
/// (the rated maximum) needs no more than `0x18`, which the stock firmware
/// uses for it.
pub const VDD_MAX: u8 = 0x18;

/// Core supply codes this firmware may write: the vendor table's rows for
/// the levels above (`0x0C` = 1.0 V, `0x10` = 1.1 V, `0x18` = 1.3 V).
pub const ALLOWED_VDD: [u8; 3] = [0x0C, 0x10, VDD_MAX];

const _: () = {
    let mut i = 0;
    while i < Level::ALL.len() {
        assert!(Level::ALL[i].vdd_code() <= VDD_MAX);
        i += 1;
    }
    let mut j = 0;
    while j < ALLOWED_VDD.len() {
        assert!(ALLOWED_VDD[j] <= VDD_MAX);
        j += 1;
    }
};

impl Level {
    pub const ALL: [Level; 6] = [Level::Low, Level::Base, Level::Mid, Level::High, Level::Turbo, Level::Max];

    /// Core clock in MHz.
    #[must_use]
    pub const fn mhz(self) -> u16 {
        match self {
            Level::Low => 24,
            Level::Base => 48,
            Level::Mid => 96,
            Level::High => 144,
            Level::Turbo => 192,
            Level::Max => 288,
        }
    }

    /// Core clock in Hz.
    #[must_use]
    pub const fn hz(self) -> u32 {
        self.mhz() as u32 * 1_000_000
    }

    /// Core supply code `VOUT_CTL[4:0]` the vendor uses at this level
    /// (01 §7): `0x0C` up to 144 MHz, `0x10` at 192, `0x18` at 288.
    #[must_use]
    pub const fn vdd_code(self) -> u8 {
        match self {
            Level::Low | Level::Base | Level::Mid | Level::High => 0x0C,
            Level::Turbo => 0x10,
            Level::Max => VDD_MAX,
        }
    }

    /// From its index in [`Level::ALL`] (anything larger is `Max`).
    #[must_use]
    pub const fn from_index(i: u8) -> Level {
        match i {
            0 => Level::Low,
            1 => Level::Base,
            2 => Level::Mid,
            3 => Level::High,
            4 => Level::Turbo,
            _ => Level::Max,
        }
    }

    /// The level of an audio clock floor (`firmware::CLOCK_FLOOR`,
    /// `audio::CODEC_FLOOR`): 0 Base, 1 Mid, 2 High, 3 Turbo, 4 Max
    /// (anything larger is Max).
    #[must_use]
    pub const fn from_floor(f: u8) -> Level {
        Level::from_index(if f >= 4 { 5 } else { f + 1 })
    }

    /// The floor code of this level ([`Self::from_floor`]'s inverse; Low
    /// gives 0, as Base).
    #[must_use]
    pub const fn as_floor(self) -> u8 {
        match self {
            Level::Low | Level::Base => 0,
            Level::Mid => 1,
            Level::High => 2,
            Level::Turbo => 3,
            Level::Max => 4,
        }
    }
}

/// The supply code a write may use: `code` clamped to [`VDD_MAX`], `None`
/// when that is not one of [`ALLOWED_VDD`] (nothing is written).
#[must_use]
pub const fn vdd_write(code: u32) -> Option<u32> {
    let c = if code > VDD_MAX as u32 { VDD_MAX as u32 } else { code };
    let mut i = 0;
    while i < ALLOWED_VDD.len() {
        if ALLOWED_VDD[i] as u32 == c {
            return Some(c);
        }
        i += 1;
    }
    None
}

/// The supply writes around one level switch: `before` the clock changes
/// (then wait for it to settle), `after` it. Going up the voltage is
/// raised first; going down the clock is lowered first and the voltage
/// last (the vendor's order, system.md §1.3 steps 4–5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Switch {
    pub before: Option<u8>,
    pub after: Option<u8>,
}

/// The supply writes for a switch to `to` with the supply at `vdd_now`.
#[must_use]
pub const fn switch(vdd_now: u8, to: Level) -> Switch {
    let v = to.vdd_code();
    if v > vdd_now {
        Switch { before: Some(v), after: None }
    } else if v < vdd_now {
        Switch { before: None, after: Some(v) }
    } else {
        Switch { before: None, after: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lowest supply code a clock (MHz) is run at by the vendor table.
    fn needs(mhz: u16) -> u8 {
        Level::ALL.iter().find(|l| l.mhz() >= mhz).map_or(VDD_MAX, |l| l.vdd_code())
    }

    /// PWR-5: every switch, from every level to every other, carried out
    /// in [`switch`]'s order (supply before, the HOSC detour at 24 MHz, the
    /// new clock, supply after) never runs a clock above what the supply
    /// of that moment carries: up to Max the 1.3 V comes first, down from
    /// Max the clock drops first and 1.3 V goes last.
    #[test]
    fn switch_order_keeps_the_clock_within_the_supply() {
        for from in Level::ALL {
            for to in Level::ALL {
                let mut vdd = from.vdd_code();
                let s = switch(vdd, to);
                let check =
                    |vdd: u8, mhz: u16| assert!(vdd >= needs(mhz), "{from:?}→{to:?}: {mhz} MHz at {vdd:#x}");
                check(vdd, from.mhz());
                if let Some(v) = s.before {
                    assert!(v > vdd, "{from:?}→{to:?}: before only raises");
                    vdd = v;
                    check(vdd, from.mhz());
                }
                check(vdd, 24);
                check(vdd, to.mhz());
                if let Some(v) = s.after {
                    assert!(v < vdd, "{from:?}→{to:?}: after only lowers");
                    vdd = v;
                }
                check(vdd, to.mhz());
                assert_eq!(vdd, to.vdd_code(), "{from:?}→{to:?} ends at the level's code");
            }
        }
        assert_eq!(switch(0x0C, Level::Max), Switch { before: Some(0x18), after: None });
        assert_eq!(switch(0x10, Level::Max), Switch { before: Some(0x18), after: None });
        assert_eq!(switch(0x18, Level::High), Switch { before: None, after: Some(0x0C) });
        assert_eq!(switch(0x18, Level::Turbo), Switch { before: None, after: Some(0x10) });
        assert_eq!(switch(0x18, Level::Low), Switch { before: None, after: Some(0x0C) });
        assert_eq!(switch(0x0C, Level::Mid), Switch { before: None, after: None });
        // From the reset default (1.2 V) to a 1.0 V level: lowered after.
        assert_eq!(switch(0x14, Level::Base), Switch { before: None, after: Some(0x0C) });
    }

    /// [DS] §2.1/§2.2: no level and no write goes above 1.3 V (`0x18`).
    #[test]
    fn no_supply_above_1_3_v() {
        for l in Level::ALL {
            assert!(l.vdd_code() <= VDD_MAX, "{l:?}");
            assert!(ALLOWED_VDD.contains(&l.vdd_code()), "{l:?}");
        }
        assert_eq!(Level::Max.vdd_code(), 0x18);
        for code in 0..=0x1F_u32 {
            if let Some(c) = vdd_write(code) {
                assert!(c <= u32::from(VDD_MAX), "{code:#x} → {c:#x}");
            }
        }
        assert_eq!(vdd_write(0x19), Some(0x18), "clamped");
        assert_eq!(vdd_write(0x1F), Some(0x18), "clamped");
        assert_eq!(vdd_write(0x14), None, "not a level's code");
        assert_eq!(vdd_write(0x0C), Some(0x0C));
        assert_eq!(vdd_write(0x10), Some(0x10));
    }

    #[test]
    fn levels() {
        for (i, l) in Level::ALL.iter().enumerate() {
            assert_eq!(Level::from_index(i as u8), *l);
            assert_eq!(l.hz(), u32::from(l.mhz()) * 1_000_000);
        }
        assert_eq!(Level::from_index(9), Level::Max);
        assert_eq!(Level::Max.mhz(), 288);
        let floors = [Level::Base, Level::Mid, Level::High, Level::Turbo, Level::Max, Level::Max];
        for (f, l) in floors.iter().enumerate() {
            assert_eq!(Level::from_floor(f as u8), *l);
        }
        assert_eq!(Level::from_floor(255), Level::Max);
    }
}
