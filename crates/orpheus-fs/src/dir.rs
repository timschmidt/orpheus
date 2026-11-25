//! Directories: positions, entry-set parsing (FAT short/long names, exFAT
//! entry sets), lookup, iteration with resumable cursors, creation, removal,
//! renaming and the path walk.

use crate::block::BlockDevice;
use crate::cache::Kind;
use crate::unicode::{cp437, fold16, utf16_to_utf8_in_place};
use crate::volume::{DateTime, FsKind, Volume, rd16, rd32, rd64};
use crate::{Error, Result};

/// Maximum UTF-8 length of a name (255 UTF-16 units, 3 bytes each).
pub const MAX_NAME_UTF8: usize = 765;
const NAME_BUF: usize = 768;
/// Where UTF-16 units are collected inside the name buffer before the
/// in-place conversion (see `utf16_to_utf8_in_place`).
const U16_AT: usize = NAME_BUF - 2 * 255;
const MAX_NAME16: usize = 255;
/// FAT directories hold at most 65536 entries.
const FAT_DIR_MAX_BYTES: u64 = 65536 * 32;
const END: u32 = u32::MAX;
const MAX_DEPTH: usize = 32;

pub(crate) const ATTR_RO: u8 = 0x01;
pub(crate) const ATTR_DIR: u8 = 0x10;
pub(crate) const ATTR_ARCHIVE: u8 = 0x20;

/// Directories the path walk resolved lately (lent to the volume with
/// [`Volume::set_walk_cache`], like the sector cache): a walk starts at the
/// longest cached prefix instead of the root. On a large card the root directory
/// alone can take tens of sector reads to search, and the firmware opens
/// files under `/.orpheus/...` while audio plays. Keys are the exact path
/// bytes up to a component (at most [`WALK_KEY`] bytes); the cache is
/// emptied whenever a directory may have changed shape: an entry set is
/// deleted or restored (remove, rename) or a directory grows (exFAT
/// lengths).
pub(crate) const WALK_KEY: usize = 24;
const WALK_SLOTS: usize = 3;

/// One cached directory, packed (a `DirSpec` is 32 bytes with padding).
#[derive(Clone, Copy, Default)]
struct WalkSlot {
    first: u32,
    /// Byte length bound (at most 256 MiB).
    len: u32,
    loc_cluster: u32,
    loc_off: u32,
    /// Bit 0: the directory is contiguous; bit 1: its entry's location is;
    /// bits 2–7: uses ([`USE_SHIFT`]: +4 per hit, −1 every 8th insert).
    flags: u8,
    /// Key length; 0: an empty slot.
    klen: u8,
    key: [u8; WALK_KEY],
}

/// The use count in `WalkSlot::flags` (bits 2–7, saturating at 63).
const USE_SHIFT: u8 = 2;

impl WalkSlot {
    fn uses(&self) -> u8 {
        self.flags >> USE_SHIFT
    }

    fn set_uses(&mut self, n: u8) {
        self.flags = (self.flags & 0b11) | (n.min(63) << USE_SHIFT);
    }

    fn spec(&self) -> DirSpec {
        DirSpec {
            first: self.first,
            contig: self.flags & 1 != 0,
            len: u64::from(self.len),
            self_loc: EntryLoc { cluster: self.loc_cluster, off: self.loc_off, contig: self.flags & 2 != 0 },
        }
    }
}

/// A small cache of resolved directory paths, so repeated walks to the
/// same folder skip the directory reads.
#[derive(Clone, Copy, Default)]
pub struct WalkCache {
    slots: [WalkSlot; WALK_SLOTS],
    next: u8,
    /// Walks served from the cache (statistics, tests).
    hits: u32,
}

impl WalkCache {
    /// A cache with no entries.
    pub const EMPTY: WalkCache = WalkCache {
        slots: [WalkSlot {
            first: 0,
            len: 0,
            loc_cluster: 0,
            loc_off: 0,
            flags: 0,
            klen: 0,
            key: [0; WALK_KEY],
        }; WALK_SLOTS],
        next: 0,
        hits: 0,
    };

    pub(crate) fn clear(&mut self) {
        for s in &mut self.slots {
            s.klen = 0;
        }
    }

    /// The longest cached directory that `path` starts with, ending at a
    /// component boundary: its spec and where the rest of the path starts.
    fn lookup(&mut self, path: &str) -> Option<(DirSpec, usize)> {
        let b = path.as_bytes();
        let mut best: Option<(usize, usize)> = None;
        for (i, s) in self.slots.iter().enumerate().filter(|(_, s)| s.klen > 0) {
            let n = usize::from(s.klen);
            let boundary = matches!(b.get(n), None | Some(b'/'));
            if boundary && b.get(..n) == s.key.get(..n) && best.is_none_or(|(_, m)| n > m) {
                best = Some((i, n));
            }
        }
        let (i, n) = best?;
        let s = self.slots.get_mut(i)?;
        s.set_uses(s.uses().saturating_add(4));
        Some((s.spec(), n))
    }

    fn insert(&mut self, key: &[u8], spec: DirSpec) {
        let Ok(len) = u32::try_from(spec.len) else { return };
        if key.len() > WALK_KEY || key.is_empty() {
            return;
        }
        if self.slots.iter().any(|s| s.klen > 0 && s.key.get(..usize::from(s.klen)) == Some(key)) {
            return;
        }
        let mut k = [0u8; WALK_KEY];
        if let Some(d) = k.get_mut(..key.len()) {
            d.copy_from_slice(key);
        }
        // An empty slot, else the least used (from `next` round, so equal
        // counts take turns): the firmware's own folders (`/.orpheus/index`,
        // read by every listing) outlive the art and library walks that
        // pass through. Round robin alone evicted them, and each listing
        // then searched the card's root again (60 ms, 2026-11-25).
        let start = usize::from(self.next) % WALK_SLOTS;
        let i = (0..WALK_SLOTS)
            .map(|d| (start + d) % WALK_SLOTS)
            .min_by_key(|&j| self.slots.get(j).map_or(0, |s| if s.klen == 0 { 0 } else { 1 + u16::from(s.uses()) }))
            .unwrap_or(start);
        self.next = self.next.wrapping_add(1);
        if self.next % 8 == 0 {
            for s in &mut self.slots {
                s.set_uses(s.uses().saturating_sub(1));
            }
        }
        if let Some(slot) = self.slots.get_mut(i) {
            *slot = WalkSlot {
                first: spec.first,
                len,
                loc_cluster: spec.self_loc.cluster,
                loc_off: spec.self_loc.off,
                flags: u8::from(spec.contig) | u8::from(spec.self_loc.contig) << 1,
                klen: key.len() as u8,
                key: k,
            };
        }
    }
}

/// Path components: split on `/`, empty and `.` components dropped.
pub(crate) fn components(path: &str) -> impl Iterator<Item = &str> {
    path.split('/').filter(|c| !c.is_empty() && *c != ".")
}

/// Location of one 32-byte directory entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct EntryLoc {
    /// Cluster holding the entry; 0 = FAT12/16 fixed root; `END` = none.
    pub cluster: u32,
    /// Byte offset inside the cluster (or the fixed root).
    pub off: u32,
    /// The directory is a contiguous exFAT run (NoFatChain).
    pub contig: bool,
}

impl EntryLoc {
    pub const NONE: EntryLoc = EntryLoc { cluster: END, off: 0, contig: false };
}

/// Which directory and how to walk it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct DirSpec {
    pub first: u32,
    pub contig: bool,
    /// Byte length bound.
    pub len: u64,
    /// exFAT: the directory's own entry set in its parent (for growth).
    pub self_loc: EntryLoc,
}

/// Position inside a directory.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct DirPos {
    pub cluster: u32,
    pub off: u32,
    pub index: u32,
}

/// A resumable directory cursor (40 bytes, `Copy`). Store it to come back
/// to the same place later; it stays valid until the directory changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DirCursor {
    pub(crate) spec: DirSpec,
    pub(crate) pos: DirPos,
}

impl DirCursor {
    /// Entry index (32-byte slot number) of the next entry to examine.
    #[must_use]
    pub fn index(&self) -> u32 {
        self.pos.index
    }
    /// True once the end of the directory was reached.
    #[must_use]
    pub fn at_end(&self) -> bool {
        self.pos.cluster == END
    }
    /// First cluster of the directory (0 for a FAT12/16 root).
    #[must_use]
    pub fn dir_cluster(&self) -> u32 {
        self.spec.first
    }
}

