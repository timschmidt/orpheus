//! The project file (`/.orpheus/seq/SEQnn.OSQ`): compact, versioned,
//! checked by a CRC, ready for migration.
//!
//! ```text
//! file    = "OSQ1" version:u16 flags:u16 record* crc:u32
//! record  = tag:u8 len:u16 body[len]                 (unknown tags skipped)
//! tag 1   = project: name[16] tempo:u16 swing:u8 current:u8 mode:u8 seed:u32
//! tag 2   = tracks:  n:u8 (note:u8 level:u8 flags:u8)[n]      flags: 1 mute, 2 solo
//! tag 3   = pattern: index:u8 len:u8 tracks:u8 n:u8 (step:3 bytes)[tracks × n]  (track-major;
//!           n ≥ len: steps past the length are kept, as the model keeps them)
//! tag 4   = song:    n:u8 (pattern:u8 repeats:u8)[n]
//! tag 5   = kit:     an `OSB1` sound bank (orpheus_synth::bank) holding one kit record
//! crc     = CRC-32 (IEEE 802.3, reflected, as zip and PNG) of every byte before it
//! ```
//!
//! Numbers are little-endian. Only patterns that differ from an empty
//! 16-step pattern are written (a new project is about 400 bytes, a full
//! one at most [`MAX_FILE`]). Steps keep their three bytes
//! ([`crate::model::Step`]).
//!
//! **Versions and migration.** `version` is [`VERSION`] (1). A reader
//! takes every version up to its own: a record body longer than the
//! fields it knows is fine (later versions append fields), a shorter one
//! keeps the defaults of the fields it lacks, unknown tags are skipped; a
//! file of a newer version than the reader is refused
//! ([`FileError::TooNew`]) so it is never overwritten by an older
//! firmware's save. [`migrate`] is where a version-2 reader would turn
//! version-1 values into its own.
//!
//! **Nothing is trusted.** The CRC is checked first; then every count is
//! bounded by the record, every index and value is clamped or the record
//! refused, a kit is decoded by the synth's sanitising bank decoder. The
//! `seq_load` fuzz target checks that any input either loads into a
//! project that saves and loads back identically, or is refused.

use orpheus_synth::bank::{self, NAME_LEN, Name, Record};

use crate::model::{MAX_STEPS, PATTERNS, PlayMode, Project, SONG_LEN, Step, TRACKS};

/// File magic.
pub const MAGIC: [u8; 4] = *b"OSQ1";
/// The format version this crate writes and the newest it reads.
pub const VERSION: u16 = 1;
/// Record tags.
pub const TAG_PROJECT: u8 = 1;
/// Tracks record.
pub const TAG_TRACKS: u8 = 2;
/// A pattern.
pub const TAG_PATTERN: u8 = 3;
/// The song.
pub const TAG_SONG: u8 = 4;
/// The kit (an embedded sound bank).
pub const TAG_KIT: u8 = 5;
/// Header bytes (magic, version, flags).
pub const HEADER: usize = 8;
/// Largest file a project makes: header, project, tracks, every pattern
/// at 64 steps, a full song, a 32-entry kit, the CRC.
pub const MAX_FILE: usize = HEADER
    + (3 + NAME_LEN + 9)
    + (3 + 1 + 3 * TRACKS)
    + PATTERNS * (3 + 4 + 3 * TRACKS * MAX_STEPS)
    + (3 + 1 + 2 * SONG_LEN)
    + (3 + 4 + 3 + NAME_LEN + 1 + 9 * orpheus_synth::kit::KIT_ENTRIES)
    + 4;

/// Why a file was not loaded or saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileError {
    /// Shorter than a header and a CRC, or a record runs past the end.
    Short,
    /// Not a project file.
    BadMagic,
    /// The CRC does not match (a torn or damaged file).
    BadCrc,
    /// Written by a newer version.
    TooNew(u16),
    /// A record's contents are impossible.
    BadRecord(u8),
    /// The output buffer is too small.
    Full,
}

/// CRC-32 (IEEE, reflected, initial and final XOR all ones) of `data`,
/// continuing from `crc` (0 to start).
#[must_use]
pub fn crc32(crc: u32, data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    };
    let mut c = !crc;
    for &b in data {
        c = TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

struct Out<'a> {
    buf: &'a mut [u8],
    n: usize,
}

