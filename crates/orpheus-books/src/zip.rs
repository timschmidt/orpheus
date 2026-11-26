//! Zip archives (EPUB's container, APPNOTE 6.3): the end record, the
//! central directory read as a stream of entries, local headers, CRC-32,
//! and the paths that name entries. Only what EPUB needs: stored and
//! deflated entries of one-disk archives; zip64, encrypted entries and
//! other methods are refused (the caller says so to the reader).
//!
//! Entries are found by a hash of their name (FNV-1a, 64 bits, ASCII
//! letters folded and `\` read as `/`, as some packers write it): the
//! directory is walked again for each look-up, so nothing of it is held.

/// End of central directory record: its fixed part.
pub const END: usize = 22;
/// The most an archive comment adds after it.
pub const MAX_COMMENT: u32 = 65_535;
/// Central directory file header: its fixed part.
pub const CD_HEADER: usize = 46;
/// Local file header: its fixed part.
pub const LOCAL: usize = 30;

const END_SIG: [u8; 4] = *b"PK\x05\x06";
const CD_SIG: [u8; 4] = *b"PK\x01\x02";
const LOCAL_SIG: [u8; 4] = *b"PK\x03\x04";
const ZIP64_LOCATOR_SIG: [u8; 4] = *b"PK\x06\x07";

/// Method: stored.
pub const STORED: u16 = 0;
/// Method: deflated.
pub const DEFLATED: u16 = 8;

static CRC_TABLE: [u32; 256] = {
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

/// CRC-32 (zip's): `crc` so far (0 to start), then `data`.
#[must_use]
pub fn crc32(crc: u32, data: &[u8]) -> u32 {
    let mut c = !crc;
    for &b in data {
        c = CRC_TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

fn rd16(b: &[u8], o: usize) -> u16 {
    match b.get(o..o + 2) {
        Some(s) => u16::from_le_bytes([s[0], s[1]]),
        None => 0,
    }
}

fn rd32(b: &[u8], o: usize) -> u32 {
    match b.get(o..o + 4) {
        Some(s) => u32::from_le_bytes([s[0], s[1], s[2], s[3]]),
        None => 0,
    }
}

/// FNV-1a (64 bits): the start value.
pub const FNV64: u64 = 0xCBF2_9CE4_8422_2325;

/// A name's hash, one byte more (ASCII folded, `\` as `/`).
#[must_use]
pub fn name_byte(h: u64, b: u8) -> u64 {
    let b = if b == b'\\' { b'/' } else { b.to_ascii_lowercase() };
    (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3)
}

/// A name's hash.
#[must_use]
pub fn name_hash(name: &[u8]) -> u64 {
    name.iter().fold(FNV64, |h, &b| name_byte(h, b))
}

/// Why an archive is refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ZipError {
    /// No end record, or one that points outside the file.
    NotZip,
    /// A zip64 archive (over 4 GiB or 65,535 entries).
    Zip64,
    /// Split over several files.
    MultiDisk,
}

/// What the end record says.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct End {
    pub entries: u16,
    pub cd_size: u32,
    /// Where the central directory is (with `bias`).
    pub cd_at: u32,
    /// Bytes before the archive (a self-extractor's stub): added to every
    /// offset the archive gives.
    pub bias: u32,
}

/// The end record in `buf` (the file's bytes from `at`; the file is
/// `len` bytes long), searched from its end. `None`: not in `buf` (look
/// before it, if the comment could reach that far).
#[must_use]
pub fn find_end(buf: &[u8], at: u32, len: u32) -> Option<Result<End, ZipError>> {
    let last = buf.len().checked_sub(END)?;
    for i in (0..=last).rev() {
        if buf[i..i + 4] != END_SIG {
            continue;
        }
        let pos = at + i as u32;
        let comment = u32::from(rd16(buf, i + 20));
        if u64::from(pos) + END as u64 + u64::from(comment) > u64::from(len) {
            continue;
        }
        if rd16(buf, i + 4) != 0 || rd16(buf, i + 6) != 0 {
            return Some(Err(ZipError::MultiDisk));
        }
        let entries = rd16(buf, i + 10);
        let (cd_size, cd_off) = (rd32(buf, i + 12), rd32(buf, i + 16));
        let locator = i >= 20 && buf[i - 20..i - 16] == ZIP64_LOCATOR_SIG;
        if entries == 0xFFFF || cd_size == u32::MAX || cd_off == u32::MAX || locator {
            return Some(Err(ZipError::Zip64));
        }
        let Some(cd_end) = cd_off.checked_add(cd_size).filter(|&e| e <= pos) else {
            return Some(Err(ZipError::NotZip));
        };
        let bias = pos - cd_end;
        return Some(Ok(End { entries, cd_size, cd_at: cd_off + bias, bias }));
    }
    None
}

/// A central directory entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Entry {
    /// [`name_hash`] of its name.
    pub hash: u64,
    /// Its local header (with the archive's bias).
    pub local: u32,
    pub csize: u32,
    pub usize: u32,
    pub crc: u32,
    pub method: u16,
    pub flags: u16,
}