/// Compact reference to a directory entry: the directory's first cluster
/// and the entry-set index (bit 31: the directory is a contiguous exFAT
/// run). Together with the volume serial and a name hash kept by the caller
/// this is the 16-byte file locator of 04-audio §6.3.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Locator {
    /// First cluster of the parent directory.
    pub dir: u32,
    /// Entry index of the set's first slot; bit 31 = contiguous directory.
    pub index: u32,
}

/// Metadata of one entry set.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EntryInfo {
    pub attr: u8,
    pub first_cluster: u32,
    pub size: u64,
    pub valid: u64,
    pub contig: bool,
    pub start: EntryLoc,
    pub main: EntryLoc,
    pub count: u8,
    pub index: u32,
    pub created: DateTime,
    pub modified: DateTime,
    pub sfn: [u8; 11],
    pub has_lfn: bool,
}

impl EntryInfo {
    const EMPTY: EntryInfo = EntryInfo {
        attr: 0,
        first_cluster: 0,
        size: 0,
        valid: 0,
        contig: false,
        start: EntryLoc::NONE,
        main: EntryLoc::NONE,
        count: 0,
        index: 0,
        created: DateTime::DEFAULT,
        modified: DateTime::DEFAULT,
        sfn: [0; 11],
        has_lfn: false,
    };
    pub fn is_dir(&self) -> bool {
        self.attr & ATTR_DIR != 0
    }
}

/// One directory entry as returned by [`Volume::read_dir`] (about 840 bytes;
/// the caller owns it).
#[derive(Clone)]
pub struct DirEntry {
    name: [u8; NAME_BUF],
    name_len: u16,
    pub(crate) info: EntryInfo,
    pub(crate) dir: DirSpec,
}

impl Default for DirEntry {
    fn default() -> Self {
        Self::new()
    }
}

impl DirEntry {
    /// An empty entry to be filled by `read_dir`/`stat`.
    #[must_use]
    pub const fn new() -> Self {
        DirEntry {
            name: [0; NAME_BUF],
            name_len: 0,
            info: EntryInfo::EMPTY,
            dir: DirSpec { first: 0, contig: false, len: 0, self_loc: EntryLoc::NONE },
        }
    }
    /// The (long) name.
    #[must_use]
    pub fn name(&self) -> &str {
        let n = usize::from(self.name_len).min(NAME_BUF);
        core::str::from_utf8(&self.name[..n]).unwrap_or("")
    }
    /// True for a directory.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.info.is_dir()
    }
    /// File size in bytes (exFAT directories: allocated bytes).
    #[must_use]
    pub fn size(&self) -> u64 {
        self.info.size
    }
    /// FAT attribute bits (0x01 read-only, 0x02 hidden, 0x04 system,
    /// 0x10 directory, 0x20 archive).
    #[must_use]
    pub fn attributes(&self) -> u8 {
        self.info.attr
    }
    /// Last modification time.
    #[must_use]
    pub fn modified(&self) -> DateTime {
        self.info.modified
    }
    /// Creation time.
    #[must_use]
    pub fn created(&self) -> DateTime {
        self.info.created
    }
    /// First data cluster (0 for an empty file).
    #[must_use]
    pub fn first_cluster(&self) -> u32 {
        self.info.first_cluster
    }
    /// FAT 8.3 name as stored (11 bytes, space padded), if FAT.
    #[must_use]
    pub fn short_name(&self) -> [u8; 11] {
        self.info.sfn
    }
    /// Entry-set index inside its directory.
    #[must_use]
    pub fn index(&self) -> u32 {
        self.info.index
    }
    /// Locator for re-opening this entry without a path walk.
    #[must_use]
    pub fn locator(&self) -> Locator {
        Locator { dir: self.dir.first, index: self.info.index | if self.dir.contig { 1 << 31 } else { 0 } }
    }
    /// Cursor positioned on this entry (`read_dir` returns it again).
    #[must_use]
    pub fn cursor(&self) -> DirCursor {
        DirCursor {
            spec: self.dir,
            pos: DirPos {
                cluster: self.info.start.cluster,
                off: self.info.start.off,
                index: self.info.index,
            },
        }
    }
}

// ------------------------------------------------------------ name sinks

pub(crate) trait Sink {
    fn reset(&mut self);
    fn set_len(&mut self, len16: usize);
    fn put(&mut self, idx: usize, units: &[u16]);
}

pub(crate) struct Matcher<'a> {
    target: &'a str,
    len16: usize,
    pub ok: bool,
}

impl<'a> Matcher<'a> {
    pub fn new(target: &'a str) -> Self {
        Matcher { target, len16: target.encode_utf16().count(), ok: false }
    }
}

impl Sink for Matcher<'_> {
    fn reset(&mut self) {
        self.ok = true;
    }
    fn set_len(&mut self, len16: usize) {
        if len16 != self.len16 {
            self.ok = false;
        }
    }
    fn put(&mut self, idx: usize, units: &[u16]) {
        if !self.ok {
            return;
        }
        if idx + units.len() > self.len16 {
            self.ok = false;
            return;
        }
        let t = self.target.encode_utf16().skip(idx);
        for (&u, v) in units.iter().zip(t) {
            if fold16(u) != fold16(v) {
                self.ok = false;
                return;
            }
        }
    }
}

struct Collector<'a> {
    buf: &'a mut [u8; NAME_BUF],
    len: usize,
}

impl Sink for Collector<'_> {
    fn reset(&mut self) {
        self.len = 0;
    }
    fn set_len(&mut self, len16: usize) {
        self.len = len16.min(MAX_NAME16);
    }
    fn put(&mut self, idx: usize, units: &[u16]) {
        for (k, &u) in units.iter().enumerate() {
            let i = idx + k;
            if i >= MAX_NAME16 {
                return;
            }
            let p = U16_AT + 2 * i;
            self.buf[p..p + 2].copy_from_slice(&u.to_le_bytes());
        }
    }
}

struct NoSink;
impl Sink for NoSink {
    fn reset(&mut self) {}
    fn set_len(&mut self, _: usize) {}
    fn put(&mut self, _: usize, _: &[u16]) {}
}

/// Free-slot tracking during a scan (for creating entries).
pub(crate) struct FreeTrack {
    need: u32,
    run_start: DirPos,
    run_len: u32,
    found: bool,
    last_cluster: u32,
    last_index: u32,
}

impl FreeTrack {
    fn new(need: u32) -> Self {
        FreeTrack {
            need,
            run_start: DirPos { cluster: END, off: 0, index: 0 },
            run_len: 0,
            found: false,
            last_cluster: END,
            last_index: 0,
        }
    }
    fn free_at(&mut self, p: DirPos) {
        self.last_cluster = p.cluster;
        self.last_index = p.index;
        if self.found {
            return;
        }
        if self.run_len == 0 {
            self.run_start = p;
        }
        self.run_len += 1;
        if self.run_len >= self.need {
            self.found = true;
        }
    }
    fn used(&mut self, p: DirPos) {
        self.last_cluster = p.cluster;
        self.last_index = p.index;
        if !self.found {
            self.run_len = 0;
        }
    }
}

// ----------------------------------------------------------- FAT helpers

fn sfn_checksum(sfn: &[u8; 11]) -> u8 {
    let mut s: u8 = 0;
    for &b in sfn {
        s = (if s & 1 != 0 { 0x80u8 } else { 0 }).wrapping_add(s >> 1).wrapping_add(b);
    }
    s
}

/// Decode a short name into UTF-16 units; returns the count.
fn sfn_units(e: &[u8], out: &mut [u16; 12]) -> usize {
    let nt = e.get(12).copied().unwrap_or(0);
    let mut n = 0;
    let base_end = (0..8).rev().find(|&i| e[i] != b' ').map_or(0, |i| i + 1);
    for (i, &b0) in e[..base_end].iter().enumerate() {
        let b = if i == 0 && b0 == 0x05 { 0xE5 } else { b0 };
        let b = if nt & 0x08 != 0 { b.to_ascii_lowercase() } else { b };
        out[n] = cp437(b);
        n += 1;
    }
    let ext_end = (8..11).rev().find(|&i| e[i] != b' ').map_or(8, |i| i + 1);
    if ext_end > 8 {
        out[n] = u16::from(b'.');
        n += 1;
        for &b in &e[8..ext_end] {
            let b = if nt & 0x10 != 0 { b.to_ascii_lowercase() } else { b };
            out[n] = cp437(b);
            n += 1;
        }
    }
    n
}

fn sfn_char_ok(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'()-@^_`{}~".contains(c)
}

