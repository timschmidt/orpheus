//! What an image export may be (06-connectivity §3, 05 §7.10): the
//! medium, read-only or not, and the checks every request passes before a
//! byte of the card is shown to a computer. Shared by the firmware (the
//! `usb image` developer command and the USB Drive app) and the
//! simulator.
//!
//! The rules (CLAUDE.md rule 5):
//! * a CD-ROM is always read-only;
//! * a disk is read-only unless writing was asked for, the file is under
//!   `/.orpheus/`, and the player is in test mode;
//! * a writable export writes only inside the file's own runs
//!   ([`super::extent::ImageDevice`]), never past them.

use orpheus_fs::{BlockDevice as FsDevice, File, Volume};

use super::extent::{ExtentTable, SECTOR, TooFragmented};
use super::mmc::CD_BLOCK;
use super::scsi::{DeviceKind, Unit};

/// How the image is presented (06 §3.2, §3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Medium {
    /// Peripheral type 5, 2048-byte blocks.
    Cd,
    /// Direct access, removable (RMB = 1).
    #[default]
    RemovableDisk,
    /// Direct access, fixed (RMB = 0; some boot menus list only these).
    FixedDisk,
}

impl Medium {
    /// The default for a file name: CD-ROM for `.iso`, a removable disk
    /// otherwise.
    #[must_use]
    pub fn for_name(name: &str) -> Medium {
        if ext_of(name).is_some_and(|e| e.eq_ignore_ascii_case("iso")) {
            Medium::Cd
        } else {
            Medium::RemovableDisk
        }
    }

    #[must_use]
    pub const fn block_size(self) -> u32 {
        match self {
            Medium::Cd => CD_BLOCK,
            _ => SECTOR,
        }
    }

    #[must_use]
    pub const fn is_cd(self) -> bool {
        matches!(self, Medium::Cd)
    }

    /// Developer-command word: `cd`, `disk`, `fixed`.
    #[must_use]
    pub fn parse(w: &str) -> Option<Medium> {
        match w {
            "cd" => Some(Medium::Cd),
            "disk" => Some(Medium::RemovableDisk),
            "fixed" => Some(Medium::FixedDisk),
            _ => None,
        }
    }

    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Medium::Cd => "cd",
            Medium::RemovableDisk => "disk",
            Medium::FixedDisk => "fixed",
        }
    }
}

fn ext_of(name: &str) -> Option<&str> {
    name.rsplit_once('.').map(|(_, e)| e)
}

/// File extensions the USB Drive app lists (05 §7.10).
pub const IMAGE_EXTS: &[&str] = &["iso", "img", "bin", "raw", "vhd"];

/// The name has one of [`IMAGE_EXTS`].
#[must_use]
pub fn is_image_name(name: &str) -> bool {
    ext_of(name).is_some_and(|e| IMAGE_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// The folders the USB Drive app lists: the user's, and the one tests put
/// their images in (the only place a writable image may be).
pub const USER_DIR: &str = "/Images";
pub const TEST_DIR: &str = "/.orpheus/images";

/// The only top-level folder Orpheus writes (CLAUDE.md rule 5).
const OWN_DIR: &str = ".orpheus";

/// `path` names something inside `/.orpheus/`: absolute, the first
/// component `.orpheus` (ASCII case-insensitive, as FAT compares names),
/// at least one more component, and no empty, `.` or `..` component that
/// could lead back out.
#[must_use]
pub fn under_orpheus(path: &str) -> bool {
    let Some(rest) = path.strip_prefix('/') else { return false };
    let mut parts = rest.split('/');
    if !parts.next().is_some_and(|p| p.eq_ignore_ascii_case(OWN_DIR)) {
        return false;
    }
    let mut any = false;
    for p in parts {
        if p.is_empty() || p == "." || p == ".." {
            return false;
        }
        any = true;
    }
    any
}

/// What was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Request {
    pub medium: Medium,
    /// Let the computer write (disk only).
    pub write: bool,
}

/// Why an export is refused (each has a message in the app and the
/// developer channel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A CD-ROM is never writable.
    CdReadOnly,
    /// Writable only for files under `/.orpheus/`.
    WriteOutsideOrpheus,
    /// Writable only in test mode.
    WriteNeedsTestMode,
    /// Smaller than one block of the medium.
    TooSmall,
    /// Larger than the extent table can address (2 TiB).
    TooLarge,
    /// An exFAT file whose valid length is below its size (preallocated,
    /// not fully written): the rest of its clusters hold old data.
    NotFullyWritten,
    /// More runs on the card than the extent table holds.
    TooFragmented,
    /// The file system failed or is inconsistent.
    Io,
    /// Not a file, or not there.
    NotFound,
}