impl Out<'_> {
    fn bytes(&mut self, b: &[u8]) -> Result<(), FileError> {
        let d = self.buf.get_mut(self.n..self.n + b.len()).ok_or(FileError::Full)?;
        d.copy_from_slice(b);
        self.n += b.len();
        Ok(())
    }

    fn u8(&mut self, v: u8) -> Result<(), FileError> {
        self.bytes(&[v])
    }

    /// A record: `body` writes it; its length is filled in after.
    fn record(
        &mut self,
        tag: u8,
        body: impl FnOnce(&mut Self) -> Result<(), FileError>,
    ) -> Result<(), FileError> {
        let at = self.n;
        self.bytes(&[tag, 0, 0])?;
        body(self)?;
        let len = u16::try_from(self.n - at - 3).map_err(|_| FileError::Full)?;
        self.buf[at + 1..at + 3].copy_from_slice(&len.to_le_bytes());
        Ok(())
    }
}

/// Write `p` into `buf`; returns the bytes used (≤ [`MAX_FILE`]).
///
/// # Errors
///
/// [`FileError::Full`] if `buf` is too small.
pub fn save(p: &Project, buf: &mut [u8]) -> Result<usize, FileError> {
    let mut o = Out { buf, n: 0 };
    o.bytes(&MAGIC)?;
    o.bytes(&VERSION.to_le_bytes())?;
    o.bytes(&0u16.to_le_bytes())?;
    o.record(TAG_PROJECT, |o| {
        o.bytes(&p.name_raw().0)?;
        o.bytes(&p.tempo().to_le_bytes())?;
        o.u8(p.swing())?;
        o.u8(p.current() as u8)?;
        o.u8(match p.mode() {
            PlayMode::Pattern => 0,
            PlayMode::Song => 1,
        })?;
        o.bytes(&p.seed.to_le_bytes())
    })?;
    o.record(TAG_TRACKS, |o| {
        o.u8(TRACKS as u8)?;
        for t in p.tracks() {
            o.bytes(&[t.note, t.level, u8::from(t.mute) | (u8::from(t.solo) << 1)])?;
        }
        Ok(())
    })?;
    for i in 0..PATTERNS {
        let pat = p.pattern(i);
        if pat.is_blank() {
            continue;
        }
        let n = pat.stored();
        o.record(TAG_PATTERN, |o| {
            o.bytes(&[i as u8, pat.len() as u8, TRACKS as u8, n as u8])?;
            for t in 0..TRACKS {
                for s in &pat.track(t)[..n] {
                    o.bytes(&s.to_bytes())?;
                }
            }
            Ok(())
        })?;
    }
    if !p.song.is_empty() {
        o.record(TAG_SONG, |o| {
            o.u8(p.song.len() as u8)?;
            for e in p.song.entries() {
                o.bytes(&[e.pattern, e.repeats])?;
            }
            Ok(())
        })?;
    }
    o.record(TAG_KIT, |o| {
        let rest = o.buf.get_mut(o.n..).ok_or(FileError::Full)?;
        let mut w = bank::Writer::new(rest).map_err(|_| FileError::Full)?;
        w.kit(p.kit_name(), p.kit()).map_err(|_| FileError::Full)?;
        o.n += w.len();
        Ok(())
    })?;
    let crc = crc32(0, &o.buf[..o.n]);
    o.bytes(&crc.to_le_bytes())?;
    Ok(o.n)
}

/// Check the header and the CRC; returns the version and the records'
/// bytes.
fn check(d: &[u8]) -> Result<(u16, &[u8]), FileError> {
    if d.len() < HEADER + 4 {
        return Err(if d.len() >= 4 && d[..4] != MAGIC { FileError::BadMagic } else { FileError::Short });
    }
    if d[..4] != MAGIC {
        return Err(FileError::BadMagic);
    }
    let (body, crc) = d.split_at(d.len() - 4);
    if crc32(0, body) != u32::from_le_bytes([crc[0], crc[1], crc[2], crc[3]]) {
        return Err(FileError::BadCrc);
    }
    let version = u16::from_le_bytes([d[4], d[5]]);
    if version > VERSION {
        return Err(FileError::TooNew(version));
    }
    if version == 0 {
        return Err(FileError::BadRecord(0));
    }
    Ok((version, &body[HEADER..]))
}

/// The project's name from a file, without loading it (the project list).
///
/// # Errors
///
/// As [`load_into`]'s header and CRC checks.
pub fn peek_name(d: &[u8]) -> Result<Name, FileError> {
    let (_, mut r) = check(d)?;
    while r.len() >= 3 {
        let (tag, len) = (r[0], usize::from(u16::from_le_bytes([r[1], r[2]])));
        let body = r.get(3..3 + len).ok_or(FileError::Short)?;
        if tag == TAG_PROJECT {
            return Ok(clean(body.get(..NAME_LEN).ok_or(FileError::BadRecord(tag))?));
        }
        r = &r[3 + len..];
    }
    Ok(Project::DEFAULT_NAME)
}

