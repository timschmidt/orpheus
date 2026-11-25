//! USB mass-storage Bulk-Only Transport framing, as spoken by the boot
//! ROM's ADFU mode and by `ymon` (both frame their commands as CBW/CSW),
//! over any [`Transport`].

use anyhow::{Result, bail};

use crate::transport::Transport;

pub const CBW_SIG: u32 = 0x4342_5355; // "USBC"
pub const CSW_SIG: u32 = 0x5342_5355; // "USBS"
pub const CSW_LEN: usize = 13;
pub const CBW_LEN: usize = 31;

#[cfg(feature = "native")]
pub use crate::native::{find, wait_for, wait_gone};

/// Status and data of one command.
pub struct Reply {
    pub status: u8,
    pub data: Vec<u8>,
}

/// The 31-byte command block wrapper.
#[must_use]
pub fn cbw(cdb: &[u8], tag: u32, data_len: usize, data_in: bool) -> [u8; CBW_LEN] {
    let mut c = [0u8; CBW_LEN];
    c[0..4].copy_from_slice(&CBW_SIG.to_le_bytes());
    c[4..8].copy_from_slice(&tag.to_le_bytes());
    c[8..12].copy_from_slice(&(data_len as u32).to_le_bytes());
    c[12] = if data_in { 0x80 } else { 0 };
    c[14] = 16;
    let n = cdb.len().min(16);
    c[15..15 + n].copy_from_slice(&cdb[..n]);
    c
}

fn is_csw(b: &[u8]) -> bool {
    b.len() == CSW_LEN && b[..4] == CSW_SIG.to_le_bytes()
}

/// One CBW/data/CSW exchange. `cdb` ≤ 16 bytes; `tag` 0 for ADFU.
pub async fn command<T: Transport>(
    t: &mut T,
    cdb: &[u8],
    tag: u32,
    data_out: Option<&[u8]>,
    data_in: usize,
    timeout_ms: u32,
) -> Result<Reply> {
    let dlen = data_out.map_or(data_in, <[u8]>::len);
    t.bulk_out(&cbw(cdb, tag, dlen, data_in > 0), timeout_ms).await?;
    if let Some(d) = data_out
        && !d.is_empty()
    {
        t.bulk_out(d, timeout_ms).await?;
    }
    let mps = t.packet_size().max(1);
    let read = async |t: &mut T, want: usize| -> Result<Vec<u8>> {
        let req = want.div_ceil(mps).max(1) * mps;
        match t.bulk_in(req, timeout_ms).await {
            Ok(v) => Ok(v),
            Err(e) => {
                t.clear_halt_in().await;
                Err(e)
            }
        }
    };
    let (data, csw) = if data_in > 0 {
        // Data and CSW may arrive in one transfer (when the data ends on a
        // packet boundary) or in two.
        let mut buf = Vec::with_capacity(data_in + CSW_LEN);
        while buf.len() < data_in {
            let chunk = read(t, data_in + CSW_LEN - buf.len()).await?;
            if chunk.is_empty() {
                bail!("bulk IN returned nothing");
            }
            buf.extend_from_slice(&chunk);
            if chunk.len() % mps != 0 {
                break;
            }
        }
        if buf.len() < data_in + CSW_LEN {
            if buf.len() == CSW_LEN && data_in != CSW_LEN && is_csw(&buf) {
                // Device answered with status only (error, no data).
                return Ok(Reply { status: buf[12], data: Vec::new() });
            }
            let csw = read(t, CSW_LEN).await?;
            buf.extend_from_slice(&csw);
        }
        if buf.len() < data_in + CSW_LEN {
            bail!("short reply: {} bytes for {data_in} + CSW", buf.len());
        }
        let csw = buf.split_off(buf.len() - CSW_LEN);
        buf.truncate(data_in);
        (buf, csw)
    } else {
        (Vec::new(), read(t, CSW_LEN).await?)
    };
    if !is_csw(&csw) {
        bail!("bad CSW {csw:02x?}");
    }
    Ok(Reply { status: csw[12], data })
}