impl Refusal {
    /// Short text for the developer channel.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Refusal::CdReadOnly => "a CD is read-only",
            Refusal::WriteOutsideOrpheus => "writable only under /.orpheus/",
            Refusal::WriteNeedsTestMode => "writable only in test mode",
            Refusal::TooSmall => "smaller than one block",
            Refusal::TooLarge => "too large",
            Refusal::NotFullyWritten => "file not fully written",
            Refusal::TooFragmented => "too fragmented",
            Refusal::Io => "card error",
            Refusal::NotFound => "no such file",
        }
    }
}

/// An export that passed [`check`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Export {
    pub medium: Medium,
    pub read_only: bool,
    /// The file's size.
    pub bytes: u64,
}

/// Check a request against the rules. `under_orpheus`: the file is inside
/// `/.orpheus/` (by path, [`under_orpheus`], or by its folder); `valid`:
/// the bytes of the file that hold data (exFAT's valid length; the size on
/// FAT).
pub fn check(
    req: Request,
    under_orpheus: bool,
    test_mode: bool,
    bytes: u64,
    valid: u64,
) -> Result<Export, Refusal> {
    if req.write {
        if req.medium.is_cd() {
            return Err(Refusal::CdReadOnly);
        }
        if !under_orpheus {
            return Err(Refusal::WriteOutsideOrpheus);
        }
        if !test_mode {
            return Err(Refusal::WriteNeedsTestMode);
        }
    }
    if bytes < u64::from(req.medium.block_size()) {
        return Err(Refusal::TooSmall);
    }
    if bytes.div_ceil(u64::from(SECTOR)) > u64::from(u32::MAX) {
        return Err(Refusal::TooLarge);
    }
    if valid < bytes {
        return Err(Refusal::NotFullyWritten);
    }
    Ok(Export { medium: req.medium, read_only: !req.write, bytes })
}

impl Export {
    /// The logical unit to present.
    #[must_use]
    pub fn unit(&self) -> Unit {
        let vendor = *b"Orpheus ";
        let revision = *b"0001";
        match self.medium {
            Medium::Cd => Unit::cdrom(self.bytes, vendor, *b"USB Drive CD-ROM", revision),
            m => Unit {
                kind: DeviceKind::Disk,
                removable: m == Medium::RemovableDisk,
                block_size: SECTOR,
                blocks: self.bytes / u64::from(SECTOR),
                read_only: self.read_only,
                present: true,
                vendor,
                product: *b"USB Drive disk  ",
                revision,
            },
        }
    }
}

/// Map `f`'s runs on the card into `table` (empty on entry), in file
/// order. Merges neighbouring runs; [`Refusal::TooFragmented`] when the
/// table is full.
pub async fn map_file<D: FsDevice>(
    vol: &mut Volume<'_, D>,
    f: &File,
    table: &mut ExtentTable<'_>,
) -> Result<(), Refusal> {
    let mut it = vol.extents(f);
    loop {
        match vol.next_extent(&mut it).await {
            Ok(Some(e)) => {
                if e.offset != u64::from(table.sectors()) * u64::from(SECTOR) {
                    return Err(Refusal::Io);
                }
                table.push(e.lba, e.sectors).map_err(|TooFragmented| Refusal::TooFragmented)?;
            }
            Ok(None) => break,
            Err(_) => return Err(Refusal::Io),
        }
    }
    if u64::from(table.sectors()) * u64::from(SECTOR) < f.size() { Err(Refusal::Io) } else { Ok(()) }
}

