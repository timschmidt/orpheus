//! A simulated player behind the [`Transport`] trait: the boot ROM's ADFU
//! mode, ymon, the NOR payloads and the running firmware's developer
//! channel (INFO, REBOOT and the update), over a simulated 8 MiB NOR
//! (`orpheus_update::sim::SimNor`). The host tests run every flow against
//! it; the browser installer's demo mode and its Node test suite use it
//! through WebUSB's shape.
//!
//! Payloads are recognised by a tag at their start ([`TAG_YMON`] …), so the
//! tests and the demo can pass small stand-ins for the real images.
//!
//! What it checks (and counts in [`MockPlayer::violations`]): an erase or
//! program of sector 0 by anything but the sector-0 payload, anything at
//! `0x7E0000` and above, and the sector-0 payload outside sector 0.

use std::collections::VecDeque;

use anyhow::{Result, anyhow, bail};
use orpheus_proto::devchan::{Info, REBOOT_ADFU, REQ_INFO, REQ_REBOOT, REQ_TEST, REQ_UPDATE, VERSION};
use orpheus_proto::layout::{NOR_SIZE, SECTOR, full, full_writable};
use orpheus_proto::mem::{YMON_ARGS, YMON_BASE, YMON_PAYLOAD};
use orpheus_proto::slot::{HEADER_SIZE, SlotHeader};
use orpheus_update::install::{Flash, Installer, Phase};
use orpheus_update::sim::SimNor;
use orpheus_update::wire::{self, State, Status};
use orpheus_update::{Error as UpdError, slots};

use crate::adfu::{
    SUB_ABORT, SUB_EXEC, SUB_EXEC_STATE, SUB_FAULT, SUB_JUMP, SUB_PING, SUB_READ, SUB_RESULT_DATA,
    SUB_RESULT_SIZE, SUB_WRITE,
};
use crate::bot::{CBW_LEN, CBW_SIG, CSW_SIG};
use crate::nor::BUF;
use crate::transport::{Control, Transport};

pub const TAG_YMON: &[u8] = b"MOCK-YMON";
pub const TAG_NORDUMP: &[u8] = b"MOCK-NORDUMP";
pub const TAG_NORPROG: &[u8] = b"MOCK-NORPROG";
pub const TAG_NORPROG0: &[u8] = b"MOCK-NORPROG0";

/// Stand-ins for the payload images.
#[must_use]
pub fn payload(tag: &[u8]) -> Vec<u8> {
    let mut v = tag.to_vec();
    v.resize(64, 0);
    v
}

const RAM_BASE: u32 = 0x0010_0000;
const RAM_SIZE: usize = 0x4_0000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The boot ROM's ADFU mode (ymon when `ymon`).
    Adfu,
    /// Orpheus runs (the developer channel answers).
    Firmware,
    /// Nothing starts (no valid slot and no recovery: never expected).
    Dead,
}

pub struct MockPlayer {
    pub nor: SimNor,
    pub mode: Mode,
    pub ymon: bool,
    ram: Vec<u8>,
    /// A bulk OUT CBW waiting for its data stage.
    pending: Option<([u8; 16], usize)>,
    replies: VecDeque<Vec<u8>>,
    result: Vec<u8>,
    exec_state: u32,
    /// Payload calls so far, and the one that fails (an unplugged cable).
    pub calls: usize,
    pub fail_call: Option<usize>,
    /// Flip a bit in the next `nordump` result (an unreliable read).
    pub flaky_read: bool,
    /// Forbidden writes attempted (see the module documentation).
    pub violations: usize,
    /// Sector-0 writes by the sector-0 payload.
    pub sector0_writes: usize,
    /// The firmware's update over the developer channel.
    inst: Option<Installer>,
    commit: bool,
    upd_error: u8,
    clock: u64,
}

