//! Boot ROM ADFU mode and the `ymon` RAM monitor (port of `yoton.py`).
//!
//! Both answer CBWs whose CDB is `cd` + `sub` (LE32) + `len` (LE32) +
//! `addr` (LE32). The ROM implements `0x13` write RAM, `0x93` read RAM (not
//! the ROM itself) and `0x20` jump. `ymon` (loaded at `0x118000`) adds
//! fault-protected payload calls, reads of any address, abort and reset; see
//! `yoton/payloads/ymon.c`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use orpheus_proto::devchan::{ADFU_PID, ADFU_VID};
use orpheus_proto::mem::{YMON_ARGS, YMON_BASE, YMON_PAYLOAD};

use crate::bot::{self, Bot};

pub const SUB_WRITE: u32 = 0x13;
pub const SUB_READ: u32 = 0x93;
pub const SUB_JUMP: u32 = 0x20;
pub const SUB_EXEC: u32 = 0x21;
pub const SUB_RESULT_SIZE: u32 = 0x22;
pub const SUB_RESULT_DATA: u32 = 0x23;
pub const SUB_FAULT: u32 = 0x24;
pub const SUB_EXEC_STATE: u32 = 0x25;
pub const SUB_ABORT: u32 = 0x26;
pub const SUB_PING: u32 = 0x7E;

const T: Duration = Duration::from_secs(5);

/// Where the reference tools and payloads live (read-only).
#[must_use]
pub fn yoton_dir() -> PathBuf {
    std::env::var_os("ORPHEUS_DIR")
        .map_or_else(|| PathBuf::from("/home/tim/Documents/GitHub/yoton"), PathBuf::from)
}

#[must_use]
pub fn payload_path(name: &str) -> PathBuf {
    yoton_dir().join("payloads").join(name)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Rom,
    Ymon,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecState {
    Idle,
    Running,
    Done,
    Faulted,
    Aborted,
    Unknown(u32),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct YmonFault {
    pub magic: u32,
    pub cfsr: u32,
    pub hfsr: u32,
    pub mmfar: u32,
    pub bfar: u32,
    pub pc: u32,
    pub lr: u32,
    pub xpsr: u32,
}

impl std::fmt::Display for YmonFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "pc={:#010x} lr={:#010x} xpsr={:#010x} cfsr={:#010x} hfsr={:#010x} mmfar={:#010x} bfar={:#010x}",
            self.pc, self.lr, self.xpsr, self.cfsr, self.hfsr, self.mmfar, self.bfar
        )
    }
}

pub struct Adfu {
    bot: Bot,
}

fn cdb(sub: u32, len: u32, addr: u32) -> [u8; 16] {
    let mut c = [0u8; 16];
    c[0] = 0xCD;
    c[1..5].copy_from_slice(&sub.to_le_bytes());
    c[5..9].copy_from_slice(&len.to_le_bytes());
    c[9..13].copy_from_slice(&addr.to_le_bytes());
    c
}

impl Adfu {
    /// Open the ADFU device, waiting up to `wait` for it to appear.
    pub fn open(wait: Duration) -> Result<Self> {
        let info = bot::wait_for(ADFU_VID, ADFU_PID, wait)
            .ok_or_else(|| anyhow!("no ADFU device (10d6:10d6) on the bus"))?;
        // The ROM needs a moment after enumeration before it answers.
        std::thread::sleep(Duration::from_millis(300));
        Ok(Self { bot: Bot::open(info)? })
    }

    /// Raw command; returns (CSW status, data).
    pub fn xfer(
        &mut self,
        sub: u32,
        len: u32,
        addr: u32,
        data_out: Option<&[u8]>,
        data_in: usize,
        timeout: Duration,
    ) -> Result<(u8, Vec<u8>)> {
        let r = self.bot.command(&cdb(sub, len, addr), 0, data_out, data_in, timeout)?;
        Ok((r.status, r.data))
    }

    fn xfer_ok(
        &mut self,
        sub: u32,
        len: u32,
        addr: u32,
        out: Option<&[u8]>,
        data_in: usize,
    ) -> Result<Vec<u8>> {
        let (st, d) = self.xfer(sub, len, addr, out, data_in, T)?;
        if st != 0 {
            bail!("ADFU command {sub:#x} at {addr:#010x}: CSW status {st}");
        }
        Ok(d)
    }

    /// Which program answers: the ROM or ymon. `None` if neither responds.
    pub fn mode(&mut self) -> Option<Mode> {
        match self.xfer(SUB_PING, 16, 0, None, 16, Duration::from_millis(800)) {
            Ok((0, d)) if d.get(..4) == Some(b"YMON") => Some(Mode::Ymon),
            Ok((1 | 2, _)) => Some(Mode::Rom),
            Ok(_) | Err(_) => None,
        }
    }