/// The name as an exact 8.3 short name, if it is one: (sfn, NT case flags).
fn short_form(name: &str) -> Option<([u8; 11], u8)> {
    let (base, ext) = match name.rfind('.') {
        Some(i) => (&name[..i], &name[i + 1..]),
        None => (name, ""),
    };
    if base.is_empty() || base.len() > 8 || ext.len() > 3 || (name.contains('.') && ext.is_empty()) {
        return None;
    }
    if !base.chars().all(sfn_char_ok) || !ext.chars().all(sfn_char_ok) {
        return None;
    }
    let case = |s: &str| -> Option<bool> {
        // Some(true) all lower, Some(false) all upper/no letters, None mixed
        let lower = s.chars().any(|c| c.is_ascii_lowercase());
        let upper = s.chars().any(|c| c.is_ascii_uppercase());
        match (lower, upper) {
            (true, true) => None,
            (true, false) => Some(true),
            _ => Some(false),
        }
    };
    let bl = case(base)?;
    let el = case(ext)?;
    let mut sfn = [b' '; 11];
    for (i, b) in base.bytes().enumerate() {
        sfn[i] = b.to_ascii_uppercase();
    }
    for (i, b) in ext.bytes().enumerate() {
        sfn[8 + i] = b.to_ascii_uppercase();
    }
    if sfn[0] == 0xE5 {
        sfn[0] = 0x05;
    }
    let nt = if bl { 0x08 } else { 0 } | if el { 0x10 } else { 0 };
    Some((sfn, nt))
}

fn name_hash32(name: &str, seed: u32) -> u32 {
    let mut h: u32 = 0x811C_9DC5 ^ seed;
    for u in name.encode_utf16() {
        h = (h ^ u32::from(u)).wrapping_mul(0x0100_0193);
    }
    h
}

/// Short-name candidate `k` (0..8) for a long name.
fn sfn_candidate(name: &str, k: u32) -> [u8; 11] {
    let trimmed = name.trim_start_matches('.');
    let (base, ext) = match trimmed.rfind('.') {
        Some(i) if i > 0 => (&trimmed[..i], &trimmed[i + 1..]),
        _ => (trimmed, ""),
    };
    let conv = |c: char| -> Option<u8> {
        if c == ' ' || c == '.' {
            None
        } else if sfn_char_ok(c) {
            Some(c.to_ascii_uppercase() as u8)
        } else {
            Some(b'_')
        }
    };
    let mut b = [b' '; 8];
    let mut bl = 0;
    for c in base.chars().filter_map(conv) {
        if bl == 8 {
            break;
        }
        b[bl] = c;
        bl += 1;
    }
    if bl == 0 {
        b[0] = b'_';
        bl = 1;
    }
    let mut sfn = [b' '; 11];
    for (i, c) in ext.chars().filter_map(conv).take(3).enumerate() {
        sfn[8 + i] = c;
    }
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    if k < 4 {
        let keep = bl.min(6);
        sfn[..keep].copy_from_slice(&b[..keep]);
        sfn[keep] = b'~';
        sfn[keep + 1] = b'1' + k as u8;
    } else {
        let keep = bl.min(2);
        sfn[..keep].copy_from_slice(&b[..keep]);
        let h = name_hash32(name, k);
        for i in 0..4 {
            sfn[keep + i] = HEX[((h >> (12 - 4 * i)) & 0xF) as usize];
        }
        sfn[keep + 4] = b'~';
        sfn[keep + 5] = b'1' + (k - 4) as u8;
    }
    if sfn[0] == 0xE5 {
        sfn[0] = 0x05;
    }
    sfn
}

fn lfn_units(e: &[u8; 32]) -> [u16; 13] {
    let mut u = [0u16; 13];
    let offs = [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
    for (k, &o) in offs.iter().enumerate() {
        u[k] = rd16(e, o);
    }
    u
}

fn lfn_entry(name: &str, seq: u8, last: bool, cksum: u8) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[0] = seq | if last { 0x40 } else { 0 };
    e[11] = 0x0F;
    e[13] = cksum;
    let start = (usize::from(seq) - 1) * 13;
    let mut it = name.encode_utf16().skip(start);
    let offs = [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
    let mut ended = false;
    for &o in &offs {
        let v = if ended {
            0xFFFF
        } else if let Some(u) = it.next() {
            u
        } else {
            ended = true;
            0x0000
        };
        e[o..o + 2].copy_from_slice(&v.to_le_bytes());
    }
    e
}

/// Validate a new name; returns its UTF-16 length.
fn validate_name<E>(name: &str) -> Result<usize, E> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(Error::InvalidName);
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(Error::InvalidName);
    }
    for c in name.chars() {
        if (c as u32) < 0x20 || "\"*/:<>?\\|".contains(c) {
            return Err(Error::InvalidName);
        }
    }
    let n = name.encode_utf16().count();
    if n > MAX_NAME16 {
        return Err(Error::NameTooLong);
    }
    Ok(n)
}

// --------------------------------------------------------- exFAT helpers

fn set_checksum(mut sum: u16, e: &[u8; 32], first: bool) -> u16 {
    for (i, &b) in e.iter().enumerate() {
        if first && (i == 2 || i == 3) {
            continue;
        }
        sum = (if sum & 1 != 0 { 0x8000u16 } else { 0 }).wrapping_add(sum >> 1).wrapping_add(u16::from(b));
    }
    sum
}

fn exfat_ts(dt: &DateTime) -> u32 {
    let (d, t) = dt.to_fat();
    (u32::from(d) << 16) | u32::from(t)
}

/// What a new entry set describes.
#[derive(Clone, Copy)]
pub(crate) struct NewEntry {
    pub attr: u8,
    pub first: u32,
    pub size: u64,
    pub valid: u64,
    pub contig: bool,
    pub created: DateTime,
    pub modified: DateTime,
}

/// Result of a path walk.
pub(crate) struct Walk<'p> {
    pub parent: DirSpec,
    pub name: &'p str,
    pub info: Option<EntryInfo>,
}

impl<'c, D: BlockDevice> Volume<'c, D> {
    // ----------------------------------------------------- positions

    pub(crate) fn spec_root(&self) -> DirSpec {
        match self.g.kind {
            FsKind::Fat12 | FsKind::Fat16 => DirSpec {
                first: 0,
                contig: false,
                len: u64::from(self.g.root_entries) * 32,
                self_loc: EntryLoc::NONE,
            },
            FsKind::Fat32 => DirSpec {
                first: self.g.root_cluster,
                contig: false,
                len: FAT_DIR_MAX_BYTES,
                self_loc: EntryLoc::NONE,
            },
            FsKind::ExFat => DirSpec {
                first: self.g.root_cluster,
                contig: false,
                len: 256 << 20,
                self_loc: EntryLoc::NONE,
            },
        }
    }

    pub(crate) fn dir_spec(&self, info: &EntryInfo) -> DirSpec {
        if self.g.is_exfat() {
            DirSpec {
                first: info.first_cluster,
                contig: info.contig,
                len: info.size.min(256 << 20),
                self_loc: info.start,
            }
        } else if info.first_cluster == 0 {
            self.spec_root()
        } else {
            DirSpec { first: info.first_cluster, contig: false, len: FAT_DIR_MAX_BYTES, self_loc: info.main }
        }
    }

    fn start_pos(&self, spec: &DirSpec) -> DirPos {
        let fixed = spec.first == 0 && !self.g.is_exfat() && self.g.kind != FsKind::Fat32;
        let ok = fixed || self.g.valid_cluster(spec.first);
        DirPos { cluster: if ok && spec.len > 0 { spec.first } else { END }, off: 0, index: 0 }
    }

    pub(crate) fn cursor_for(&self, spec: DirSpec) -> DirCursor {
        DirCursor { spec, pos: self.start_pos(&spec) }
    }

    /// Cursor at the start of the root directory.
    pub fn root(&self) -> DirCursor {
        self.cursor_for(self.spec_root())
    }

    fn pos_lba(&self, cluster: u32, off: u32) -> u32 {
        if cluster == 0 { self.g.root_start + off / 512 } else { self.g.cluster_lba(cluster) + off / 512 }
    }

    pub(crate) async fn advance(&mut self, spec: &DirSpec, pos: &mut DirPos) -> Result<(), D::Error> {
        if pos.cluster == END {
            return Ok(());
        }
        #[cfg(feature = "std")]
        {
            self.steps = self.steps.wrapping_add(1);
        }
        pos.index += 1;
        pos.off += 32;
        if u64::from(pos.index) * 32 >= spec.len {
            pos.cluster = END;
            return Ok(());
        }
        let unit = if pos.cluster == 0 { self.g.root_entries * 32 } else { self.g.cluster_bytes() };
        if pos.off >= unit {
            if pos.cluster == 0 {
                pos.cluster = END;
                return Ok(());
            }
            match self.next_cluster(pos.cluster, spec.contig).await? {
                Some(n) => {
                    pos.cluster = n;
                    pos.off = 0;
                }
                None => pos.cluster = END,
            }
        }
        Ok(())
    }

