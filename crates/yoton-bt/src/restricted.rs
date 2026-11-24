//! Restricted commands — **not for automatic use**.
//!
//! CLAUDE.md rule 6: commands 0x20–0x24, 0x28, 0x2C and 0x7F are sent only
//! by the work package that owns them, and the module upgrade never without
//! a known-good image and the user's explicit go-ahead (recovery from a
//! failed module upgrade is unknown; decision D-6). The only user in this
//! crate is [`crate::Transmitter::shutdown`], which sends the shutdown
//! command (0x24) on the player's power-off path (owner decision
//! 2026-11-24, WP BT-2). The transmitter state machine skips the vendor's
//! settings read (0x2C) at start-up, and switching Bluetooth off drops the
//! enable line without 0x24. Tests check that no other path emits these
//! IDs.

use crate::frame::{self, EncodeError};
use crate::link::{Busy, CommandLayer, Transport};

/// A restricted command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestrictedCommand<'a> {
    /// 0x20 size[2]: enter upgrade mode (packet size, 0x0200 for ATS2851).
    EnterUpgrade { packet_size: u16 },
    /// 0x21: leave upgrade mode.
    LeaveUpgrade,
    /// 0x22 seq[2] data: one upgrade data packet (a 519-byte frame for 512
    /// bytes of data; encode only, it does not fit the command layer).
    UpgradeData { seq: u16, data: &'a [u8] },
    /// 0x23: restart the module (event 0x23 follows).
    RestartModule,
    /// 0x24: shut down (event 0x24 follows; then the enable line low).
    Shutdown,
    /// 0x28 idx: change the UART baud rate (6 = 115200, 2 = 1 500 000);
    /// the host changes its own divisor after the acknowledge.
    ChangeBaud { index: u8 },
    /// 0x2C: read module settings (event 0x2C).
    ReadSettings,
    /// 0x7F idx power: write module settings; the module restarts.
    WriteSettings { baud_index: u8, power: u8 },
}

impl RestrictedCommand<'_> {
    #[must_use]
    pub fn id(&self) -> u8 {
        match self {
            RestrictedCommand::EnterUpgrade { .. } => 0x20,
            RestrictedCommand::LeaveUpgrade => 0x21,
            RestrictedCommand::UpgradeData { .. } => 0x22,
            RestrictedCommand::RestartModule => 0x23,
            RestrictedCommand::Shutdown => 0x24,
            RestrictedCommand::ChangeBaud { .. } => 0x28,
            RestrictedCommand::ReadSettings => 0x2C,
            RestrictedCommand::WriteSettings { .. } => 0x7F,
        }
    }
}

/// Encode a restricted command into `out` (an upgrade data packet needs
/// `data.len() + 7` bytes). The caller is the owning work package.
pub fn encode_restricted(cmd: &RestrictedCommand<'_>, out: &mut [u8]) -> Result<usize, EncodeError> {
    let id = cmd.id();
    match *cmd {
        RestrictedCommand::EnterUpgrade { packet_size } => {
            frame::encode_unchecked(id, &packet_size.to_be_bytes(), out)
        }
        RestrictedCommand::UpgradeData { seq, data } => {
            let n = 7 + data.len();
            if out.len() < n {
                return Err(EncodeError::TooLong);
            }
            // Build in place: header, seq, data, checksum.
            let len = (data.len() + 3) as u16;
            out[0] = frame::SYNC;
            out[1..3].copy_from_slice(&len.to_be_bytes());
            out[3] = id;
            out[4..6].copy_from_slice(&seq.to_be_bytes());
            out[6..6 + data.len()].copy_from_slice(data);
            out[n - 1] = frame::checksum(&out[1..n - 1]);
            Ok(n)
        }
        RestrictedCommand::ChangeBaud { index } => frame::encode_unchecked(id, &[index], out),
        RestrictedCommand::WriteSettings { baud_index, power } => {
            frame::encode_unchecked(id, &[baud_index, power], out)
        }
        RestrictedCommand::LeaveUpgrade
        | RestrictedCommand::RestartModule
        | RestrictedCommand::Shutdown
        | RestrictedCommand::ReadSettings => frame::encode_unchecked(id, &[], out),
    }
}

/// Send a restricted command through the command layer (acknowledge,
/// timeout and retry as for any command). Not for upgrade data packets.
pub fn send_restricted(
    layer: &mut CommandLayer,
    cmd: &RestrictedCommand<'_>,
    retries: u8,
    now_ms: u64,
    tx: &mut impl Transport,
) -> Result<(), Busy> {
    let mut b = [0u8; frame::MAX_TX_FRAME];
    let n = encode_restricted(cmd, &mut b).map_err(|_| Busy)?;
    layer.send_encoded(cmd.id(), &b[..n], retries, now_ms, tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(c: RestrictedCommand<'_>) -> std::vec::Vec<u8> {
        let mut b = [0u8; 600];
        let n = encode_restricted(&c, &mut b).unwrap();
        b[..n].to_vec()
    }

    #[test]
    fn documented_bytes() {
        assert_eq!(enc(RestrictedCommand::ReadSettings), [0xAA, 0, 1, 0x2C, 0xD3]);
        assert_eq!(enc(RestrictedCommand::Shutdown), [0xAA, 0, 1, 0x24, 0xDB]);
        assert_eq!(enc(RestrictedCommand::RestartModule), [0xAA, 0, 1, 0x23, 0xDC]);
        assert_eq!(enc(RestrictedCommand::ChangeBaud { index: 2 }), [0xAA, 0, 2, 0x28, 0x02, 0xD4]);
        assert_eq!(
            enc(RestrictedCommand::WriteSettings { baud_index: 6, power: 7 }),
            [0xAA, 0, 3, 0x7F, 6, 7, 0x71]
        );
        assert_eq!(
            enc(RestrictedCommand::EnterUpgrade { packet_size: 0x200 }),
            [0xAA, 0, 3, 0x20, 2, 0, 0xDB]
        );
        // 512 data bytes: a 519-byte frame starting AA 02 03 22 (§6 step 5).
        let data = [0x5Au8; 512];
        let f = enc(RestrictedCommand::UpgradeData { seq: 1, data: &data });
        assert_eq!(f.len(), 519);
        assert_eq!(&f[..6], &[0xAA, 0x02, 0x03, 0x22, 0x00, 0x01]);
        assert_eq!(frame::checksum(&f[1..]), 0);
    }
}
