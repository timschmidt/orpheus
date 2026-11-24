//! Developer channel: vendor requests on endpoint 0
//! (docs/plan/06-connectivity.md §2.5).

/// Protocol version reported by `INFO`.
pub const VERSION: u16 = 1;

/// USB identity in development builds (decision D-3: pid.codes test id).
pub const DEV_VID: u16 = 0x1209;
pub const DEV_PID: u16 = 0x0001;

/// Boot ROM recovery mode.
pub const ADFU_VID: u16 = 0x10D6;
pub const ADFU_PID: u16 = 0x10D6;
/// Rockbox port in mass-storage mode; stock firmware.
pub const ROCKBOX_PID: u16 = 0x1157;
pub const STOCK_PID: u16 = 0x1101;

pub const REQ_INFO: u8 = 0x01;
pub const REQ_LOG: u8 = 0x02;
pub const REQ_FRAME: u8 = 0x03;
pub const REQ_KEYS: u8 = 0x04;
pub const REQ_STATS: u8 = 0x05;
pub const REQ_TEST: u8 = 0x06;
pub const REQ_REBOOT: u8 = 0x07;
pub const REQ_TIME: u8 = 0x08;
pub const REQ_SET: u8 = 0x09;
pub const REQ_PEEK: u8 = 0x0A;
pub const REQ_PROVISION: u8 = 0x0B;
/// OUT: append the data stage (≤ 4 KiB) to the file opened with the
/// `fcreate` test command (only under `/.orpheus/`; development builds).
pub const REQ_PUT: u8 = 0x0C;

/// `REBOOT` targets (`wValue`).
pub const REBOOT_NORMAL: u16 = 0;
pub const REBOOT_ADFU: u16 = 1;
pub const REBOOT_STOCK: u16 = 2;
pub const REBOOT_OTHER_SLOT: u16 = 3;

/// Largest data stage the host asks for until U-USB-2 settles multi-packet
/// control transfers.
pub const SAFE_CHUNK: u16 = 64;

/// `INFO` response (little-endian, 32 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Info {
    pub version: u16,
    pub layout: u8,
    pub personality: u8,
    pub uptime_ms: u32,
    pub battery_mv: u16,
    pub flags: u16,
    pub build_id: [u8; 16],
    pub reserved: u32,
}

impl Info {
    pub const SIZE: usize = 32;

    #[must_use]
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0..2].copy_from_slice(&self.version.to_le_bytes());
        b[2] = self.layout;
        b[3] = self.personality;
        b[4..8].copy_from_slice(&self.uptime_ms.to_le_bytes());
        b[8..10].copy_from_slice(&self.battery_mv.to_le_bytes());
        b[10..12].copy_from_slice(&self.flags.to_le_bytes());
        b[12..28].copy_from_slice(&self.build_id);
        b[28..32].copy_from_slice(&self.reserved.to_le_bytes());
        b
    }

    #[must_use]
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let b: &[u8; Self::SIZE] = b.get(..Self::SIZE)?.try_into().ok()?;
        let mut build_id = [0u8; 16];
        build_id.copy_from_slice(&b[12..28]);
        Some(Self {
            version: u16::from_le_bytes([b[0], b[1]]),
            layout: b[2],
            personality: b[3],
            uptime_ms: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            battery_mv: u16::from_le_bytes([b[8], b[9]]),
            flags: u16::from_le_bytes([b[10], b[11]]),
            build_id,
            reserved: u32::from_le_bytes([b[28], b[29], b[30], b[31]]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_roundtrip() {
        let i = Info { version: VERSION, layout: 1, uptime_ms: 1234, battery_mv: 3900, ..Default::default() };
        assert_eq!(Info::from_bytes(&i.to_bytes()), Some(i));
    }
}