/// Parsed `usb image <path> [cd|disk|fixed] [ro|rw]` (words after
/// `image`). The medium defaults by the name, access to read-only.
#[must_use]
pub fn parse_command(args: &str) -> Option<(&str, Request)> {
    let mut words = args.split(' ').filter(|w| !w.is_empty());
    let path = words.next()?;
    let mut req = Request { medium: Medium::for_name(path), write: false };
    for w in words {
        match w {
            "ro" => req.write = false,
            "rw" => req.write = true,
            _ => req.medium = Medium::parse(w)?,
        }
    }
    path.starts_with('/').then_some((path, req))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_by_name() {
        assert_eq!(Medium::for_name("Ubuntu 24.04.ISO"), Medium::Cd);
        assert_eq!(Medium::for_name("boot.img"), Medium::RemovableDisk);
        assert_eq!(Medium::for_name("noext"), Medium::RemovableDisk);
        assert!(is_image_name("a.vhd") && is_image_name("B.Raw") && !is_image_name("a.mp3"));
        assert!(!is_image_name("iso"));
    }

    #[test]
    fn orpheus_paths() {
        for p in ["/.orpheus/images/a.img", "/.ORPHEUS/x", "/.orpheus/test/sub/b.iso"] {
            assert!(under_orpheus(p), "{p}");
        }
        for p in [
            "/.orpheus",
            "/.orpheus/",
            "/Images/a.img",
            ".orpheus/a.img",
            "/.orpheus/../Music/a.img",
            "/.orpheus/./a.img",
            "/.orpheus//a.img",
            "/.orpheus2/a.img",
            "/x/.orpheus/a.img",
            "",
        ] {
            assert!(!under_orpheus(p), "{p}");
        }
    }

    #[test]
    fn policy() {
        let cd = Request { medium: Medium::Cd, write: false };
        let disk = Request { medium: Medium::RemovableDisk, write: false };
        let rw = Request { medium: Medium::RemovableDisk, write: true };
        let e = check(cd, false, false, 5000, 5000).unwrap();
        assert!(e.read_only);
        assert_eq!(e.unit().blocks, 3, "CD tail padded");
        assert!(e.unit().read_only);
        let e = check(disk, false, false, 5000, 5000).unwrap();
        assert!(e.read_only && e.unit().read_only);
        assert_eq!(e.unit().blocks, 9, "disk tail not addressable");
        assert!(e.unit().removable);
        // Writable: only under /.orpheus/ in test mode, never a CD.
        assert_eq!(check(Request { write: true, ..cd }, true, true, 4096, 4096), Err(Refusal::CdReadOnly));
        assert_eq!(check(rw, false, true, 4096, 4096), Err(Refusal::WriteOutsideOrpheus));
        assert_eq!(check(rw, true, false, 4096, 4096), Err(Refusal::WriteNeedsTestMode));
        let e = check(rw, true, true, 4096, 4096).unwrap();
        assert!(!e.read_only && !e.unit().read_only);
        let fixed = Request { medium: Medium::FixedDisk, write: false };
        assert!(!check(fixed, false, false, 512, 512).unwrap().unit().removable);
        // Sizes.
        assert_eq!(check(cd, false, false, 2047, 2047), Err(Refusal::TooSmall));
        assert_eq!(check(disk, false, false, 511, 511), Err(Refusal::TooSmall));
        assert_eq!(check(disk, false, false, 1 << 41, 1 << 41), Err(Refusal::TooLarge));
        assert_eq!(check(disk, false, false, 4096, 1024), Err(Refusal::NotFullyWritten));
    }

    #[test]
    fn command_words() {
        assert_eq!(
            parse_command("/.orpheus/images/t.iso"),
            Some(("/.orpheus/images/t.iso", Request { medium: Medium::Cd, write: false }))
        );
        assert_eq!(
            parse_command("/.orpheus/images/t.img disk rw"),
            Some(("/.orpheus/images/t.img", Request { medium: Medium::RemovableDisk, write: true }))
        );
        assert_eq!(
            parse_command("/a.iso  fixed ro"),
            Some(("/a.iso", Request { medium: Medium::FixedDisk, write: false }))
        );
        assert_eq!(parse_command("/a.iso floppy"), None);
        assert_eq!(parse_command("a.iso"), None);
        assert_eq!(parse_command(""), None);
    }
}
