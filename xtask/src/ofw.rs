//! The update file (UPD-1, 02 §2.5): `cargo xtask build` writes
//! `target/orpheus/orpheus-<version>.ofw` next to `firmware.yfw` for
//! full-layout builds — the `.ofw` header (version, build id, SHA-256,
//! layout, change list) and the slot image (`orpheus_update::ofw`).
//!
//! The change list is `target/orpheus/changes.txt` if present (the release
//! process writes it), else one line naming the commit.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use orpheus_proto::slot::{LAYOUT_FULL, SlotHeader};
use orpheus_update::ofw::{self, Header, VERSION_LEN};
use sha2::{Digest, Sha256};

use crate::util::{git_commit, git_date, out_dir};

/// `0.1.0+1a2b3c4d` (at most 16 bytes, printable).
pub fn version() -> String {
    let commit: String = git_commit().chars().filter(char::is_ascii_hexdigit).take(8).collect();
    let mut v = format!("{}+{commit}", env!("CARGO_PKG_VERSION"));
    v.truncate(VERSION_LEN);
    v
}

/// Header + image.
pub fn build(image: &[u8], version: &str, changes: &str) -> Result<Vec<u8>> {
    let s = SlotHeader::from_bytes(image).map_err(|e| anyhow!("slot header: {e:?}"))?;
    let h = Header {
        layout: s.layout,
        image_len: u32::try_from(image.len())?,
        changes_len: 0,
        version: ofw::version_bytes(version),
        build_id: s.build_id,
        sha256: Sha256::digest(image).into(),
    };
    let mut out = h.to_bytes(changes).to_vec();
    out.extend_from_slice(image);
    ofw::check(&out[..ofw::HEADER_SIZE], &out[ofw::HEADER_SIZE..]).map_err(|e| anyhow!("{}", e.text()))?;
    Ok(out)
}

/// Write the `.ofw` for a built slot image; `None` for dual-layout builds
/// (the player cannot update those itself).
pub fn write_for(yfw: &Path) -> Result<Option<PathBuf>> {
    let image = std::fs::read(yfw).with_context(|| format!("read {}", yfw.display()))?;
    let s = SlotHeader::from_bytes(&image).map_err(|e| anyhow!("slot header: {e:?}"))?;
    if s.layout != LAYOUT_FULL {
        return Ok(None);
    }
    let v = version();
    let changes = std::fs::read_to_string(out_dir().join("changes.txt"))
        .unwrap_or_else(|_| format!("Development build {} ({})", git_commit(), git_date()));
    let file = build(&image, &v, changes.trim())?;
    let path = out_dir().join(format!("orpheus-{v}.ofw"));
    std::fs::write(&path, file).with_context(|| format!("write {}", path.display()))?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_file_checks() {
        let img = orpheus_update::sim::image(3, 0x2000, 1);
        let f = build(&img, "0.1.0+abcdef12", "One\nTwo").unwrap();
        let (h, _) = orpheus_host::devchan::check_file(&f).unwrap();
        assert_eq!(h.version_str(), Some("0.1.0+abcdef12"));
        assert_eq!(ofw::changes(&f), "One\nTwo");
        assert!(version().len() <= VERSION_LEN);
    }
}
