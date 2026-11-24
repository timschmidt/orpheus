//! SCSI pass-through to a mass-storage device through the kernel (`SG_IO`),
//! leaving `usb-storage` bound. Used to ask the Rockbox port or the stock
//! firmware to reboot into ADFU (`CC` identify, then `CB 21`).

use std::fs::OpenOptions;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const SG_IO: libc::c_ulong = 0x2285;
const SG_DXFER_NONE: libc::c_int = -1;
const SG_DXFER_FROM_DEV: libc::c_int = -3;

#[repr(C)]
struct SgIoHdr {
    interface_id: libc::c_int,
    dxfer_direction: libc::c_int,
    cmd_len: libc::c_uchar,
    mx_sb_len: libc::c_uchar,
    iovec_count: libc::c_ushort,
    dxfer_len: libc::c_uint,
    dxferp: *mut libc::c_void,
    cmdp: *const libc::c_uchar,
    sbp: *mut libc::c_uchar,
    timeout: libc::c_uint,
    flags: libc::c_uint,
    pack_id: libc::c_int,
    usr_ptr: *mut libc::c_void,
    status: libc::c_uchar,
    masked_status: libc::c_uchar,
    msg_status: libc::c_uchar,
    sb_len_wr: libc::c_uchar,
    host_status: libc::c_ushort,
    driver_status: libc::c_ushort,
    resid: libc::c_int,
    duration: libc::c_uint,
    info: libc::c_uint,
}

/// `/dev/sgN` nodes whose vendor string contains one of `vendors`.
#[must_use]
pub fn find_sg(vendors: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/sys/class/scsi_generic") else {
        return out;
    };
    for e in rd.flatten() {
        let v = std::fs::read_to_string(e.path().join("device/vendor")).unwrap_or_default();
        if vendors.iter().any(|x| v.contains(x)) {
            out.push(Path::new("/dev").join(e.file_name()));
        }
    }
    out.sort();
    out
}

/// Send a CDB reading up to `data_in` bytes. Returns (SCSI status, data).
pub fn command(dev: &Path, cdb: &[u8], data_in: usize) -> Result<(u8, Vec<u8>)> {
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(dev)
        .with_context(|| format!("open {}", dev.display()))?;
    let mut buf = vec![0u8; data_in];
    let mut sense = [0u8; 32];
    let mut h = SgIoHdr {
        interface_id: libc::c_int::from(b'S'),
        dxfer_direction: if data_in > 0 { SG_DXFER_FROM_DEV } else { SG_DXFER_NONE },
        cmd_len: cdb.len() as u8,
        mx_sb_len: sense.len() as u8,
        iovec_count: 0,
        dxfer_len: data_in as u32,
        dxferp: if data_in > 0 { buf.as_mut_ptr().cast() } else { std::ptr::null_mut() },
        cmdp: cdb.as_ptr(),
        sbp: sense.as_mut_ptr(),
        timeout: 5000,
        flags: 0,
        pack_id: 0,
        usr_ptr: std::ptr::null_mut(),
        status: 0,
        masked_status: 0,
        msg_status: 0,
        sb_len_wr: 0,
        host_status: 0,
        driver_status: 0,
        resid: 0,
        duration: 0,
        info: 0,
    };
    // SAFETY: `h` describes buffers (`buf`, `sense`, `cdb`) that outlive the
    // call and whose lengths are given in the header; SG_IO is synchronous.
    let rc = unsafe { libc::ioctl(f.as_raw_fd(), SG_IO, &raw mut h) };
    if rc < 0 {
        bail!("SG_IO on {}: {}", dev.display(), std::io::Error::last_os_error());
    }
    let n = data_in.saturating_sub(usize::try_from(h.resid).unwrap_or(0));
    buf.truncate(n);
    Ok((h.status, buf))
}

/// Ask an Actions-based firmware in mass-storage mode to reboot into the boot
/// ROM's ADFU mode. Refuses unless the identify command answers
/// `ACTIONSUSBD`.
pub fn reboot_to_adfu(dev: &Path) -> Result<()> {
    let mut cdb = [0u8; 16];
    cdb[0] = 0xCC;
    cdb[7] = 11;
    let (_, id) = command(dev, &cdb, 11)?;
    if id != b"ACTIONSUSBD" {
        bail!("{}: identify answered {id:02x?}, not ACTIONSUSBD; not rebooting", dev.display());
    }
    let mut cdb = [0u8; 16];
    cdb[0] = 0xCB;
    cdb[1] = 0x21;
    cdb[7] = 2;
    // The device may reset before completing the status phase.
    let _ = command(dev, &cdb, 2);
    Ok(())
}