impl Entry {
    /// Encrypted (traditional or strong): refused.
    #[must_use]
    pub fn encrypted(&self) -> bool {
        self.flags & 1 != 0
    }

    /// Sizes or offset in a zip64 field: refused.
    #[must_use]
    pub fn zip64(&self) -> bool {
        self.csize == u32::MAX || self.usize == u32::MAX || self.local == u32::MAX
    }

    /// Stored or deflated, not encrypted, not zip64.
    #[must_use]
    pub fn readable(&self) -> bool {
        !self.encrypted() && !self.zip64() && (self.method == STORED || self.method == DEFLATED)
    }
}

/// Bytes of a name kept (a longer one is hashed whole, kept cut).
pub const NAME_MAX: usize = 255;

/// A path or name in a fixed buffer (bytes: zip names need not be
/// UTF-8).
#[derive(Clone, Copy, Debug)]
pub struct Name {
    b: [u8; NAME_MAX],
    n: u8,
    /// Longer than kept.
    pub cut: bool,
}

impl Default for Name {
    fn default() -> Self {
        Name::new()
    }
}

impl Name {
    #[must_use]
    pub const fn new() -> Name {
        Name { b: [0; NAME_MAX], n: 0, cut: false }
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.b[..usize::from(self.n)]
    }

    pub fn clear(&mut self) {
        self.n = 0;
        self.cut = false;
    }

    pub fn push(&mut self, c: u8) {
        if let Some(s) = self.b.get_mut(usize::from(self.n)) {
            *s = c;
            self.n += 1;
        } else {
            self.cut = true;
        }
    }

    pub fn set(&mut self, s: &[u8]) {
        self.clear();
        for &c in s {
            self.push(c);
        }
    }

    #[must_use]
    pub fn hash(&self) -> u64 {
        name_hash(self.as_bytes())
    }

    /// Up to and including the last `/` (the folder of a document).
    #[must_use]
    pub fn folder(&self) -> &[u8] {
        let b = self.as_bytes();
        match b.iter().rposition(|&c| c == b'/') {
            Some(i) => &b[..=i],
            None => &[],
        }
    }

    /// Ends with `suffix` (ASCII letters folded).
    #[must_use]
    pub fn ends_with(&self, suffix: &[u8]) -> bool {
        let b = self.as_bytes();
        b.len() >= suffix.len() && b[b.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    }

    fn pop(&mut self) {
        // "a/b/" → "a/".
        if self.n > 0 && self.b[usize::from(self.n) - 1] == b'/' {
            self.n -= 1;
        }
        self.n = self.as_bytes().iter().rposition(|&c| c == b'/').map_or(0, |i| i as u8 + 1);
    }
}

/// Where an `href` points: a document of the archive and a fragment in
/// it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Target {
    /// [`name_hash`] of the document's path.
    pub path: u64,
    /// The fragment's hash ([`crate::xml::hash32`]), if it has one.
    pub frag: Option<u32>,
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Percent-decoded bytes of `s`, one at a time.
fn unescape(s: &[u8], mut f: impl FnMut(u8)) {
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%'
            && let (Some(h), Some(l)) =
                (s.get(i + 1).copied().and_then(hex), s.get(i + 2).copied().and_then(hex))
        {
            f(h << 4 | l);
            i += 3;
            continue;
        }
        f(s[i]);
        i += 1;
    }
}

