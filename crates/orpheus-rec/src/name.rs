//! Recording file names (05 §7.5): `/Recordings/REC-YYYYMMDD-HHMMSS.ext`;
//! from the radio `FM-101.90-YYYYMMDD-HHMMSS.ext`; with the clock not set
//! `REC-0001.ext` (the first number not taken). Two recordings in the same
//! second get `-2`, `-3`, … before the extension.

/// The folder for recordings (05 §7.5). The device test writes under
/// `/.orpheus/test/rec` instead (CLAUDE.md rule 5).
pub const FOLDER: &str = "/Recordings";
/// Longest path built here.
pub const MAX_PATH: usize = 96;

/// A fixed-capacity path.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Path {
    buf: [u8; MAX_PATH],
    len: usize,
}

impl core::fmt::Debug for Path {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Default for Path {
    fn default() -> Self {
        Path { buf: [0; MAX_PATH], len: 0 }
    }
}

impl Path {
    #[must_use]
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.buf.get(..self.len).unwrap_or(&[])).unwrap_or("")
    }

    /// The last component.
    #[must_use]
    pub fn file_name(&self) -> &str {
        let s = self.as_str();
        s.rsplit('/').next().unwrap_or(s)
    }

    /// Append (silently cut at the capacity, on a character boundary).
    pub fn push(&mut self, s: &str) {
        for c in s.chars() {
            let mut b = [0u8; 4];
            let e = c.encode_utf8(&mut b).as_bytes();
            match self.buf.get_mut(self.len..self.len + e.len()) {
                Some(d) => d.copy_from_slice(e),
                None => return,
            }
            self.len += e.len();
        }
    }

    /// Append a number, zero-padded to `width`.
    pub fn push_num(&mut self, v: u32, width: usize) {
        // Digits from the lowest, then pushed from the highest.
        let mut d = [0u8; 10];
        let mut n = 0;
        let mut v = v;
        for slot in d.iter_mut() {
            *slot = b'0' + (v % 10) as u8;
            n += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        for _ in n..width {
            self.push("0");
        }
        for &c in d.iter().take(n).rev() {
            let b = [c];
            self.push(core::str::from_utf8(&b).unwrap_or(""));
        }
    }

    #[must_use]
    pub fn of(s: &str) -> Path {
        let mut p = Path::default();
        p.push(s);
        p
    }
}

/// A calendar time (the RTC's).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Stamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// Build a recording's path in `folder`.
///
/// * `when`: the clock, `None` if it is not set (then `counter` numbers
///   the file);
/// * `fm_khz`: the station when recording the radio;
/// * `suffix`: 0 or 1 for none, else `-suffix` (a name already taken).
#[must_use]
pub fn path(
    folder: &str,
    when: Option<Stamp>,
    counter: u32,
    fm_khz: Option<u32>,
    suffix: u32,
    ext: &str,
) -> Path {
    let mut p = Path::of(folder);
    p.push("/");
    match fm_khz {
        Some(khz) => {
            p.push("FM-");
            p.push_num(khz / 1000, 1);
            p.push(".");
            p.push_num(khz % 1000 / 10, 2);
            p.push("-");
        }
        None => p.push("REC-"),
    }
    match when {
        Some(t) => {
            p.push_num(u32::from(t.year), 4);
            p.push_num(u32::from(t.month), 2);
            p.push_num(u32::from(t.day), 2);
            p.push("-");
            p.push_num(u32::from(t.hour), 2);
            p.push_num(u32::from(t.minute), 2);
            p.push_num(u32::from(t.second), 2);
        }
        None => p.push_num(counter, 4),
    }
    if suffix > 1 {
        p.push("-");
        p.push_num(suffix, 1);
    }
    p.push(".");
    p.push(ext);
    p
}

/// Whether a name looks like one of ours (`REC-…`/`FM-…` with an audio
/// extension): the Recordings list and the counter look at these only.
#[must_use]
pub fn is_recording(name: &str) -> bool {
    let lower_ext = name.rsplit_once('.').map(|(_, e)| e);
    let ext_ok = lower_ext.is_some_and(|e| e.eq_ignore_ascii_case("wav") || e.eq_ignore_ascii_case("flac"));
    ext_ok && (name.starts_with("REC-") || name.starts_with("FM-"))
}

/// The counter of an undated name (`REC-0042.wav` → 42).
#[must_use]
pub fn counter_of(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("REC-")?;
    let digits = rest.split(['.', '-']).next()?;
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Stamp = Stamp { year: 2026, month: 11, day: 24, hour: 9, minute: 5, second: 7 };

    #[test]
    fn dated_names() {
        assert_eq!(path(FOLDER, Some(T), 0, None, 0, "wav").as_str(), "/Recordings/REC-20261124-090507.wav");
        assert_eq!(
            path(FOLDER, Some(T), 0, Some(101_900), 2, "flac").as_str(),
            "/Recordings/FM-101.90-20261124-090507-2.flac"
        );
        assert_eq!(
            path(FOLDER, Some(T), 0, Some(87_500), 0, "wav").file_name(),
            "FM-87.50-20261124-090507.wav"
        );
    }

    #[test]
    fn counter_when_the_clock_is_not_set() {
        let p = path("/.orpheus/test/rec", None, 7, None, 0, "wav");
        assert_eq!(p.as_str(), "/.orpheus/test/rec/REC-0007.wav");
        assert_eq!(counter_of(p.file_name()), Some(7));
        assert_eq!(counter_of("REC-20261124-090507.wav"), None);
        assert!(is_recording("REC-0007.wav") && is_recording("FM-101.90-x.FLAC"));
        assert!(!is_recording("song.wav") && !is_recording("REC-1.mp3"));
    }
}