/// Load a file into `p` (reset first). On an error after the checks `p`
/// holds what was read so far (callers keep the file and start a new
/// project).
///
/// # Errors
///
/// [`FileError`].
pub fn load_into(p: &mut Project, d: &[u8]) -> Result<(), FileError> {
    let (version, mut r) = check(d)?;
    p.reset();
    let mut kit_seen = false;
    while !r.is_empty() {
        let h = r.get(..3).ok_or(FileError::Short)?;
        let (tag, len) = (h[0], usize::from(u16::from_le_bytes([h[1], h[2]])));
        let body = r.get(3..3 + len).ok_or(FileError::Short)?;
        r = &r[3 + len..];
        match tag {
            TAG_PROJECT => project(p, body)?,
            TAG_TRACKS => tracks(p, body),
            TAG_PATTERN => pattern(p, body)?,
            TAG_SONG => song(p, body),
            TAG_KIT if !kit_seen => {
                kit_seen = true;
                kit(p, body)?;
            }
            _ => {}
        }
    }
    migrate(p, version);
    Ok(())
}

/// Turn the values of an older `version` into this version's. Version 1
/// is the first: nothing to do.
pub fn migrate(_p: &mut Project, version: u16) {
    debug_assert!(version <= VERSION);
}

fn project(p: &mut Project, b: &[u8]) -> Result<(), FileError> {
    let name = b.get(..NAME_LEN).ok_or(FileError::BadRecord(TAG_PROJECT))?;
    p.set_name_raw(clean(name));
    let at = |i: usize, d: u8| b.get(NAME_LEN + i).copied().unwrap_or(d);
    let tempo = u16::from_le_bytes([at(0, 0xE0), at(1, 0x2E)]);
    p.set_raw(tempo, at(2, 50), at(3, 0));
    p.set_mode(if at(4, 0) == 1 { PlayMode::Song } else { PlayMode::Pattern });
    if let Some(s) = b.get(NAME_LEN + 5..NAME_LEN + 9) {
        p.seed = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
    }
    Ok(())
}

fn tracks(p: &mut Project, b: &[u8]) {
    let n = usize::from(b.first().copied().unwrap_or(0)).min(TRACKS);
    for (t, x) in b.get(1..).unwrap_or(&[]).chunks_exact(3).take(n).enumerate() {
        let tr = p.track_mut(t);
        tr.note = x[0].min(127);
        tr.level = x[1].min(100);
        tr.mute = x[2] & 1 != 0;
        tr.solo = x[2] & 2 != 0;
    }
}

fn pattern(p: &mut Project, b: &[u8]) -> Result<(), FileError> {
    let bad = FileError::BadRecord(TAG_PATTERN);
    let h = b.get(..4).ok_or(bad)?;
    let (i, len, tracks, n) = (usize::from(h[0]), usize::from(h[1]), usize::from(h[2]), usize::from(h[3]));
    if i >= PATTERNS || len == 0 || len > MAX_STEPS || tracks == 0 || n < len || n > MAX_STEPS {
        return Err(bad);
    }
    let steps = b.get(4..4 + 3 * tracks * n).ok_or(bad)?;
    let pat = p.pattern_mut(i);
    pat.set_len_raw(len);
    // Tracks past this version's are skipped (a later version may have
    // more).
    for (t, row) in steps.chunks_exact(3 * n).take(TRACKS).enumerate() {
        for (s, x) in row.chunks_exact(3).enumerate() {
            pat.set_raw(t, s, Step::from_bytes([x[0], x[1], x[2]]));
        }
    }
    Ok(())
}

fn song(p: &mut Project, b: &[u8]) {
    let n = usize::from(b.first().copied().unwrap_or(0)).min(SONG_LEN);
    p.song.clear();
    for x in b.get(1..).unwrap_or(&[]).chunks_exact(2).take(n) {
        p.song.push(x[0], x[1]);
    }
}

fn kit(p: &mut Project, b: &[u8]) -> Result<(), FileError> {
    let recs = bank::records(b).map_err(|_| FileError::BadRecord(TAG_KIT))?;
    for r in recs {
        match r {
            Ok(Record::Kit(name, kit)) => {
                p.set_kit_raw(clean(&name.0), kit);
                return Ok(());
            }
            Ok(Record::Patch(..)) => {}
            Err(_) => return Err(FileError::BadRecord(TAG_KIT)),
        }
    }
    // No kit in the bank: the default kit stays.
    Ok(())
}

/// A stored name as the model keeps names: its valid UTF-8 up to the
/// first NUL, NUL-padded (so a loaded project saves the same bytes).
#[must_use]
pub fn clean(b: &[u8]) -> Name {
    let b = &b[..b.len().min(NAME_LEN)];
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    let s = match core::str::from_utf8(&b[..end]) {
        Ok(s) => s,
        Err(e) => core::str::from_utf8(&b[..e.valid_up_to()]).unwrap_or(""),
    };
    Name::new(s)
}
