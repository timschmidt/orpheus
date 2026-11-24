//! Command layer (06-connectivity §4.2): one command at a time, the
//! matching acknowledge (event 0x00 carrying the command ID) within
//! 500 ms, a bounded number of resends, then the link is declared down
//! (the caller power-cycles the module).
//!
//! Sans-I/O: the caller passes the time and a [`Transport`] that queues
//! bytes for the UART; no async runtime is needed, and the firmware's
//! `async fn command()` is a thin loop around [`CommandLayer::send`],
//! [`CommandLayer::on_frame`] and [`CommandLayer::poll`].

use crate::frame::{Frame, MAX_TX_FRAME};
use crate::proto::Command;

/// Acknowledge timeout (the vendor: 50 ticks of 10 ms).
pub const ACK_TIMEOUT_MS: u64 = 500;
/// Resends after the first transmission for ordinary commands.
pub const DEFAULT_RETRIES: u8 = 1;

/// Where encoded frames go (the UART TX queue).
pub trait Transport {
    fn send(&mut self, frame: &[u8]);
}

/// A command is still waiting for its acknowledge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Busy;

/// What [`CommandLayer::poll`] reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Poll {
    Idle,
    Waiting,
    /// A resend went out.
    Resent {
        id: u8,
    },
    /// No acknowledge after all resends: the link is down.
    Timeout {
        id: u8,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkStats {
    pub sent: u32,
    pub resent: u32,
    pub timeouts: u32,
    /// Acknowledges for nothing pending (e.g. replayed traffic).
    pub stray_acks: u32,
}

pub struct CommandLayer {
    frame: [u8; MAX_TX_FRAME],
    len: usize,
    id: u8,
    pending: bool,
    sent_at: u64,
    retries_left: u8,
    pub stats: LinkStats,
}

impl Default for CommandLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandLayer {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            frame: [0; MAX_TX_FRAME],
            len: 0,
            id: 0,
            pending: false,
            sent_at: 0,
            retries_left: 0,
            stats: LinkStats { sent: 0, resent: 0, timeouts: 0, stray_acks: 0 },
        }
    }

    /// Forget any pending command (module power-cycled).
    pub fn reset(&mut self) {
        self.pending = false;
    }

    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.pending
    }

    /// The last frame handed to the transport (sent or resent). It stays
    /// put until the next send, so a UART driver can transmit straight
    /// from it instead of keeping its own copy.
    #[must_use]
    pub fn last_frame(&self) -> &[u8] {
        self.frame.get(..self.len).unwrap_or(&[])
    }

    /// The command awaiting its acknowledge.
    #[must_use]
    pub fn pending(&self) -> Option<u8> {
        self.pending.then_some(self.id)
    }

    /// Send a command; `retries` resends after a timeout.
    pub fn send(
        &mut self,
        cmd: &Command<'_>,
        retries: u8,
        now_ms: u64,
        tx: &mut impl Transport,
    ) -> Result<(), Busy> {
        let mut b = [0u8; MAX_TX_FRAME];
        let n = cmd.encode(&mut b).map_err(|_| Busy)?;
        self.send_encoded(cmd.id(), &b[..n], retries, now_ms, tx)
    }

    /// Send an already encoded frame (crate-internal; restricted commands
    /// come through [`crate::restricted::send_restricted`]).
    pub(crate) fn send_encoded(
        &mut self,
        id: u8,
        frame: &[u8],
        retries: u8,
        now_ms: u64,
        tx: &mut impl Transport,
    ) -> Result<(), Busy> {
        if self.pending || frame.len() > MAX_TX_FRAME {
            return Err(Busy);
        }
        let Some(f) = self.frame.get_mut(..frame.len()) else { return Err(Busy) };
        f.copy_from_slice(frame);
        self.len = frame.len();
        self.id = id;
        self.pending = true;
        self.sent_at = now_ms;
        self.retries_left = retries;
        self.stats.sent += 1;
        tx.send(self.last_frame());
        Ok(())
    }

    /// Look at a received frame; returns the ID of the command it
    /// acknowledges, if it is the pending one.
    pub fn on_frame(&mut self, f: &Frame) -> Option<u8> {
        if f.id != 0x00 {
            return None;
        }
        let cmd = *f.params().first()?;
        if self.pending && cmd == self.id {
            self.pending = false;
            Some(cmd)
        } else {
            self.stats.stray_acks += 1;
            None
        }
    }

    /// Check the acknowledge timeout; resends or gives up.
    pub fn poll(&mut self, now_ms: u64, tx: &mut impl Transport) -> Poll {
        if !self.pending {
            return Poll::Idle;
        }
        if now_ms.saturating_sub(self.sent_at) < ACK_TIMEOUT_MS {
            return Poll::Waiting;
        }
        if self.retries_left > 0 {
            self.retries_left -= 1;
            self.sent_at = now_ms;
            self.stats.resent += 1;
            tx.send(self.last_frame());
            return Poll::Resent { id: self.id };
        }
        self.pending = false;
        self.stats.timeouts += 1;
        Poll::Timeout { id: self.id }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    #[derive(Default)]
    struct Tx(Vec<Vec<u8>>);
    impl Transport for Tx {
        fn send(&mut self, f: &[u8]) {
            self.0.push(f.to_vec());
        }
    }

    #[test]
    fn ack_timeout_retry() {
        let mut l = CommandLayer::new();
        let mut tx = Tx::default();
        l.send(&Command::Identity(2), 1, 0, &mut tx).unwrap();
        assert_eq!(l.send(&Command::ReadPaired, 1, 0, &mut tx), Err(Busy));
        assert_eq!(l.poll(499, &mut tx), Poll::Waiting);
        // A stray ack for another command does not complete it.
        assert_eq!(l.on_frame(&Frame::new(0, &[0x2C])), None);
        assert_eq!(l.poll(500, &mut tx), Poll::Resent { id: 6 });
        assert_eq!(tx.0.len(), 2);
        assert_eq!(tx.0[0], tx.0[1]);
        assert_eq!(l.poll(999, &mut tx), Poll::Waiting);
        assert_eq!(l.poll(1000, &mut tx), Poll::Timeout { id: 6 });
        assert!(!l.is_busy());
        // Acknowledged in time.
        l.send(&Command::ReadPaired, 1, 2000, &mut tx).unwrap();
        assert_eq!(l.on_frame(&Frame::new(0, &[0x2A])), Some(0x2A));
        assert_eq!(l.poll(5000, &mut tx), Poll::Idle);
        assert_eq!(l.stats, LinkStats { sent: 2, resent: 1, timeouts: 1, stray_acks: 1 });
    }
}