/// `href` (from the document at `base`) as a path in the archive, put in
/// `out`: percent-decoded, `.` and `..` resolved, the query dropped.
/// `None` for a link out of the book (a scheme such as `http:`).
pub fn resolve(base: &[u8], href: &[u8], out: &mut Name) -> Option<Target> {
    let (path, frag) = match href.iter().position(|&c| c == b'#') {
        Some(i) => (&href[..i], Some(&href[i + 1..])),
        None => (href, None),
    };
    let path = match path.iter().position(|&c| c == b'?') {
        Some(i) => &path[..i],
        None => path,
    };
    let colon = path.iter().position(|&c| c == b':');
    let slash = path.iter().position(|&c| c == b'/');
    if colon.is_some_and(|c| slash.is_none_or(|s| c < s)) {
        return None;
    }
    out.clear();
    if path.is_empty() {
        out.set(base);
    } else {
        if path.first() != Some(&b'/') {
            for &c in base.iter().take(base.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1)) {
                out.push(c);
            }
        }
        for seg in path.split(|&c| c == b'/') {
            match seg {
                b"" | b"." => {}
                b".." => out.pop(),
                seg => {
                    unescape(seg, |c| out.push(c));
                    out.push(b'/');
                }
            }
        }
        if out.n > 0 && out.b[usize::from(out.n) - 1] == b'/' {
            out.n -= 1;
        }
    }
    let frag = frag.filter(|f| !f.is_empty()).map(|f| {
        let mut h = crate::xml::FNV32;
        unescape(f, |c| h = crate::xml::fnv32(h, c));
        h
    });
    Some(Target { path: out.hash(), frag })
}

/// Reads the central directory as a stream: bytes in any pieces, one
/// entry out when its name is complete (its extra field and comment are
/// skipped after it).
#[derive(Clone, Copy, Debug)]
pub struct CdWalk {
    hdr: [u8; CD_HEADER],
    hdr_n: u8,
    name_left: u16,
    skip: u32,
    hash: u64,
    bias: u32,
    /// Entries read.
    pub seen: u16,
    /// Not a central directory header where one should be: the walk ends.
    pub bad: bool,
}

impl CdWalk {
    #[must_use]
    pub fn new(bias: u32) -> CdWalk {
        CdWalk {
            hdr: [0; CD_HEADER],
            hdr_n: 0,
            name_left: 0,
            skip: 0,
            hash: FNV64,
            bias,
            seen: 0,
            bad: false,
        }
    }

    /// Take bytes of `b`; stops after an entry (returned with the bytes
    /// taken). Its name is kept in `name` if one is lent (the same one for
    /// all of an entry's bytes).
    pub fn feed(&mut self, b: &[u8], mut name: Option<&mut Name>) -> (usize, Option<Entry>) {
        let mut i = 0;
        while i < b.len() && !self.bad {
            if self.name_left == 0 && self.skip > 0 {
                let n = (self.skip as usize).min(b.len() - i);
                self.skip -= n as u32;
                i += n;
                continue;
            }
            if self.name_left > 0 {
                let c = b[i];
                i += 1;
                self.hash = name_byte(self.hash, c);
                if let Some(n) = name.as_deref_mut() {
                    n.push(c);
                }
                self.name_left -= 1;
                if self.name_left == 0 {
                    return (i, Some(self.entry()));
                }
                continue;
            }
            let at = usize::from(self.hdr_n);
            let n = (CD_HEADER - at).min(b.len() - i);
            self.hdr[at..at + n].copy_from_slice(&b[i..i + n]);
            self.hdr_n += n as u8;
            i += n;
            if usize::from(self.hdr_n) < CD_HEADER {
                continue;
            }
            if self.hdr[..4] != CD_SIG {
                self.bad = true;
                return (b.len(), None);
            }
            self.hdr_n = 0;
            self.hash = FNV64;
            if let Some(n) = name.as_deref_mut() {
                n.clear();
            }
            self.name_left = rd16(&self.hdr, 28);
            self.skip = u32::from(rd16(&self.hdr, 30)) + u32::from(rd16(&self.hdr, 32));
            if self.name_left == 0 {
                return (i, Some(self.entry()));
            }
        }
        (if self.bad { b.len() } else { i }, None)
    }