    pub fn read(&mut self, addr: u32, n: usize) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            let c = (n - out.len()).min(0x1_0000);
            let a = addr + out.len() as u32;
            let d = self.xfer_ok(SUB_READ, c as u32, a, None, c)?;
            out.extend_from_slice(&d);
        }
        Ok(out)
    }

    pub fn read_u32(&mut self, addr: u32) -> Result<u32> {
        let d = self.read(addr, 4)?;
        Ok(u32::from_le_bytes([d[0], d[1], d[2], d[3]]))
    }

    /// Write memory in `step`-byte commands (the ROM accepts 0x200; ymon any).
    pub fn write_step(&mut self, addr: u32, data: &[u8], step: usize) -> Result<()> {
        for (i, chunk) in data.chunks(step).enumerate() {
            self.xfer_ok(SUB_WRITE, chunk.len() as u32, addr + (i * step) as u32, Some(chunk), 0)?;
        }
        Ok(())
    }

    pub fn write(&mut self, addr: u32, data: &[u8]) -> Result<()> {
        self.write_step(addr, data, 0x200)
    }

    pub fn write_u32(&mut self, addr: u32, v: u32) -> Result<()> {
        self.write(addr, &v.to_le_bytes())
    }

    /// Write and read back.
    pub fn write_verify(&mut self, addr: u32, data: &[u8], step: usize) -> Result<()> {
        self.write_step(addr, data, step)?;
        let back = self.read(addr, data.len())?;
        if back != data {
            let i = back.iter().zip(data).position(|(a, b)| a != b).unwrap_or(0);
            bail!("verify failed at {:#010x}", addr as usize + i);
        }
        Ok(())
    }

    /// Jump to `addr` (Thumb bit added). Does not return a meaningful reply
    /// for ymon until the jumped-to code returns.
    pub fn jump(&mut self, addr: u32) -> Result<()> {
        let (st, _) = self.xfer(SUB_JUMP, 0, addr | 1, None, 0, T)?;
        if st != 0 {
            bail!("jump to {addr:#010x}: CSW status {st}");
        }
        Ok(())
    }

    /// Make sure ymon runs (upload from the reference repository if the ROM
    /// answers).
    pub fn ensure_ymon(&mut self) -> Result<()> {
        match self.mode() {
            Some(Mode::Ymon) => return Ok(()),
            Some(Mode::Rom) => {}
            None => bail!("ADFU device does not answer (neither ROM nor ymon)"),
        }
        let path = payload_path("ymon.bin");
        let blob = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        self.write_verify(YMON_BASE, &blob, 0x200).context("upload ymon")?;
        self.jump(YMON_BASE)?;
        std::thread::sleep(Duration::from_millis(50));
        if self.mode() != Some(Mode::Ymon) {
            bail!("ymon did not start");
        }
        Ok(())
    }

    pub fn exec_state(&mut self) -> Result<ExecState> {
        let d = self.xfer_ok(SUB_EXEC_STATE, 4, 0, None, 4)?;
        Ok(match u32::from_le_bytes([d[0], d[1], d[2], d[3]]) {
            0 => ExecState::Idle,
            1 => ExecState::Running,
            2 => ExecState::Done,
            3 => ExecState::Faulted,
            4 => ExecState::Aborted,
            x => ExecState::Unknown(x),
        })
    }

    pub fn ymon_fault(&mut self) -> Result<YmonFault> {
        let d = self.xfer_ok(SUB_FAULT, 32, 0, None, 32)?;
        let w = |i: usize| u32::from_le_bytes([d[4 * i], d[4 * i + 1], d[4 * i + 2], d[4 * i + 3]]);
        Ok(YmonFault {
            magic: w(0),
            cfsr: w(1),
            hfsr: w(2),
            mmfar: w(3),
            bfar: w(4),
            pc: w(5),
            lr: w(6),
            xpsr: w(7),
        })
    }

    /// Call a payload already loaded at `addr` with up to 16 argument words
    /// (at `0x11F800`); returns the `{addr, size}` result it points to.
    pub fn call(&mut self, addr: u32, args: &[u32], timeout: Duration) -> Result<Vec<u8>> {
        if !args.is_empty() {
            let b: Vec<u8> = args.iter().flat_map(|a| a.to_le_bytes()).collect();
            self.write(YMON_ARGS, &b)?;
        }
        let (st, _) = self.xfer(SUB_EXEC, 0, addr | 1, None, 0, T)?;
        if st != 0 {
            bail!("payload start: CSW status {st}");
        }
        let t0 = Instant::now();
        let state = loop {
            let s = self.exec_state()?;
            if s != ExecState::Running {
                break s;
            }
            if t0.elapsed() > timeout {
                let _ = self.xfer(SUB_ABORT, 0, 0, None, 0, T);
                bail!("payload timed out after {timeout:?}; aborted");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        match state {
            ExecState::Done => {}
            ExecState::Faulted => bail!("payload faulted: {}", self.ymon_fault()?),
            s => bail!("payload ended in state {s:?}"),
        }
        let d = self.xfer_ok(SUB_RESULT_SIZE, 4, 0, None, 4)?;
        let size = u32::from_le_bytes([d[0], d[1], d[2], d[3]]) as usize;
        if size == 0 {
            return Ok(Vec::new());
        }
        let (st, data) = self.xfer(SUB_RESULT_DATA, size as u32, 0, None, size, timeout)?;
        if st != 0 {
            bail!("payload result: CSW status {st}");
        }
        Ok(data)
    }

    pub fn load_payload(&mut self, blob: &[u8]) -> Result<()> {
        self.write_verify(YMON_PAYLOAD, blob, 0x1000)
    }

    pub fn load_payload_file(&mut self, path: &Path) -> Result<()> {
        let blob = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        self.load_payload(&blob)
    }

    /// System reset through ymon/ROM (`0xB0` CDB); the device re-enumerates.
    pub fn reset(mut self) {
        let _ = self.bot.command(&[0xB0], 0, None, 0, Duration::from_secs(1));
    }

    /// Hand the device back (drop the claim) without resetting it.
    pub fn close(self) {}
}