impl Default for MockPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl MockPlayer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            nor: SimNor::new(),
            mode: Mode::Adfu,
            ymon: false,
            ram: vec![0; RAM_SIZE],
            pending: None,
            replies: VecDeque::new(),
            result: Vec::new(),
            exec_state: 0,
            calls: 0,
            fail_call: None,
            flaky_read: false,
            violations: 0,
            sector0_writes: 0,
            inst: None,
            commit: false,
            upd_error: 0,
            clock: 0,
        }
    }

    /// A player with the original firmware: a vendor boot record and data
    /// everywhere (`seed`), vendor data at the top.
    #[must_use]
    pub fn stock(seed: u8) -> Self {
        let mut m = Self::new();
        let mut x = u32::from(seed) | 0x100;
        for b in &mut m.nor.data {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *b = x as u8;
        }
        // Erased stretches, as a real chip has.
        m.nor.data[0x76_2000..0x7D_F000].fill(0xFF);
        m.nor.data[2] = 0xA5;
        m.nor.data[orpheus_proto::layout::dual::STAGE2 as usize] = 0;
        m
    }

    /// Stage 2's choice on this chip, then that mode (a reset).
    pub fn boot(&mut self) {
        let pick = {
            let mut r = self.nor.reader();
            slots::stage2_pick(&mut r)
        };
        self.ymon = false;
        self.inst = None;
        self.commit = false;
        self.mode = if pick.is_some() { Mode::Firmware } else { Mode::Adfu };
    }

    /// The slot stage 2 starts (index in `layout::SLOTS`).
    #[must_use]
    pub fn booted(&self) -> Option<usize> {
        let mut r = self.nor.reader();
        slots::stage2_pick(&mut r)
    }

    fn ram(&mut self, addr: u32, n: usize) -> Result<&mut [u8]> {
        let o = addr.checked_sub(RAM_BASE).ok_or_else(|| anyhow!("RAM address {addr:#x}"))? as usize;
        self.ram.get_mut(o..o + n).ok_or_else(|| anyhow!("RAM range {addr:#x}+{n:#x}"))
    }

    fn word(&mut self, addr: u32) -> u32 {
        self.ram(addr, 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn csw(status: u8) -> Vec<u8> {
        let mut c = vec![0u8; 13];
        c[..4].copy_from_slice(&CSW_SIG.to_le_bytes());
        c[12] = status;
        c
    }

    /// One ADFU/ymon command.
    fn command(&mut self, cdb: [u8; 16], out: &[u8]) {
        if cdb[0] == crate::adfu::CDB_RESET {
            self.boot();
            return;
        }
        let w = |o: usize| u32::from_le_bytes([cdb[o], cdb[o + 1], cdb[o + 2], cdb[o + 3]]);
        let (sub, len, addr) = (w(1), w(5) as usize, w(9));
        let (data, status) = match sub {
            SUB_PING if self.ymon => {
                let mut d = b"YMON".to_vec();
                d.resize(16, 0);
                (d, 0)
            }
            SUB_PING => (Vec::new(), 1),
            SUB_WRITE => match self.ram(addr, out.len()) {
                Ok(s) => {
                    s.copy_from_slice(out);
                    (Vec::new(), 0)
                }
                Err(_) => (Vec::new(), 1),
            },
            SUB_READ => match self.ram(addr, len) {
                Ok(s) => (s.to_vec(), 0),
                Err(_) => (Vec::new(), 1),
            },
            SUB_JUMP => {
                if addr & !1 == YMON_BASE && self.ram(YMON_BASE, TAG_YMON.len()).is_ok_and(|s| s == TAG_YMON)
                {
                    self.ymon = true;
                }
                (Vec::new(), 0)
            }
            SUB_EXEC if self.ymon => {
                self.exec(addr & !1);
                (Vec::new(), 0)
            }
            SUB_EXEC_STATE if self.ymon => (self.exec_state.to_le_bytes().to_vec(), 0),
            SUB_RESULT_SIZE if self.ymon => ((self.result.len() as u32).to_le_bytes().to_vec(), 0),
            SUB_RESULT_DATA if self.ymon => (self.result.clone(), 0),
            SUB_FAULT if self.ymon => (vec![0; 32], 0),
            SUB_ABORT if self.ymon => (Vec::new(), 0),
            _ => (Vec::new(), 1),
        };
        if !data.is_empty() {
            self.replies.push_back(data);
        }
        self.replies.push_back(Self::csw(status));
    }

    fn exec(&mut self, at: u32) {
        self.calls += 1;
        self.result.clear();
        if self.fail_call == Some(self.calls) {
            self.exec_state = 3; // faulted: the flow stops here
            return;
        }
        self.exec_state = 2;
        if at != YMON_PAYLOAD {
            self.exec_state = 3;
            return;
        }
        let tag = self.ram(YMON_PAYLOAD, 16).map(|s| s.to_vec()).unwrap_or_default();
        let a = [self.word(YMON_ARGS), self.word(YMON_ARGS + 4), self.word(YMON_ARGS + 8)];
        if tag.starts_with(TAG_NORDUMP) {
            let (addr, n) = (a[0] as usize, a[1] as usize);
            let mut d = self.nor.data.get(addr..addr + n).map(<[u8]>::to_vec).unwrap_or_default();
            if self.flaky_read && !d.is_empty() {
                self.flaky_read = false;
                d[n / 2] ^= 0x10;
            }
            self.result = d;
            if let Ok(s) = self.ram(YMON_ARGS + 12, 4) {
                s.copy_from_slice(&crate::flows::JEDEC.to_le_bytes());
            }
            return;
        }
        let sector0 = tag.starts_with(TAG_NORPROG0);
        let status: u32 = if sector0 || tag.starts_with(TAG_NORPROG) {
            let (op, addr, len) = (a[0], a[1], a[2]);
            let span = if op == 1 { SECTOR } else { len };
            let allowed = if sector0 { addr + span <= SECTOR } else { full_writable(addr, span) };
            if !allowed || op == 0 || op > 2 {
                self.violations += usize::from(op == 1 || op == 2);
                0xBAD
            } else if sector0 {
                self.sector0_writes += 1;
                let d = &mut self.nor.data;
                if op == 1 {
                    d[..SECTOR as usize].fill(0xFF);
                } else {
                    let src = self.ram[(BUF - RAM_BASE) as usize..(BUF - RAM_BASE) as usize + len as usize]
                        .to_vec();
                    for (i, b) in src.iter().enumerate() {
                        d[addr as usize + i] &= b;
                    }
                }
                0
            } else {
                let r = if op == 1 {
                    self.nor.erase_sector(addr)
                } else {
                    let src = self.ram[(BUF - RAM_BASE) as usize..(BUF - RAM_BASE) as usize + len as usize]
                        .to_vec();
                    self.nor.program(addr, &src)
                };
                if r.is_ok() { 0 } else { 0x7173 }
            }
        } else {
            0xFFFF
        };
        if self.nor.forbidden > 0 {
            self.violations += self.nor.forbidden;
            self.nor.forbidden = 0;
        }
        self.result = [status, 0, a[0], crate::flows::JEDEC].iter().flat_map(|w| w.to_le_bytes()).collect();
    }

    // -------------------------------------------------- the firmware side

    fn booted_build(&self) -> [u8; 16] {
        let Some(i) = self.booted() else { return [0; 16] };
        let b = orpheus_proto::layout::SLOTS[i] as usize;
        SlotHeader::from_bytes(&self.nor.data[b..b + HEADER_SIZE]).map(|h| h.build_id).unwrap_or([0; 16])
    }

    fn update_out(&mut self, op: u16, index: u16, data: &[u8]) -> bool {
        let r: Result<(), UpdError> = match op {
            wire::OP_BEGIN => (|| {
                if data.len() != wire::BEGIN_LEN {
                    return Err(UpdError::Short);
                }
                let booted = self.booted().map(|i| i as u8);
                let t = slots::choose(true, booted)?;
                let ofw = orpheus_update::ofw::HEADER_SIZE;
                let mut inst = Installer::new(&data[..ofw], &data[ofw..], t)?;
                inst.start(&mut self.nor)?;
                self.inst = Some(inst);
                self.commit = false;
                Ok(())
            })(),
            wire::OP_DATA => match self.inst.as_mut() {
                Some(i) => i.write(&mut self.nor, u32::from(index) * wire::CHUNK as u32, data),
                None => Err(UpdError::State),
            },
            wire::OP_COMMIT => match self.inst.as_ref() {
                Some(i) if i.phase() == Phase::Verifying => {
                    self.commit = true;
                    Ok(())
                }
                _ => Err(UpdError::State),
            },
            wire::OP_ABORT => {
                self.inst = None;
                Ok(())
            }
            _ => Err(UpdError::State),
        };
        self.upd_error = r.err().map_or(0, |e| e as u8);
        r.is_ok()
    }

    /// The firmware's background read-back (its USB supervisor).
    fn update_poll(&mut self) {
        let Some(inst) = self.inst.as_mut() else { return };
        if !self.commit {
            return;
        }
        let mut buf = [0u8; 4096];
        if inst.phase() == Phase::Verifying
            && let Err(e) = inst.verify_step(&mut self.nor, &mut buf)
        {
            self.upd_error = e as u8;
        }
        if inst.phase() == Phase::Commit
            && let Err(e) = inst.commit(&mut self.nor)
        {
            self.upd_error = e as u8;
        }
    }

    fn update_status(&self) -> Status {
        let mut s = Status {
            state: State::Idle,
            error: self.upd_error,
            slot: 0xFF,
            received: 0,
            verified: 0,
            image_len: 0,
        };
        if let Some(i) = &self.inst {
            s.state = match i.phase() {
                Phase::Ready | Phase::Writing => State::Writing,
                Phase::Verifying if self.commit => State::Verifying,
                Phase::Verifying => State::Writing,
                Phase::Commit => State::Committing,
                Phase::Done => State::Done,
                Phase::Failed(e) => {
                    s.error = e as u8;
                    State::Failed
                }
            };
            s.slot = i.target().index;
            s.received = i.received();
            s.verified = i.verified();
            s.image_len = i.image_len();
        } else if s.error != 0 {
            s.state = State::Failed;
        }
        s
    }
}

impl Transport for MockPlayer {
    async fn bulk_out(&mut self, data: &[u8], _timeout_ms: u32) -> Result<()> {
        if self.mode != Mode::Adfu {
            bail!("bulk OUT: no ADFU device");
        }
        if let Some((cdb, n)) = self.pending.take() {
            if data.len() != n {
                bail!("data stage {} bytes, CBW said {n}", data.len());
            }
            self.command(cdb, data);
            return Ok(());
        }
        if data.len() != CBW_LEN || data[..4] != CBW_SIG.to_le_bytes() {
            bail!("not a CBW");
        }
        let n = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
        let mut cdb = [0u8; 16];
        cdb.copy_from_slice(&data[15..31]);
        if data[12] & 0x80 == 0 && n > 0 {
            self.pending = Some((cdb, n));
        } else {
            self.command(cdb, &[]);
        }
        Ok(())
    }

    async fn bulk_in(&mut self, len: usize, _timeout_ms: u32) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        // The device sends data and CSW as separate transfers, or together
        // when the data ends on a packet boundary.
        while let Some(front) = self.replies.front_mut() {
            let room = len - out.len();
            if room == 0 {
                break;
            }
            if front.len() <= room {
                let f = self.replies.pop_front().unwrap_or_default();
                let short = f.len() % 512 != 0;
                out.extend_from_slice(&f);
                if short {
                    break;
                }
            } else {
                out.extend(front.drain(..room));
                break;
            }
        }
        if out.is_empty() {
            bail!("bulk IN: nothing to send");
        }
        Ok(out)
    }

    fn packet_size(&self) -> usize {
        512
    }

    async fn control_in(&mut self, c: Control, len: u16, _timeout_ms: u32) -> Result<Vec<u8>> {
        if self.mode != Mode::Firmware {
            bail!("control IN: the developer channel is not running");
        }
        self.update_poll();
        let mut v = match c.request {
            REQ_INFO => {
                Info { version: VERSION, layout: 2, build_id: self.booted_build(), ..Info::default() }
                    .to_bytes()
                    .to_vec()
            }
            REQ_UPDATE => self.update_status().to_bytes().to_vec(),
            REQ_TEST => b"mock".to_vec(),
            r => bail!("control IN {r:#x}: stalled"),
        };
        v.truncate(usize::from(len));
        Ok(v)
    }

    async fn control_out(&mut self, c: Control, data: &[u8], _timeout_ms: u32) -> Result<()> {
        if self.mode != Mode::Firmware {
            bail!("control OUT: the developer channel is not running");
        }
        match c.request {
            REQ_UPDATE => {
                if self.update_out(c.value, c.index, data) {
                    Ok(())
                } else {
                    bail!("control OUT UPDATE: stalled")
                }
            }
            REQ_REBOOT => {
                if c.value == REBOOT_ADFU {
                    self.mode = Mode::Adfu;
                    self.ymon = false;
                } else {
                    self.boot();
                }
                Ok(())
            }
            REQ_TEST => Ok(()),
            r => bail!("control OUT {r:#x}: stalled"),
        }
    }

    async fn sleep_ms(&mut self, ms: u32) {
        self.clock += u64::from(ms);
        if self.mode == Mode::Firmware {
            self.update_poll();
        }
    }

    fn now_ms(&self) -> u64 {
        self.clock
    }
}

impl MockPlayer {
    /// The whole chip as a backup file would hold it.
    #[must_use]
    pub fn chip(&self) -> Vec<u8> {
        self.nor.data[..NOR_SIZE as usize].to_vec()
    }

    /// Install a slot image directly (test set-up), stage 2 present.
    pub fn with_orpheus(mut self, image: &[u8]) -> Self {
        self.nor.load(full::SLOT_A, image);
        let s2 = orpheus_proto::layout::dual::STAGE2 as usize;
        self.nor.data[s2..s2 + 4].copy_from_slice(&orpheus_proto::layout::STAGE2_MAGIC.to_le_bytes());
        self.boot();
        self
    }
}
