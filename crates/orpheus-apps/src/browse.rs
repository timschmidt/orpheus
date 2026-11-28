//! Shared browsing logic: what a file is (by extension) and the one
//! collation of 04 §6.1 (case-insensitive, digits compared as numbers).
//! Used by the firmware's directory index and by the simulator, so both
//! sort exactly alike.

use core::cmp::Ordering;

/// A directory as the browse service names it (the firmware: its first
/// cluster on the card; [`DirId::ROOT`] is the root).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct DirId(pub u32);

impl DirId {
    pub const ROOT: DirId = DirId(0);
}

/// A file or directory on the card: the `orpheus-fs` locator (parent
/// directory's first cluster, entry-set index). The service pairs it with
/// the volume serial and a name hash for the 16-byte locator of 04 §6.3.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct FileRef {
    pub dir: u32,
    pub index: u32,
}

impl FileRef {
    /// Into a resume record's bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 8] {
        let mut b = [0u8; 8];
        b[..4].copy_from_slice(&self.dir.to_le_bytes());
        b[4..].copy_from_slice(&self.index.to_le_bytes());
        b
    }

    #[must_use]
    pub fn from_bytes(b: &[u8]) -> FileRef {
        let w = |o: usize| b.get(o..o + 4).and_then(|s| s.try_into().ok()).map_or(0, u32::from_le_bytes);
        FileRef { dir: w(0), index: w(4) }
    }
}

/// What the browser shows and does for an entry (05 §7.2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntryKind {
    Dir,
    Audio,
    Playlist,
    Image,
    Text,
    DiskImage,
    Firmware,
    OtpImport,
    Other,
}

fn ext_is(name: &str, exts: &[&str]) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else { return false };
    exts.iter().any(|e| ext.eq_ignore_ascii_case(e))
}

/// Audio extensions the player opens (04 §4.1).
pub const AUDIO_EXTS: &[&str] = &[
    "mp3", "mp2", "flac", "ogg", "oga", "opus", "spx", "wav", "aif", "aiff", "m4a", "mp4", "aac", "alac",
    "wv", "ape", "wma", "au",
];

/// MIDI extensions, audio with the `midi` feature (MIDI-1).
pub const MIDI_EXTS: &[&str] = &["mid", "midi", "kar", "rmi"];

/// Whether `name` is an audio file the player opens.
#[must_use]
pub fn is_audio_name(name: &str) -> bool {
    ext_is(name, AUDIO_EXTS) || (cfg!(feature = "midi") && ext_is(name, MIDI_EXTS))
}

/// Classify an entry by its name.
#[must_use]
pub fn kind_of(name: &str, is_dir: bool) -> EntryKind {
    if is_dir {
        EntryKind::Dir
    } else if name.eq_ignore_ascii_case("otpauth.txt") || name.eq_ignore_ascii_case("otp-import.txt") {
        EntryKind::OtpImport
    } else if is_audio_name(name) {
        EntryKind::Audio
    } else if ext_is(name, &["m3u", "m3u8", "pls"]) {
        EntryKind::Playlist
    } else if ext_is(name, &["jpg", "jpeg", "png", "bmp", "gif", "qoi"]) {
        EntryKind::Image
    } else if ext_is(name, &["txt", "lrc", "cue", "md", "log", "nfo"]) {
        EntryKind::Text
    } else if ext_is(name, &["iso", "img", "bin", "raw", "vhd"]) {
        EntryKind::DiskImage
    } else if ext_is(name, &["yfw", "ofw"]) {
        EntryKind::Firmware
    } else {
        EntryKind::Other
    }
}

/// Hidden in the browser unless "Show hidden" is on: dot-files, `.orpheus`,
/// `System Volume Information` (and entries with the FAT hidden or system
/// attribute, which the service checks).
#[must_use]
pub fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.') || name.eq_ignore_ascii_case("System Volume Information")
}

