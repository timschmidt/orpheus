//! Boot ROM ADFU mode and the `ymon` RAM monitor (port of `yoton.py`).
//!
//! Both answer CBWs whose CDB is `cd` + `sub` (LE32) + `len` (LE32) +
//! `addr` (LE32). The ROM implements `0x13` write RAM, `0x93` read RAM (not
//! the ROM itself) and `0x20` jump. `ymon` (loaded at `0x118000`) adds
//! fault-protected payload calls, reads of any address, abort and reset; see
//! `yoton/payloads/ymon.c`.
//!
//! [`Link`] is the protocol over any [`Transport`] (desktop, browser,
//! mock); [`Adfu`] is the desktop's blocking front end over `nusb`.

use std::fmt;

use anyhow::{Result, anyhow, bail};
use orpheus_proto::mem::{YMON_ARGS, YMON_BASE, YMON_PAYLOAD};

use crate::bot;
use crate::transport::Transport;

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
/// The reset CDB (`0xB0`) ymon and the ROM answer by resetting.
pub const CDB_RESET: u8 = 0xB0;

const T_MS: u32 = 5000;

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

impl fmt::Display for YmonFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pc={:#010x} lr={:#010x} xpsr={:#010x} cfsr={:#010x} hfsr={:#010x} mmfar={:#010x} bfar={:#010x}",
            self.pc, self.lr, self.xpsr, self.cfsr, self.hfsr, self.mmfar, self.bfar
        )
    }
}

/// The ADFU command block.
#[must_use]
pub fn cdb(sub: u32, len: u32, addr: u32) -> [u8; 16] {
    let mut c = [0u8; 16];
    c[0] = 0xCD;
    c[1..5].copy_from_slice(&sub.to_le_bytes());
    c[5..9].copy_from_slice(&len.to_le_bytes());
    c[9..13].copy_from_slice(&addr.to_le_bytes());
    c
}

/// The ROM/ymon protocol over a transport.
pub struct Link<T: Transport> {
    pub t: T,
}

impl<T: Transport> Link<T> {
    pub fn new(t: T) -> Self {
        Self { t }
    }

    /// Raw command; returns (CSW status, data).
    pub async fn xfer(
        &mut self,
        sub: u32,
        len: u32,
        addr: u32,
        data_out: Option<&[u8]>,
        data_in: usize,
        timeout_ms: u32,
    ) -> Result<(u8, Vec<u8>)> {
        let r = bot::command(&mut self.t, &cdb(sub, len, addr), 0, data_out, data_in, timeout_ms).await?;
        Ok((r.status, r.data))
    }

    async fn xfer_ok(
        &mut self,
        sub: u32,
        len: u32,
        addr: u32,
        out: Option<&[u8]>,
        data_in: usize,
    ) -> Result<Vec<u8>> {
        let (st, d) = self.xfer(sub, len, addr, out, data_in, T_MS).await?;
        if st != 0 {
            bail!("ADFU command {sub:#x} at {addr:#010x}: CSW status {st}");
        }
        Ok(d)
    }

    /// Which program answers: the ROM or ymon. `None` if neither responds.
    pub async fn mode(&mut self) -> Option<Mode> {
        match self.xfer(SUB_PING, 16, 0, None, 16, 800).await {
            Ok((0, d)) if d.get(..4) == Some(b"YMON") => Some(Mode::Ymon),
            Ok((1 | 2, _)) => Some(Mode::Rom),
            Ok(_) | Err(_) => None,
        }
    }

