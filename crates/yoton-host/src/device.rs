//! Which firmware the attached player runs, and getting it into ADFU.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use yoton_proto::devchan::{ADFU_PID, ADFU_VID, DEV_PID, DEV_VID, ROCKBOX_PID, STOCK_PID};

use crate::adfu::{Adfu, Mode};
use crate::{bot, sg};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Boot ROM recovery (`10d6:10d6`), monitor not loaded.
    AdfuRom,
    /// Boot ROM recovery with `ymon` running.
    Ymon,
    /// ADFU device present but not answering.
    AdfuStuck,
    /// The Rockbox port (`10d6:1157`).
    Rockbox,
    /// Stock firmware in USB disk mode (`10d6:1101`).
    Stock,
    /// yotonrs with its developer channel (`1209:0001`).
    Yotonrs,
    /// Nothing recognisable on the bus.
    Absent,
}

impl State {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::AdfuRom => "adfu-rom",
            Self::Ymon => "ymon",
            Self::AdfuStuck => "adfu-not-answering",
            Self::Rockbox => "rockbox",
            Self::Stock => "stock",
            Self::Yotonrs => "yotonrs",
            Self::Absent => "absent",
        }
    }
}

/// Look at the bus (and ask the ADFU device which program answers).
#[must_use]
pub fn state() -> State {
    if bot::find(ADFU_VID, ADFU_PID).is_some() {
        return match Adfu::open(Duration::from_secs(1)).map(|mut a| a.mode()) {
            Ok(Some(Mode::Ymon)) => State::Ymon,
            Ok(Some(Mode::Rom)) => State::AdfuRom,
            _ => State::AdfuStuck,
        };
    }
    if bot::find(ADFU_VID, ROCKBOX_PID).is_some() {
        return State::Rockbox;
    }
    if bot::find(ADFU_VID, STOCK_PID).is_some() {
        return State::Stock;
    }
    if bot::find(DEV_VID, DEV_PID).is_some() {
        return State::Yotonrs;
    }
    State::Absent
}

/// Bring the player into ADFU with ymon loaded, from whatever runs now.
pub fn to_ymon(log: &mut dyn FnMut(&str), timeout: Duration) -> Result<Adfu> {
    let t0 = Instant::now();
    let mut asked = false;
    loop {
        match state() {
            State::Ymon | State::AdfuRom => {
                let mut a = Adfu::open(Duration::from_secs(2))?;
                a.ensure_ymon()?;
                return Ok(a);
            }
            State::Rockbox | State::Stock if !asked => {
                // usb-storage needs a moment to create the sg node.
                std::thread::sleep(Duration::from_secs(2));
                let nodes = sg::find_sg(&["Rockbox", "ACTIONS"]);
                let Some(node) = nodes.first() else {
                    bail!("player in mass-storage mode but no /dev/sg node found");
                };
                log(&format!("asking the running firmware to reboot into ADFU ({})", node.display()));
                sg::reboot_to_adfu(node)?;
                asked = true;
            }
            State::Yotonrs => bail!("yotonrs developer-channel reboot not implemented yet (WP-USB-2)"),
            State::AdfuStuck => {
                bail!("ADFU device present but not answering; wait for the watchdog or ask the user")
            }
            _ => {}
        }
        if t0.elapsed() > timeout {
            bail!("player did not reach ADFU within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}