    async fn raw_at(&mut self, cluster: u32, off: u32) -> Result<[u8; 32], D::Error> {
        let lba = self.pos_lba(cluster, off);
        let i = self.cache.load(&mut self.dev, lba, Kind::Meta, false).await?;
        let o = (off % 512) as usize;
        let mut e = [0u8; 32];
        e.copy_from_slice(&self.cache.data(i)[o..o + 32]);
        Ok(e)
    }

    /// Read the raw entry at `pos` and advance; `None` at the end.
    pub(crate) async fn dir_raw_next(
        &mut self,
        spec: &mut DirSpec,
        pos: &mut DirPos,
    ) -> Result<Option<[u8; 32]>, D::Error> {
        if pos.cluster == END {
            return Ok(None);
        }
        let e = self.raw_at(pos.cluster, pos.off).await?;
        self.advance(spec, pos).await?;
        Ok(Some(e))
    }

    fn loc_of(p: &DirPos, spec: &DirSpec) -> EntryLoc {
        EntryLoc { cluster: p.cluster, off: p.off, contig: spec.contig }
    }

    /// Step an entry location to the next entry.
    pub(crate) async fn loc_next(&mut self, loc: EntryLoc) -> Result<EntryLoc, D::Error> {
        let spec =
            DirSpec { first: loc.cluster, contig: loc.contig, len: u64::MAX, self_loc: EntryLoc::NONE };
        let mut p = DirPos { cluster: loc.cluster, off: loc.off, index: 0 };
        self.advance(&spec, &mut p).await?;
        if p.cluster == END {
            return Err(Error::Corrupt);
        }
        Ok(EntryLoc { cluster: p.cluster, off: p.off, contig: loc.contig })
    }

    pub(crate) async fn modify_entry(
        &mut self,
        loc: EntryLoc,
        f: impl FnOnce(&mut [u8]),
    ) -> Result<(), D::Error> {
        if loc.cluster == END {
            return Err(Error::Corrupt);
        }
        let lba = self.pos_lba(loc.cluster, loc.off);
        let i = self.cache.load(&mut self.dev, lba, Kind::Meta, false).await?;
        let o = (loc.off % 512) as usize;
        f(&mut self.cache.data_mut(i)[o..o + 32]);
        Ok(())
    }

    // ------------------------------------------------------ set parsing

    async fn next_set<S: Sink>(
        &mut self,
        spec: &DirSpec,
        pos: &mut DirPos,
        sink: &mut S,
        free: Option<&mut FreeTrack>,
    ) -> Result<Option<EntryInfo>, D::Error> {
        if self.g.is_exfat() {
            self.next_set_exfat(spec, pos, sink, free).await
        } else {
            self.next_set_fat(spec, pos, sink, free).await
        }
    }

    /// Free slots from `pos` (an end marker) to the end of the allocation.
    async fn track_tail(
        &mut self,
        spec: &DirSpec,
        pos: &mut DirPos,
        f: &mut FreeTrack,
    ) -> Result<(), D::Error> {
        while pos.cluster != END {
            f.free_at(*pos);
            if f.found {
                break;
            }
            self.advance(spec, pos).await?;
        }
        Ok(())
    }

    async fn next_set_fat<S: Sink>(
        &mut self,
        spec: &DirSpec,
        pos: &mut DirPos,
        sink: &mut S,
        mut free: Option<&mut FreeTrack>,
    ) -> Result<Option<EntryInfo>, D::Error> {
        // (start position, total, checksum, next expected seq, len16)
        let mut lfn: Option<(DirPos, u8, u8, u8, usize)> = None;
        loop {
            if pos.cluster == END {
                return Ok(None);
            }
            let here = *pos;
            let e = self.raw_at(here.cluster, here.off).await?;
            if e[0] == 0 {
                if let Some(f) = free.as_deref_mut() {
                    self.track_tail(spec, pos, f).await?;
                }
                pos.cluster = END;
                return Ok(None);
            }
            self.advance(spec, pos).await?;
            if e[0] == 0xE5 {
                lfn = None;
                if let Some(f) = free.as_deref_mut() {
                    f.free_at(here);
                }
                continue;
            }
            if let Some(f) = free.as_deref_mut() {
                f.used(here);
            }
            let attr = e[11];
            if attr & 0x3F == 0x0F {
                let ord = e[0];
                let units = lfn_units(&e);
                if ord & 0x40 != 0 {
                    let seq = ord & 0x1F;
                    if seq == 0 || seq > 20 {
                        lfn = None;
                        continue;
                    }
                    let valid = units.iter().position(|&u| u == 0).unwrap_or(13);
                    let len16 = (usize::from(seq) - 1) * 13 + valid;
                    if len16 == 0 || len16 > MAX_NAME16 {
                        lfn = None;
                        continue;
                    }
                    sink.reset();
                    sink.set_len(len16);
                    sink.put((usize::from(seq) - 1) * 13, &units[..valid]);
                    lfn = Some((here, seq, e[13], seq - 1, len16));
                } else if let Some((st, total, ck, expect, len16)) = lfn {
                    if expect != 0 && ord == expect && e[13] == ck {
                        sink.put((usize::from(ord) - 1) * 13, &units);
                        lfn = Some((st, total, ck, expect - 1, len16));
                    } else {
                        lfn = None;
                    }
                }
                continue;
            }
            if attr & 0x08 != 0 || e[0] == b'.' {
                lfn = None;
                continue;
            }
            let mut sfn = [0u8; 11];
            sfn.copy_from_slice(&e[..11]);
            let (start, has_lfn, count) = match lfn {
                Some((st, total, ck, 0, _)) if ck == sfn_checksum(&sfn) => (st, true, total + 1),
                _ => (here, false, 1),
            };
            if !has_lfn {
                let mut u = [0u16; 12];
                let n = sfn_units(&e, &mut u);
                sink.reset();
                sink.set_len(n);
                sink.put(0, &u[..n]);
            }
            let first = (u32::from(rd16(&e, 20)) << 16) | u32::from(rd16(&e, 26));
            let first = if self.g.kind == FsKind::Fat32 { first & 0x0FFF_FFFF } else { first & 0xFFFF };
            let size = u64::from(rd32(&e, 28));
            let info = EntryInfo {
                attr,
                first_cluster: first,
                size,
                valid: size,
                contig: false,
                start: Self::loc_of(&start, spec),
                main: Self::loc_of(&here, spec),
                count,
                index: start.index,
                created: DateTime::from_fat(rd16(&e, 16), rd16(&e, 14), e[13]),
                modified: DateTime::from_fat(rd16(&e, 24), rd16(&e, 22), 0),
                sfn,
                has_lfn,
            };
            return Ok(Some(info));
        }
    }

    async fn next_set_exfat<S: Sink>(
        &mut self,
        spec: &DirSpec,
        pos: &mut DirPos,
        sink: &mut S,
        mut free: Option<&mut FreeTrack>,
    ) -> Result<Option<EntryInfo>, D::Error> {
        loop {
            if pos.cluster == END {
                return Ok(None);
            }
            let here = *pos;
            let e = self.raw_at(here.cluster, here.off).await?;
            if e[0] == 0 {
                if let Some(f) = free.as_deref_mut() {
                    self.track_tail(spec, pos, f).await?;
                }
                pos.cluster = END;
                return Ok(None);
            }
            self.advance(spec, pos).await?;
            if e[0] & 0x80 == 0 {
                if let Some(f) = free.as_deref_mut() {
                    f.free_at(here);
                }
                continue;
            }
            if let Some(f) = free.as_deref_mut() {
                f.used(here);
            }
            if e[0] != 0x85 {
                continue;
            }
            let sec = e[1];
            if !(2..=18).contains(&sec) {
                continue;
            }
            let expect = rd16(&e, 2);
            let mut sum = set_checksum(0, &e, true);
            sink.reset();
            // (flags, name_len, vdl, first, len)
            let mut stream: Option<(u8, usize, u64, u32, u64)> = None;
            let mut names = 0usize;
            let mut ok = true;
            let mut n = 0u8;
            while n < sec {
                if pos.cluster == END {
                    ok = false;
                    break;
                }
                let p = *pos;
                let s = self.raw_at(p.cluster, p.off).await?;
                if s[0] & 0xC0 != 0xC0 {
                    ok = false;
                    break;
                }
                self.advance(spec, pos).await?;
                if let Some(f) = free.as_deref_mut() {
                    f.used(p);
                }
                sum = set_checksum(sum, &s, false);
                match s[0] {
                    0xC0 if n == 0 => {
                        let nl = usize::from(s[3]);
                        sink.set_len(nl);
                        stream = Some((s[1], nl, rd64(&s, 8), rd32(&s, 20), rd64(&s, 24)));
                    }
                    0xC1 => {
                        if let Some((_, nl, ..)) = stream {
                            let at = names * 15;
                            if at < nl {
                                let mut u = [0u16; 15];
                                for (k, x) in u.iter_mut().enumerate() {
                                    *x = rd16(&s, 2 + 2 * k);
                                }
                                let take = (nl - at).min(15);
                                sink.put(at, &u[..take]);
                            }
                            names += 1;
                        }
                    }
                    _ if n == 0 => ok = false,
                    _ => {}
                }
                n += 1;
            }
            let Some((flags, nl, vdl, first, len)) = stream else {
                continue;
            };
            if !ok || sum != expect || nl == 0 || names * 15 < nl {
                continue;
            }
            let info = EntryInfo {
                attr: e[4],
                first_cluster: first,
                size: len,
                valid: vdl.min(len),
                contig: flags & 2 != 0,
                start: Self::loc_of(&here, spec),
                main: Self::loc_of(&here, spec),
                count: sec + 1,
                index: here.index,
                created: {
                    let t = rd32(&e, 8);
                    DateTime::from_fat((t >> 16) as u16, t as u16, e[20])
                },
                modified: {
                    let t = rd32(&e, 12);
                    DateTime::from_fat((t >> 16) as u16, t as u16, e[21])
                },
                sfn: [0; 11],
                has_lfn: true,
            };
            return Ok(Some(info));
        }
    }