    pub async fn read(&mut self, addr: u32, n: usize) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(n);
        while out.len() < n {
            let c = (n - out.len()).min(0x1_0000);
            let a = addr + out.len() as u32;
            let d = self.xfer_ok(SUB_READ, c as u32, a, None, c).await?;
            out.extend_from_slice(&d);
        }
        Ok(out)
    }

    pub async fn read_u32(&mut self, addr: u32) -> Result<u32> {
        let d = self.read(addr, 4).await?;
        Ok(u32::from_le_bytes([d[0], d[1], d[2], d[3]]))
    }

    /// Write memory in `step`-byte commands (the ROM accepts 0x200; ymon any).
    pub async fn write_step(&mut self, addr: u32, data: &[u8], step: usize) -> Result<()> {
        for (i, chunk) in data.chunks(step).enumerate() {
            self.xfer_ok(SUB_WRITE, chunk.len() as u32, addr + (i * step) as u32, Some(chunk), 0).await?;
        }
        Ok(())
    }

    pub async fn write(&mut self, addr: u32, data: &[u8]) -> Result<()> {
        self.write_step(addr, data, 0x200).await
    }

    pub async fn write_u32(&mut self, addr: u32, v: u32) -> Result<()> {
        self.write(addr, &v.to_le_bytes()).await
    }

    /// Write and read back.
    pub async fn write_verify(&mut self, addr: u32, data: &[u8], step: usize) -> Result<()> {
        self.write_step(addr, data, step).await?;
        let back = self.read(addr, data.len()).await?;
        if back != data {
            let i = back.iter().zip(data).position(|(a, b)| a != b).unwrap_or(0);
            bail!("verify failed at {:#010x}", addr as usize + i);
        }
        Ok(())
    }

    /// Jump to `addr` (Thumb bit added).
    pub async fn jump(&mut self, addr: u32) -> Result<()> {
        let (st, _) = self.xfer(SUB_JUMP, 0, addr | 1, None, 0, T_MS).await?;
        if st != 0 {
            bail!("jump to {addr:#010x}: CSW status {st}");
        }
        Ok(())
    }

    /// Make sure ymon runs: upload `ymon` (the monitor's image) if the ROM
    /// answers.
    pub async fn ensure_ymon(&mut self, ymon: &[u8]) -> Result<()> {
        match self.mode().await {
            Some(Mode::Ymon) => return Ok(()),
            Some(Mode::Rom) => {}
            None => bail!("ADFU device does not answer (neither ROM nor ymon)"),
        }
        self.write_verify(YMON_BASE, ymon, 0x200).await.map_err(|e| anyhow!("upload ymon: {e:#}"))?;
        self.jump(YMON_BASE).await?;
        self.t.sleep_ms(50).await;
        if self.mode().await != Some(Mode::Ymon) {
            bail!("ymon did not start");
        }
        Ok(())
    }

    pub async fn exec_state(&mut self) -> Result<ExecState> {
        let d = self.xfer_ok(SUB_EXEC_STATE, 4, 0, None, 4).await?;
        Ok(match u32::from_le_bytes([d[0], d[1], d[2], d[3]]) {
            0 => ExecState::Idle,
            1 => ExecState::Running,
            2 => ExecState::Done,
            3 => ExecState::Faulted,
            4 => ExecState::Aborted,
            x => ExecState::Unknown(x),
        })
    }

    pub async fn ymon_fault(&mut self) -> Result<YmonFault> {
        let d = self.xfer_ok(SUB_FAULT, 32, 0, None, 32).await?;
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
    pub async fn call(&mut self, addr: u32, args: &[u32], timeout_ms: u32) -> Result<Vec<u8>> {
        if !args.is_empty() {
            let b: Vec<u8> = args.iter().flat_map(|a| a.to_le_bytes()).collect();
            self.write(YMON_ARGS, &b).await?;
        }
        let (st, _) = self.xfer(SUB_EXEC, 0, addr | 1, None, 0, T_MS).await?;
        if st != 0 {
            bail!("payload start: CSW status {st}");
        }
        let t0 = self.t.now_ms();
        let state = loop {
            let s = self.exec_state().await?;
            if s != ExecState::Running {
                break s;
            }
            if self.t.now_ms() - t0 > u64::from(timeout_ms) {
                let _ = self.xfer(SUB_ABORT, 0, 0, None, 0, T_MS).await;
                bail!("payload timed out after {timeout_ms} ms; aborted");
            }
            self.t.sleep_ms(5).await;
        };
        match state {
            ExecState::Done => {}
            ExecState::Faulted => bail!("payload faulted: {}", self.ymon_fault().await?),
            s => bail!("payload ended in state {s:?}"),
        }
        let d = self.xfer_ok(SUB_RESULT_SIZE, 4, 0, None, 4).await?;
        let size = u32::from_le_bytes([d[0], d[1], d[2], d[3]]) as usize;
        if size == 0 {
            return Ok(Vec::new());
        }
        let (st, data) = self.xfer(SUB_RESULT_DATA, size as u32, 0, None, size, timeout_ms).await?;
        if st != 0 {
            bail!("payload result: CSW status {st}");
        }
        Ok(data)
    }

    pub async fn load_payload(&mut self, blob: &[u8]) -> Result<()> {
        self.write_verify(YMON_PAYLOAD, blob, 0x1000).await
    }

    /// System reset through ymon/ROM (`0xB0` CDB); the device re-enumerates.
    pub async fn reset(&mut self) {
        let _ = bot::command(&mut self.t, &[CDB_RESET], 0, None, 0, 1000).await;
    }
}

#[cfg(feature = "native")]
pub use crate::native::{Adfu, payload_path, yoton_dir};