/// Where Pictures opens (05 §7.6, UI-PIC 2026-11-28): the first of these
/// folders the card has, else the root. The look-up ignores case as the
/// file system does (FAT and exFAT names are case-insensitive), so
/// `/PICTURES` and `/pictures` are `/Pictures`.
pub const PICTURE_DIRS: [&str; 3] = ["/Pictures", "/Photos", "/DCIM"];

/// Simple case folding for Latin, Greek and Cyrillic (04 §6.1).
#[must_use]
pub fn fold(c: char) -> char {
    let mut l = c.to_lowercase();
    match (l.next(), l.next()) {
        (Some(f), None) => match f {
            'ё' => 'е',
            'ς' => 'σ',
            other => other,
        },
        _ => c,
    }
}

/// The browser's sort order: case-insensitive, runs of digits compared as
/// numbers ("Track 2" < "Track 10"), ties broken by the raw string.
#[must_use]
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let mut na = 0u64;
                let mut la = 0;
                while let Some(c) = x.peek().copied().filter(char::is_ascii_digit) {
                    na = na.saturating_mul(10).saturating_add(u64::from(c.to_digit(10).unwrap_or(0)));
                    la += 1;
                    x.next();
                }
                let mut nb = 0u64;
                let mut lb = 0;
                while let Some(d) = y.peek().copied().filter(char::is_ascii_digit) {
                    nb = nb.saturating_mul(10).saturating_add(u64::from(d.to_digit(10).unwrap_or(0)));
                    lb += 1;
                    y.next();
                }
                match na.cmp(&nb).then(la.cmp(&lb)) {
                    Ordering::Equal => {}
                    o => return o,
                }
            }
            (Some(c), Some(d)) => {
                match fold(c).cmp(&fold(d)) {
                    Ordering::Equal => {}
                    o => return o,
                }
                x.next();
                y.next();
            }
        }
    }
}

/// The letter a row is filed under for letter jump: the folded first
/// alphanumeric character, upper-cased; digits are `#`.
#[must_use]
pub fn letter_of(name: &str) -> Option<char> {
    let c = name.chars().find(|c| c.is_alphanumeric())?;
    if c.is_ascii_digit() {
        return Some('#');
    }
    let f = fold(c);
    f.to_uppercase().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(kind_of("a.MP3", false), EntryKind::Audio);
        assert_eq!(kind_of("Mix.m3u8", false), EntryKind::Playlist);
        assert_eq!(kind_of("OTPAUTH.TXT", false), EntryKind::OtpImport);
        assert_eq!(kind_of("notes.txt", false), EntryKind::Text);
        assert_eq!(kind_of("x.iso", false), EntryKind::DiskImage);
        assert_eq!(kind_of("fw.yfw", false), EntryKind::Firmware);
        assert_eq!(kind_of("orpheus-0.2.0.ofw", false), EntryKind::Firmware);
        assert_eq!(kind_of("noext", false), EntryKind::Other);
        assert_eq!(kind_of("dir.mp3", true), EntryKind::Dir);
        let midi = if cfg!(feature = "midi") { EntryKind::Audio } else { EntryKind::Other };
        assert_eq!(kind_of("Song.KAR", false), midi);
        assert_eq!(kind_of("tune.mid", false), midi);
    }

    #[test]
    fn natural_order() {
        assert_eq!(natural_cmp("Track 2", "Track 10"), Ordering::Less);
        assert_eq!(natural_cmp("track 2", "Track 2"), "track 2".cmp("Track 2"));
        assert_eq!(natural_cmp("abc", "ABD"), Ordering::Less);
        assert_eq!(natural_cmp("Ёлка", "елка"), "Ёлка".cmp("елка"));
        assert_eq!(natural_cmp("Ёлка", "Ежи"), Ordering::Greater);
        assert_eq!(natural_cmp("a", "a1"), Ordering::Less);
        assert_eq!(natural_cmp("x02", "x2"), Ordering::Greater);
        assert_eq!(letter_of("élan"), Some('É'));
        assert_eq!(letter_of("01 Intro"), Some('#'));
        assert_eq!(letter_of("ёж"), Some('Е'));
    }
}