    fn entry(&mut self) -> Entry {
        self.seen = self.seen.saturating_add(1);
        let h = &self.hdr;
        let local = rd32(h, 42);
        Entry {
            hash: self.hash,
            local: if local == u32::MAX { local } else { local.saturating_add(self.bias) },
            csize: rd32(h, 20),
            usize: rd32(h, 24),
            crc: rd32(h, 16),
            method: rd16(h, 10),
            flags: rd16(h, 8),
        }
    }
}

/// Where an entry's data starts, from its local header (`b`: the bytes at
/// `Entry::local`, at least [`LOCAL`]). `None` if it is not one.
#[must_use]
pub fn local_data(b: &[u8]) -> Option<u32> {
    if b.get(..4)? != LOCAL_SIG || b.len() < LOCAL {
        return None;
    }
    Some(LOCAL as u32 + u32::from(rd16(b, 26)) + u32::from(rd16(b, 28)))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    #[test]
    fn crc_check_value() {
        assert_eq!(crc32(0, b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(crc32(0, b"1234"), b"56789"), 0xCBF4_3926);
        assert_eq!(crc32(0, b""), 0);
    }

    fn resolved(base: &str, href: &str) -> Option<(std::string::String, Option<u32>)> {
        let mut n = Name::new();
        let t = resolve(base.as_bytes(), href.as_bytes(), &mut n)?;
        assert_eq!(t.path, name_hash(n.as_bytes()));
        Some((std::string::String::from_utf8(n.as_bytes().to_vec()).unwrap(), t.frag))
    }

    #[test]
    fn paths() {
        let h = |s: &str| Some(crate::xml::hash32(s.as_bytes()));
        assert_eq!(
            resolved("OEBPS/content.opf", "Text/ch1.xhtml"),
            Some(("OEBPS/Text/ch1.xhtml".into(), None))
        );
        assert_eq!(
            resolved("OEBPS/Text/nav.xhtml", "../Text/ch%201.xhtml#s1"),
            Some(("OEBPS/Text/ch 1.xhtml".into(), h("s1")))
        );
        assert_eq!(resolved("OEBPS/Text/nav.xhtml", "#top"), Some(("OEBPS/Text/nav.xhtml".into(), h("top"))));
        assert_eq!(resolved("a/b/c.opf", "../../../x.html?q=1"), Some(("x.html".into(), None)));
        assert_eq!(resolved("a/c.opf", "/root.html"), Some(("root.html".into(), None)));
        assert_eq!(resolved("", "./OPS/./p.opf"), Some(("OPS/p.opf".into(), None)));
        assert_eq!(resolved("a/c.opf", "http://example.com/x"), None);
        assert_eq!(resolved("a/c.opf", "mailto:me@x"), None);
        assert_eq!(resolved("a/c.opf", "dir/p:q.html"), Some(("a/dir/p:q.html".into(), None)));
        assert_eq!(name_hash(b"OEBPS\\Text\\A.XHTML"), name_hash(b"oebps/text/a.xhtml"));
        let long = "x/".repeat(200);
        let mut n = Name::new();
        resolve(b"", long.as_bytes(), &mut n).unwrap();
        assert!(n.cut);
    }

    /// A small archive by hand: two stored entries, a comment.
    fn archive(prefix: &[u8], comment: &[u8]) -> Vec<u8> {
        let mut z = prefix.to_vec();
        let mut cd = Vec::new();
        for (name, data) in [(&b"mimetype"[..], &b"application/epub+zip"[..]), (b"a/b.txt", b"hi")] {
            let local = (z.len() - prefix.len()) as u32;
            let crc = crc32(0, data);
            let mut h = Vec::new();
            h.extend_from_slice(&LOCAL_SIG);
            h.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            h.extend_from_slice(&crc.to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            z.extend_from_slice(&h);
            z.extend_from_slice(name);
            z.extend_from_slice(data);
            cd.extend_from_slice(&CD_SIG);
            cd.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            cd.extend_from_slice(&crc.to_le_bytes());
            cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&3u16.to_le_bytes());
            // Comment length, disk, internal and external attributes.
            cd.extend_from_slice(&[0; 10]);
            cd.extend_from_slice(&local.to_le_bytes());
            cd.extend_from_slice(name);
            cd.extend_from_slice(b"xyz");
        }
        let cd_at = (z.len() - prefix.len()) as u32;
        z.extend_from_slice(&cd);
        z.extend_from_slice(&END_SIG);
        z.extend_from_slice(&[0, 0, 0, 0, 2, 0, 2, 0]);
        z.extend_from_slice(&(cd.len() as u32).to_le_bytes());
        z.extend_from_slice(&cd_at.to_le_bytes());
        z.extend_from_slice(&(comment.len() as u16).to_le_bytes());
        z.extend_from_slice(comment);
        z
    }

    fn walk(z: &[u8], e: End, piece: usize) -> Vec<Entry> {
        let mut w = CdWalk::new(e.bias);
        let cd = &z[e.cd_at as usize..(e.cd_at + e.cd_size) as usize];
        let mut out = Vec::new();
        for p in cd.chunks(piece) {
            let mut at = 0;
            while at < p.len() {
                let (n, ent) = w.feed(&p[at..], None);
                at += n;
                out.extend(ent);
            }
        }
        assert!(!w.bad);
        out
    }

    /// BOOK-2, 2026-11-25: the walk skipped an entry's extra field and
    /// comment before reading its name (they follow it): every entry with
    /// an extra field (most packers write one) hashed the wrong bytes and
    /// was never found.
    #[test]
    fn regress_cd_walk_name_before_extra() {
        let z = archive(b"", b"");
        let e = find_end(&z, 0, z.len() as u32).unwrap().unwrap();
        let es = walk(&z, e, 7);
        assert_eq!(es[0].hash, name_hash(b"mimetype"));
        assert_eq!(es[1].hash, name_hash(b"a/b.txt"));
    }

    #[test]
    fn end_and_directory() {
        for (prefix, comment) in [(&b""[..], &b""[..]), (b"MZ stub", b"a PK\x05\x06 comment")] {
            let z = archive(prefix, comment);
            let e = find_end(&z, 0, z.len() as u32).unwrap().unwrap();
            assert_eq!((e.entries, e.bias), (2, prefix.len() as u32));
            for piece in [1, 5, 512] {
                let es = walk(&z, e, piece);
                assert_eq!(es.len(), 2);
                assert_eq!(es[1].hash, name_hash(b"A/B.TXT"));
                let at = es[1].local as usize;
                let data = at + local_data(&z[at..]).unwrap() as usize;
                assert_eq!(&z[data..data + 2], b"hi");
                assert_eq!(es[1].crc, crc32(0, b"hi"));
                assert!(es[1].readable());
            }
        }
        // Searched in a later piece of the file (the caller steps back).
        let z = archive(b"", &[b'c'; 1500]);
        let mut from = z.len() - 512;
        let mut steps = 0;
        let e = loop {
            if let Some(e) = find_end(&z[from..from + 512], from as u32, z.len() as u32) {
                break e;
            }
            assert!(from > 0);
            from = from.saturating_sub(491);
            steps += 1;
        };
        assert_eq!((e.unwrap().entries, steps), (2, 3));
        // Refusals.
        let mut z64 = archive(b"", b"");
        let n = z64.len();
        z64[n - 12..n - 8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(find_end(&z64, 0, n as u32), Some(Err(ZipError::Zip64)));
        let mut multi = archive(b"", b"");
        multi[n - 18] = 1;
        assert_eq!(find_end(&multi, 0, n as u32), Some(Err(ZipError::MultiDisk)));
        assert_eq!(find_end(b"just some text, not an archive at all", 0, 37), None);
        let mut bad = archive(b"", b"");
        bad[n - 6..n - 2].copy_from_slice(&(n as u32).to_le_bytes());
        assert_eq!(find_end(&bad, 0, n as u32), Some(Err(ZipError::NotZip)));
        let enc = Entry { flags: 1, method: DEFLATED, ..Entry::default() };
        assert!(!enc.readable());
        assert!(!Entry { method: 12, ..Entry::default() }.readable());
        assert!(!Entry { csize: u32::MAX, ..Entry::default() }.readable());
    }
}