    /// Look a name up in a directory (case-insensitive).
    pub(crate) async fn find(&mut self, spec: &DirSpec, name: &str) -> Result<Option<EntryInfo>, D::Error> {
        let mut pos = self.start_pos(spec);
        let mut m = Matcher::new(name);
        let mut guard = 0u32;
        while let Some(info) = self.next_set(spec, &mut pos, &mut m, None).await? {
            if m.ok || (info.has_lfn && !self.g.is_exfat() && self.sfn_matches(&info, name)) {
                return Ok(Some(info));
            }
            guard += 1;
            if guard > 8 << 20 {
                return Err(Error::Corrupt);
            }
        }
        Ok(None)
    }

    fn sfn_matches(&self, info: &EntryInfo, name: &str) -> bool {
        let mut e = [0u8; 13];
        e[..11].copy_from_slice(&info.sfn);
        let mut u = [0u16; 12];
        let n = sfn_units(&e, &mut u);
        let mut m = Matcher::new(name);
        m.reset();
        m.set_len(n);
        m.put(0, &u[..n]);
        m.ok
    }

    /// Whether a raw slot can be the first of an entry set: an exFAT file
    /// entry; on FAT a long-name entry with the last-entry flag, or a short
    /// entry that is not free, deleted, a volume label or a dot entry.
    /// Looked at in the cached sector (no copy: this sits in
    /// `open_locator`'s future).
    async fn set_starts_at(&mut self, cluster: u32, off: u32) -> Result<bool, D::Error> {
        let lba = self.pos_lba(cluster, off);
        let i = self.cache.load(&mut self.dev, lba, Kind::Meta, false).await?;
        let o = (off % 512) as usize;
        let (b0, attr) = (self.cache.data(i)[o], self.cache.data(i)[o + 11]);
        Ok(if self.g.is_exfat() {
            b0 == 0x85
        } else if b0 == 0 || b0 == 0xE5 {
            false
        } else if attr & 0x3F == 0x0F {
            b0 & 0x40 != 0
        } else {
            attr & 0x08 == 0 && b0 != b'.'
        })
    }

    // -------------------------------------------------------- public API

    /// Read the next entry into `out`. Returns `false` at the end.
    pub async fn read_dir(&mut self, cur: &mut DirCursor, out: &mut DirEntry) -> Result<bool, D::Error> {
        let spec = cur.spec;
        let mut col = Collector { buf: &mut out.name, len: 0 };
        match self.next_set(&spec, &mut cur.pos, &mut col, None).await? {
            Some(info) => {
                let n = col.len;
                out.name_len = utf16_to_utf8_in_place(&mut out.name, U16_AT, n) as u16;
                out.info = info;
                out.dir = spec;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Move a cursor to entry index `index` (a value from
    /// [`DirEntry::index`] or [`DirCursor::index`]). Reads nothing but the
    /// FAT for the clusters it crosses: inside a cluster (or the fixed
    /// root) it moves in one step, so a seek costs one step per cluster,
    /// not one per entry slot. It used to step slot by slot inside the
    /// entry's cluster: up to 4,095 steps in a 128 KiB exFAT cluster, for
    /// every Up next title (AU-PERF, 2026-11-25).
    pub async fn seek_dir(&mut self, cur: &mut DirCursor, index: u32) -> Result<(), D::Error> {
        if index < cur.pos.index || cur.pos.cluster == END {
            cur.pos = self.start_pos(&cur.spec);
        }
        let spec = cur.spec;
        while cur.pos.index < index && cur.pos.cluster != END {
            let unit = if cur.pos.cluster == 0 { self.g.root_entries * 32 } else { self.g.cluster_bytes() };
            // Slots from here to the end of the unit, this one included.
            let left = unit.saturating_sub(cur.pos.off) / 32;
            // To the entry if it is in this unit, else to the unit's last
            // slot (`advance` then crosses into the next cluster).
            let k = (index - cur.pos.index).min(left.saturating_sub(1));
            cur.pos.index += k;
            cur.pos.off += k * 32;
            if u64::from(cur.pos.index) * 32 >= spec.len {
                cur.pos.cluster = END;
                break;
            }
            if cur.pos.index < index {
                self.advance(&spec, &mut cur.pos).await?;
            }
        }
        Ok(())
    }

    /// The directory a [`Locator`] names and its entry index.
    fn locator_spec(&self, loc: Locator) -> (DirSpec, u32) {
        let contig = loc.index & (1 << 31) != 0;
        let index = loc.index & !(1 << 31);
        let spec = if loc.dir == self.spec_root().first {
            self.spec_root()
        } else {
            DirSpec {
                first: loc.dir,
                contig,
                len: if self.g.is_exfat() { 256 << 20 } else { FAT_DIR_MAX_BYTES },
                self_loc: EntryLoc::NONE,
            }
        };
        (spec, index)
    }

    /// A cursor on the set a [`Locator`] points at; `NotFound` if no set
    /// starts there (the entry is gone: `read_dir` would look on for the
    /// next set, through a run of deleted entries or to the end of a
    /// directory that changed, only to fail).
    async fn seek_locator(&mut self, loc: Locator) -> Result<(DirCursor, u32), D::Error> {
        let (spec, index) = self.locator_spec(loc);
        let mut cur = self.cursor_for(spec);
        self.seek_dir(&mut cur, index).await?;
        if cur.pos.cluster == END || cur.pos.index != index {
            return Err(Error::NotFound);
        }
        if !self.set_starts_at(cur.pos.cluster, cur.pos.off).await? {
            return Err(Error::NotFound);
        }
        Ok((cur, index))
    }

    /// Re-read the entry a [`Locator`] points at. (Written out, not on
    /// [`Volume::seek_locator`]: its future is on the firmware's deepest
    /// card path, every listing row and Up next title.)
    pub async fn open_locator(&mut self, loc: Locator, out: &mut DirEntry) -> Result<(), D::Error> {
        let (spec, index) = self.locator_spec(loc);
        let mut cur = self.cursor_for(spec);
        self.seek_dir(&mut cur, index).await?;
        // As `seek_locator`.
        if cur.pos.cluster == END || cur.pos.index != index {
            return Err(Error::NotFound);
        }
        if !self.set_starts_at(cur.pos.cluster, cur.pos.off).await? {
            return Err(Error::NotFound);
        }
        if !self.read_dir(&mut cur, out).await? || out.info.index != index {
            return Err(Error::NotFound);
        }
        Ok(())
    }

    /// The entry a [`Locator`] points at if it is still named `name`
    /// (compared as a path walk compares: case-insensitive, or the 8.3
    /// alias); `NotFound` otherwise. No name buffer (for
    /// [`Volume::open_at`]).
    pub(crate) async fn find_at(&mut self, loc: Locator, name: &str) -> Result<EntryInfo, D::Error> {
        let (mut cur, index) = self.seek_locator(loc).await?;
        let mut m = Matcher::new(name);
        let spec = cur.spec;
        let Some(info) = self.next_set(&spec, &mut cur.pos, &mut m, None).await? else {
            return Err(Error::NotFound);
        };
        let named = m.ok || (info.has_lfn && !self.g.is_exfat() && self.sfn_matches(&info, name));
        if info.index != index || !named {
            return Err(Error::NotFound);
        }
        Ok(info)
    }

    /// Resolve a path: parent directory, last component and its entry.
    pub(crate) async fn walk<'p>(&mut self, path: &'p str) -> Result<Walk<'p>, D::Error> {
        if components(path).count() > MAX_DEPTH {
            return Err(Error::NameTooLong);
        }
        if components(path).any(|c| c == "..") {
            return Err(Error::InvalidName);
        }
        // The last component is looked up in its parent; a cached directory
        // can stand for the parent (or any ancestor) only.
        let last = components(path).last().map_or(0, |c| c.as_ptr() as usize - path.as_ptr() as usize);
        let cached = self.walks.as_deref_mut().and_then(|w| {
            let r = w.lookup(path.get(..last).unwrap_or(""))?;
            w.hits = w.hits.wrapping_add(1);
            Some(r)
        });
        let (mut spec, from) = match cached {
            Some(r) => r,
            None => (self.spec_root(), 0),
        };
        let rest = path.get(from..).unwrap_or("");
        let mut it = components(rest);
        let Some(mut name) = it.next() else {
            return Ok(Walk { parent: spec, name: "", info: None });
        };
        loop {
            let info = self.find(&spec, name).await?;
            match it.next() {
                None => {
                    return Ok(Walk { parent: spec, name, info });
                }
                Some(next) => {
                    let info = info.ok_or(Error::NotFound)?;
                    if !info.is_dir() {
                        return Err(Error::NotADirectory);
                    }
                    spec = self.dir_spec(&info);
                    // `path` up to the end of this directory's name.
                    let end = name.as_ptr() as usize - path.as_ptr() as usize + name.len();
                    if let (Some(k), Some(w)) = (path.as_bytes().get(..end), self.walks.as_deref_mut()) {
                        w.insert(k, spec);
                    }
                    name = next;
                }
            }
        }
    }

    /// Walks served from the directory cache so far.
    #[must_use]
    pub fn walk_hits(&self) -> u32 {
        self.walks.as_deref().map_or(0, |w| w.hits)
    }

    /// Lend the volume a directory cache for its path walks (emptied now:
    /// it may hold another volume's directories).
    pub fn set_walk_cache(&mut self, w: &'c mut WalkCache) {
        w.clear();
        self.walks = Some(w);
    }

    fn forget_walks(&mut self) {
        if let Some(w) = self.walks.as_deref_mut() {
            w.clear();
        }
    }

    /// Open a directory for iteration.
    pub async fn open_dir(&mut self, path: &str) -> Result<DirCursor, D::Error> {
        let w = self.walk(path).await?;
        if w.name.is_empty() {
            return Ok(self.root());
        }
        let info = w.info.ok_or(Error::NotFound)?;
        if !info.is_dir() {
            return Err(Error::NotADirectory);
        }
        Ok(self.cursor_for(self.dir_spec(&info)))
    }

    /// Cursor for a directory found by `read_dir`.
    pub fn dir_cursor(&self, entry: &DirEntry) -> Result<DirCursor, D::Error> {
        if !entry.is_dir() {
            return Err(Error::NotADirectory);
        }
        Ok(self.cursor_for(self.dir_spec(&entry.info)))
    }

    /// Look up a path and fill `out` with its entry.
    pub async fn stat(&mut self, path: &str, out: &mut DirEntry) -> Result<(), D::Error> {
        let w = self.walk(path).await?;
        let info = w.info.ok_or(Error::NotFound)?;
        let mut cur = DirCursor {
            spec: w.parent,
            pos: DirPos { cluster: info.start.cluster, off: info.start.off, index: info.index },
        };
        if !self.read_dir(&mut cur, out).await? {
            return Err(Error::Corrupt);
        }
        Ok(())
    }

    /// True if the path exists.
    pub async fn exists(&mut self, path: &str) -> Result<bool, D::Error> {
        match self.walk(path).await {
            Ok(w) => Ok(w.name.is_empty() || w.info.is_some()),
            Err(Error::NotFound | Error::NotADirectory) => Ok(false),
            Err(e) => Err(e),
        }
    }

    // ------------------------------------------------------ mutation

    /// Count clusters of a directory chain and return the last one.
    async fn dir_last_cluster(&mut self, spec: &DirSpec) -> Result<(u32, u32), D::Error> {
        let mut c = spec.first;
        let mut n = 1u32;
        if spec.contig {
            let cnt = (spec.len >> self.g.cbits()).clamp(1, u64::from(self.g.cluster_count)) as u32;
            let last = spec.first.wrapping_add(cnt - 1);
            if !self.g.valid_cluster(spec.first) || !self.g.valid_cluster(last) {
                return Err(Error::Corrupt);
            }
            return Ok((last, cnt));
        }
        loop {
            match self.fat_get(c).await? {
                crate::volume::Link::Next(x) => {
                    c = x;
                    n += 1;
                    if n > self.g.cluster_count {
                        return Err(Error::Corrupt);
                    }
                }
                crate::volume::Link::End => return Ok((c, n)),
                _ => return Err(Error::Corrupt),
            }
        }
    }

    /// Add one zeroed cluster to a directory; updates `spec`.
    async fn grow_dir(&mut self, spec: &mut DirSpec) -> Result<u32, D::Error> {
        self.forget_walks();
        if spec.first == 0 && !self.g.is_exfat() {
            return Err(Error::DirectoryFull);
        }
        let (last, count) = self.dir_last_cluster(spec).await?;
        let cb = u64::from(self.g.cluster_bytes());
        let new_len = (u64::from(count) + 1) * cb;
        let max = if self.g.is_exfat() { 256 << 20 } else { FAT_DIR_MAX_BYTES };
        if new_len > max {
            return Err(Error::DirectoryFull);
        }
        let new = self.alloc_cluster(last + 1, !spec.contig).await?;
        self.zero_cluster(new, false).await?;
        if self.g.is_exfat() {
            if spec.contig && new != last + 1 {
                // convert the directory to a FAT chain
                for k in 0..count - 1 {
                    self.fat_set(spec.first + k, spec.first + k + 1).await?;
                }
                self.fat_set(last, new).await?;
                self.fat_set(new, self.g.eoc()).await?;
                spec.contig = false;
            } else if !spec.contig {
                self.fat_set(last, new).await?;
            }
            spec.len = new_len;
            if spec.self_loc.cluster != END {
                let contig = spec.contig;
                let loc = spec.self_loc;
                let sl = self.loc_next(loc).await?;
                self.modify_entry(sl, |s| {
                    s[1] = (s[1] & !2) | 1 | if contig { 2 } else { 0 };
                    s[8..16].copy_from_slice(&new_len.to_le_bytes());
                    s[24..32].copy_from_slice(&new_len.to_le_bytes());
                })
                .await?;
                self.exfat_fix_checksum(loc).await?;
            }
        } else {
            self.fat_set(last, new).await?;
        }
        Ok(new)
    }

    pub(crate) async fn exfat_fix_checksum(&mut self, loc: EntryLoc) -> Result<(), D::Error> {
        let e = self.raw_at(loc.cluster, loc.off).await?;
        if e[0] != 0x85 {
            return Err(Error::Corrupt);
        }
        let sec = e[1];
        let mut sum = set_checksum(0, &e, true);
        let mut l = loc;
        for _ in 0..sec {
            l = self.loc_next(l).await?;
            let s = self.raw_at(l.cluster, l.off).await?;
            sum = set_checksum(sum, &s, false);
        }
        self.modify_entry(loc, |e| e[2..4].copy_from_slice(&sum.to_le_bytes())).await
    }

    /// exFAT name hash with the volume's up-case table.
    async fn exfat_name_hash(&mut self, name: &str) -> Result<u16, D::Error> {
        let mut h: u16 = 0;
        for u in name.encode_utf16() {
            let up = self.vol_upcase(u).await?;
            for b in up.to_le_bytes() {
                h = (if h & 1 != 0 { 0x8000u16 } else { 0 }).wrapping_add(h >> 1).wrapping_add(u16::from(b));
            }
        }
        Ok(h)
    }

    /// Create an entry set for `name` in `parent`. Fails if the name exists.
    pub(crate) async fn create_entry(
        &mut self,
        parent: &mut DirSpec,
        name: &str,
        ne: NewEntry,
    ) -> Result<EntryInfo, D::Error> {
        let len16 = validate_name(name)?;
        let exfat = self.g.is_exfat();
        let short = if exfat { None } else { short_form(name) };
        let need: u32 = if exfat {
            2 + len16.div_ceil(15) as u32
        } else if short.is_some() {
            1
        } else {
            1 + len16.div_ceil(13) as u32
        };
        // One pass: existence, free run, short-name collisions.
        let mut cands = [[0u8; 11]; 8];
        if !exfat && short.is_none() {
            for (k, c) in cands.iter_mut().enumerate() {
                *c = sfn_candidate(name, k as u32);
            }
        }
        let mut taken: u8 = 0;
        let mut track = FreeTrack::new(need);
        let mut pos = self.start_pos(parent);
        let mut m = Matcher::new(name);
        let spec = *parent;
        while let Some(info) = self.next_set(&spec, &mut pos, &mut m, Some(&mut track)).await? {
            if m.ok || (info.has_lfn && !exfat && self.sfn_matches(&info, name)) {
                return Err(Error::AlreadyExists);
            }
            if !exfat {
                if let Some((s, _)) = short
                    && s == info.sfn
                {
                    return Err(Error::AlreadyExists);
                }
                for (k, c) in cands.iter().enumerate() {
                    if *c == info.sfn {
                        taken |= 1 << k;
                    }
                }
            }
        }
        let sfn = match short {
            Some((s, _)) => s,
            None if exfat => [0; 11],
            None => {
                let Some(k) = (0..8).find(|k| taken & (1 << k) == 0) else {
                    return Err(Error::DirectoryFull);
                };
                cands[k]
            }
        };
        let nt = short.map_or(0, |(_, nt)| nt);
        // Space.
        let start = if track.found {
            track.run_start
        } else {
            let trailing = if track.run_len > 0 { track.run_len } else { 0 };
            let cb = self.g.cluster_bytes() / 32;
            let missing = need - trailing;
            let clusters = missing.div_ceil(cb);
            let mut first_new = END;
            if parent.first == 0 && !exfat {
                return Err(Error::DirectoryFull);
            }
            let (_, count_before) = self.dir_last_cluster(parent).await?;
            for _ in 0..clusters {
                let c = self.grow_dir(parent).await?;
                if first_new == END {
                    first_new = c;
                }
            }
            if trailing > 0 {
                track.run_start
            } else {
                DirPos { cluster: first_new, off: 0, index: count_before * cb }
            }
        };
        // Write the entries.
        let spec = *parent;
        let mut p = start;
        let (d, t) = ne.modified.to_fat();
        let (cd, ct) = ne.created.to_fat();
        let mut main = EntryLoc::NONE;
        if exfat {
            let hash = self.exfat_name_hash(name).await?;
            let gen_entry = |k: u32| -> [u8; 32] {
                let mut e = [0u8; 32];
                match k {
                    0 => {
                        e[0] = 0x85;
                        e[1] = (need - 1) as u8;
                        e[4] = ne.attr;
                        e[8..12].copy_from_slice(&exfat_ts(&ne.created).to_le_bytes());
                        e[12..16].copy_from_slice(&exfat_ts(&ne.modified).to_le_bytes());
                        e[16..20].copy_from_slice(&exfat_ts(&ne.modified).to_le_bytes());
                        e[20] = ne.created.tenms();
                        e[21] = ne.modified.tenms();
                    }
                    1 => {
                        e[0] = 0xC0;
                        e[1] = if ne.first != 0 { 1 } else { 0 } | if ne.contig { 2 } else { 0 };
                        e[3] = len16 as u8;
                        e[4..6].copy_from_slice(&hash.to_le_bytes());
                        e[8..16].copy_from_slice(&ne.valid.to_le_bytes());
                        e[20..24].copy_from_slice(&ne.first.to_le_bytes());
                        e[24..32].copy_from_slice(&ne.size.to_le_bytes());
                    }
                    _ => {
                        e[0] = 0xC1;
                        let mut it = name.encode_utf16().skip((k as usize - 2) * 15);
                        for j in 0..15 {
                            let u = it.next().unwrap_or(0);
                            e[2 + 2 * j..4 + 2 * j].copy_from_slice(&u.to_le_bytes());
                        }
                    }
                }
                e
            };
            let mut sum = 0u16;
            for k in 0..need {
                sum = set_checksum(sum, &gen_entry(k), k == 0);
            }
            for k in 0..need {
                if p.cluster == END {
                    return Err(Error::Corrupt);
                }
                let mut e = gen_entry(k);
                if k == 0 {
                    e[2..4].copy_from_slice(&sum.to_le_bytes());
                    main = Self::loc_of(&p, &spec);
                }
                let loc = Self::loc_of(&p, &spec);
                self.modify_entry(loc, |d| d.copy_from_slice(&e)).await?;
                self.advance(&spec, &mut p).await?;
            }
        } else {
            let ck = sfn_checksum(&sfn);
            let lfn_n = need - 1;
            for k in 0..need {
                if p.cluster == END {
                    return Err(Error::Corrupt);
                }
                let e = if k < lfn_n {
                    let seq = (lfn_n - k) as u8;
                    lfn_entry(name, seq, k == 0, ck)
                } else {
                    let mut e = [0u8; 32];
                    e[..11].copy_from_slice(&sfn);
                    e[11] = ne.attr;
                    e[12] = nt;
                    e[13] = ne.created.tenms();
                    e[14..16].copy_from_slice(&ct.to_le_bytes());
                    e[16..18].copy_from_slice(&cd.to_le_bytes());
                    e[18..20].copy_from_slice(&d.to_le_bytes());
                    e[20..22].copy_from_slice(&((ne.first >> 16) as u16).to_le_bytes());
                    e[22..24].copy_from_slice(&t.to_le_bytes());
                    e[24..26].copy_from_slice(&d.to_le_bytes());
                    e[26..28].copy_from_slice(&(ne.first as u16).to_le_bytes());
                    e[28..32].copy_from_slice(&(ne.size.min(u64::from(u32::MAX)) as u32).to_le_bytes());
                    main = Self::loc_of(&p, &spec);
                    e
                };
                let loc = Self::loc_of(&p, &spec);
                self.modify_entry(loc, |d| d.copy_from_slice(&e)).await?;
                self.advance(&spec, &mut p).await?;
            }
        }
        Ok(EntryInfo {
            attr: ne.attr,
            first_cluster: ne.first,
            size: ne.size,
            valid: ne.valid,
            contig: ne.contig,
            start: Self::loc_of(&start, &spec),
            main,
            count: need as u8,
            index: start.index,
            created: ne.created,
            modified: ne.modified,
            sfn,
            has_lfn: exfat || short.is_none(),
        })
    }

    /// Mark an entry set deleted.
    pub(crate) async fn delete_entry(&mut self, info: &EntryInfo) -> Result<(), D::Error> {
        self.forget_walks();
        let exfat = self.g.is_exfat();
        let mut l = info.start;
        for k in 0..info.count {
            if k > 0 {
                l = self.loc_next(l).await?;
            }
            self.modify_entry(l, |e| {
                if exfat {
                    e[0] &= 0x7F;
                } else {
                    e[0] = 0xE5;
                }
            })
            .await?;
        }
        Ok(())
    }

    /// Undo `delete_entry` (used when a rename cannot complete).
    async fn undelete_entry(&mut self, info: &EntryInfo) -> Result<(), D::Error> {
        self.forget_walks();
        let exfat = self.g.is_exfat();
        let mut l = info.start;
        let lfn = info.count.saturating_sub(1);
        let sfn0 = info.sfn[0];
        for k in 0..info.count {
            if k > 0 {
                l = self.loc_next(l).await?;
            }
            self.modify_entry(l, |e| {
                if exfat {
                    e[0] |= 0x80;
                } else if k < lfn {
                    e[0] = (lfn - k) | if k == 0 { 0x40 } else { 0 };
                } else {
                    e[0] = sfn0;
                }
            })
            .await?;
        }
        Ok(())
    }

    async fn dir_is_empty(&mut self, info: &EntryInfo) -> Result<bool, D::Error> {
        let spec = self.dir_spec(info);
        let mut pos = self.start_pos(&spec);
        Ok(self.next_set(&spec, &mut pos, &mut NoSink, None).await?.is_none())
    }

    /// Create a directory.
    pub async fn create_dir(&mut self, path: &str) -> Result<(), D::Error> {
        self.check_prefix(path)?;
        self.begin_write().await?;
        let w = self.walk(path).await?;
        if w.name.is_empty() || w.info.is_some() {
            return Err(Error::AlreadyExists);
        }
        validate_name::<D::Error>(w.name)?;
        let mut parent = w.parent;
        let c = self.alloc_cluster(0, !self.g.is_exfat()).await?;
        let now = self.now;
        if self.g.is_exfat() {
            self.zero_cluster(c, false).await?;
        } else {
            self.zero_cluster(c, true).await?;
            let lba = self.g.cluster_lba(c);
            let i = self.cache.load(&mut self.dev, lba, Kind::Meta, true).await?;
            let parent_cluster = if parent.first == self.spec_root().first { 0 } else { parent.first };
            let (d, t) = now.to_fat();
            let s = self.cache.data_mut(i);
            for (k, (nm, cl)) in [(*b".          ", c), (*b"..         ", parent_cluster)].iter().enumerate()
            {
                let e = &mut s[k * 32..k * 32 + 32];
                e[..11].copy_from_slice(nm);
                e[11] = ATTR_DIR;
                e[14..16].copy_from_slice(&t.to_le_bytes());
                e[16..18].copy_from_slice(&d.to_le_bytes());
                e[18..20].copy_from_slice(&d.to_le_bytes());
                e[20..22].copy_from_slice(&((cl >> 16) as u16).to_le_bytes());
                e[22..24].copy_from_slice(&t.to_le_bytes());
                e[24..26].copy_from_slice(&d.to_le_bytes());
                e[26..28].copy_from_slice(&(*cl as u16).to_le_bytes());
            }
            // "." and ".." must be on the medium before the entry naming
            // the directory (both are directory sectors in the cache).
            self.cache.flush_range(&mut self.dev, lba, 1).await?;
        }
        let cb = u64::from(self.g.cluster_bytes());
        let ne = NewEntry {
            attr: ATTR_DIR,
            first: c,
            size: if self.g.is_exfat() { cb } else { 0 },
            valid: if self.g.is_exfat() { cb } else { 0 },
            contig: self.g.is_exfat(),
            created: now,
            modified: now,
        };
        match self.create_entry(&mut parent, w.name, ne).await {
            Ok(_) => Ok(()),
            Err(e) => {
                let _ = self.release_cluster(c).await;
                Err(e)
            }
        }
    }

    /// Remove a file or an empty directory.
    pub async fn remove(&mut self, path: &str) -> Result<(), D::Error> {
        self.check_prefix(path)?;
        self.begin_write().await?;
        let w = self.walk(path).await?;
        if w.name.is_empty() {
            return Err(Error::InvalidName);
        }
        let info = w.info.ok_or(Error::NotFound)?;
        if info.is_dir() && !self.dir_is_empty(&info).await? {
            return Err(Error::DirectoryNotEmpty);
        }
        self.delete_entry(&info).await?;
        // The entry must be gone on disk before its clusters can be reused.
        self.cache.flush(&mut self.dev).await?;
        self.free_entry_clusters(&info).await
    }

    pub(crate) async fn free_entry_clusters(&mut self, info: &EntryInfo) -> Result<(), D::Error> {
        if info.first_cluster == 0 {
            return Ok(());
        }
        let contig = if self.g.is_exfat() && info.contig {
            Some(info.size.div_ceil(u64::from(self.g.cluster_bytes())) as u32)
        } else {
            None
        };
        self.free_chain(info.first_cluster, contig).await
    }

    /// Rename or move a file or directory. An existing *file* at `to` is
    /// replaced. The old entry is removed (and that reaches the medium)
    /// before the new one is written: a power cut in between loses the name
    /// and leaks the clusters (fsck recovers them as a lost chain), but never
    /// leaves two entries sharing clusters. If writing the new entry fails,
    /// the old one is restored.
    pub async fn rename(&mut self, from: &str, to: &str) -> Result<(), D::Error> {
        self.check_prefix(from)?;
        self.check_prefix(to)?;
        self.begin_write().await?;
        let src = self.walk(from).await?;
        if src.name.is_empty() {
            return Err(Error::InvalidName);
        }
        let si = src.info.ok_or(Error::NotFound)?;
        if si.is_dir() {
            // refuse moving a directory into itself
            let mut a = components(to);
            let mut inside = true;
            for c in components(from) {
                match a.next() {
                    Some(x) if crate::unicode::eq_ignore_case(x, c) => {}
                    _ => {
                        inside = false;
                        break;
                    }
                }
            }
            if inside && a.next().is_some() {
                return Err(Error::InvalidName);
            }
        }
        let dst = self.walk(to).await?;
        if dst.name.is_empty() {
            return Err(Error::InvalidName);
        }
        validate_name::<D::Error>(dst.name)?;
        let mut parent = dst.parent;
        if let Some(di) = dst.info
            && di.start != si.start
        {
            if di.is_dir() || si.is_dir() {
                return Err(Error::AlreadyExists);
            }
            self.delete_entry(&di).await?;
            self.cache.flush(&mut self.dev).await?;
            self.free_entry_clusters(&di).await?;
        }
        let ne = NewEntry {
            attr: si.attr,
            first: si.first_cluster,
            size: si.size,
            valid: si.valid,
            contig: si.contig,
            created: si.created,
            modified: si.modified,
        };
        self.delete_entry(&si).await?;
        self.cache.flush(&mut self.dev).await?;
        if let Err(e) = self.create_entry(&mut parent, dst.name, ne).await {
            self.undelete_entry(&si).await?;
            return Err(e);
        }
        if si.is_dir()
            && !self.g.is_exfat()
            && parent.first != src.parent.first
            && self.g.valid_cluster(si.first_cluster)
        {
            // fix ".." in the moved directory
            let pc = if parent.first == self.spec_root().first && self.g.kind == FsKind::Fat32 {
                0
            } else {
                parent.first
            };
            let dot = EntryLoc { cluster: si.first_cluster, off: 32, contig: false };
            self.modify_entry(dot, |e| {
                if &e[..2] == b".." {
                    e[20..22].copy_from_slice(&((pc >> 16) as u16).to_le_bytes());
                    e[26..28].copy_from_slice(&(pc as u16).to_le_bytes());
                }
            })
            .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    /// Regression (2026-11-25): with round-robin eviction the art and
    /// library walks pushed `/.orpheus/index` out of the 3-slot cache and
    /// every listing searched the card's root again (60 ms on the player).
    /// A directory in use survives walks that pass through others.
    #[test]
    fn regress_a_walk_entry_in_use_survives_other_walks() {
        let spec = |f| DirSpec { first: f, contig: false, len: 4096, self_loc: EntryLoc::NONE };
        let mut w = WalkCache::EMPTY;
        w.insert(b"/.orpheus", spec(10));
        w.insert(b"/.orpheus/index", spec(11));
        for round in 0..50u32 {
            // A listing: the index folder is found in the cache.
            assert!(w.lookup("/.orpheus/index/3").is_some_and(|(s, n)| s.first == 11 && n == 15), "round {round}");
            // Art and library walks through other folders.
            for k in 0..4u32 {
                let key = [b'/', b'M', b'0' + (k as u8), b'/', b'a' + (round % 26) as u8];
                w.insert(&key, spec(100 + k));
            }
        }
    }
    use super::*;

    #[test]
    fn short_forms() {
        assert_eq!(short_form("README.TXT").unwrap().0, *b"README  TXT");
        assert_eq!(short_form("readme.txt").unwrap().1, 0x18);
        assert_eq!(short_form("README.txt").unwrap().1, 0x10);
        assert!(short_form("ReadMe.txt").is_none());
        assert!(short_form("long name.txt").is_none());
        assert!(short_form("a.b.c").is_none());
        assert!(short_form("toolongname.txt").is_none());
        assert!(short_form("x+y.txt").is_none());
    }

    #[test]
    fn candidates() {
        assert_eq!(&sfn_candidate("Long File Name.mp3", 0), b"LONGFI~1MP3");
        assert_eq!(&sfn_candidate("Long File Name.mp3", 3), b"LONGFI~4MP3");
        let c = sfn_candidate("Пётр.flac", 0);
        assert_eq!(&c[..3], b"___");
        assert_eq!(&c[8..], b"FLA");
        let h = sfn_candidate("x", 5);
        assert_eq!(&h[5..7], b"~2");
        assert_eq!(&sfn_candidate(".hidden", 0)[..8], b"HIDDEN~1");
    }

    #[test]
    fn checksums() {
        // known value: "FOO     BAR"
        let s = *b"FOO     BAR";
        let mut sum: u8 = 0;
        for &b in &s {
            sum = sum.rotate_right(1).wrapping_add(b);
        }
        assert_eq!(sfn_checksum(&s), sum);
    }

    #[test]
    fn name_validation() {
        assert!(validate_name::<()>("ok name.txt").is_ok());
        assert!(validate_name::<()>("bad?").is_err());
        assert!(validate_name::<()>("trailing.").is_err());
        assert!(validate_name::<()>("..").is_err());
        let long: std::string::String = core::iter::repeat_n('x', 256).collect();
        assert_eq!(validate_name::<()>(&long), Err(Error::NameTooLong));
    }
}
