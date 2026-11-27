//! RDS group decoding (IEC 62106 / EN 50067): the programme service name
//! (group 0A/0B, 8 characters), radio text (group 2A: 64 characters,
//! 2B: 32) and clock time (group 4A, UTC to the minute), from the four
//! blocks the tuner reports (registers 0Ch–0Fh).
//!
//! The RDA5807M class reports block errors only for blocks A and B
//! (register 0Bh `BLERA`/`BLERB`): a group is used when B has at most 1–2
//! corrected bits. C and D carry no error report, so the service name is
//! taken character pair by character pair only after the same pair was
//! received twice. Radio text is shown as far as it has arrived
//! contiguously; its A/B flag starting a new text clears it. Consecutive
//! identical groups count once (the status is read twice per group). The
//! clock time, sent once a minute, is taken only when a second group 4A
//! agrees with the first (the same minute, or the next one): a corrupted
//! C or D block cannot set the player's clock.
//!
//! Characters are bytes of the RDS character set: 0x20–0x7E are taken as
//! ASCII; [`to_utf8`] maps the upper half from the basic table of the
//! standard (Annex E), as far as this crate knows it **[inferred]**.

/// Programme service name length.
pub const PS_LEN: usize = 8;
/// Radio text length (group 2A).
pub const RT_LEN: usize = 64;

/// What a group changed.
pub mod changed {
    pub const PS: u8 = 1;
    pub const RT: u8 = 2;
    /// A confirmed clock time ([`Rds::clock`]).
    pub const CT: u8 = 4;
}

/// Modified Julian Day of 1970-01-01.
const MJD_UNIX: u32 = 40_587;
/// Clock times before 2020-01-01 (MJD 58849) are taken as corrupted.
const MJD_MIN: u32 = 58_849;

/// Decoder state (about 170 bytes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rds {
    pi: u16,
    last: [u16; 4],
    ps_cand: [u8; PS_LEN],
    ps: [u8; PS_LEN],
    /// Segments (character pairs) of `ps` confirmed.
    ps_ok: u8,
    rt: [u8; RT_LEN],
    /// Character positions of `rt` received.
    rt_seen: u64,
    /// Text A/B flag (2 = none seen yet).
    rt_ab: u8,
    /// Position of the end-of-text mark (`RT_LEN` = none).
    rt_end: u8,
    /// Groups used, and groups dropped for block-B errors (wrapping:
    /// 16 bits keep the decoder inside the radio's SRAM3 block).
    pub groups: u16,
    pub bad: u16,
    /// The confirmed clock time (minutes since 1970, UTC; 0 = none).
    ct: u32,
    /// The low 16 bits of the last group 4A's minute, not yet confirmed
    /// (0xFFFF none).
    ct_cand: u16,
}

impl Default for Rds {
    fn default() -> Self {
        Rds::new()
    }
}

impl Rds {
    #[must_use]
    pub const fn new() -> Rds {
        Rds {
            pi: 0,
            last: [0; 4],
            ps_cand: [0; PS_LEN],
            ps: [b' '; PS_LEN],
            ps_ok: 0,
            rt: [b' '; RT_LEN],
            rt_seen: 0,
            rt_ab: 2,
            rt_end: RT_LEN as u8,
            groups: 0,
            bad: 0,
            ct: 0,
            ct_cand: 0xFFFF,
        }
    }

    /// A new station: forget everything but the counters.
    pub fn reset(&mut self) {
        let (g, b) = (self.groups, self.bad);
        *self = Rds::new();
        self.groups = g;
        self.bad = b;
    }

    /// The programme identification of the station (0 = none yet).
    #[must_use]
    pub const fn pi(&self) -> u16 {
        self.pi
    }

    fn clear_rt(&mut self) {
        self.rt = [b' '; RT_LEN];
        self.rt_seen = 0;
        self.rt_end = RT_LEN as u8;
    }

    /// Feed one group: blocks A–D and the error levels of A and B.
    /// Returns [`changed`] bits.
    pub fn feed(&mut self, blocks: [u16; 4], bler_a: u8, bler_b: u8) -> u8 {
        if bler_b > 1 {
            self.bad = self.bad.wrapping_add(1);
            return 0;
        }
        if blocks == self.last {
            return 0;
        }
        self.last = blocks;
        self.groups = self.groups.wrapping_add(1);
        let [a, b, c, d] = blocks;
        let mut ch = 0;
        if bler_a <= 1 && a != 0 && a != self.pi {
            if self.pi != 0 {
                // Another station: its text is not this one's.
                let had = self.ps_ok != 0 || self.rt_seen != 0;
                self.reset();
                self.last = blocks;
                if had {
                    ch = changed::PS | changed::RT;
                }
            }
            self.pi = a;
        }
        let version_b = b & 0x0800 != 0;
        match b >> 12 {
            0 => ch |= self.ps_pair(usize::from(b & 3), d.to_be_bytes()),
            2 => {
                let ab = ((b >> 4) & 1) as u8;
                if self.rt_ab != 2 && ab != self.rt_ab && self.rt_seen != 0 {
                    self.clear_rt();
                    ch |= changed::RT;
                }
                self.rt_ab = ab;
                let seg = usize::from(b & 15);
                if version_b {
                    let [d0, d1] = d.to_be_bytes();
                    ch |= self.rt_chars(seg * 2, &[d0, d1]);
                } else {
                    let [c0, c1] = c.to_be_bytes();
                    let [d0, d1] = d.to_be_bytes();
                    ch |= self.rt_chars(seg * 4, &[c0, c1, d0, d1]);
                }
            }
            4 if !version_b => ch |= self.clock_group(b, c, d),
            _ => {}
        }
        ch
    }

    /// Group 4A (IEC 62106 §3.1.5.6): the Modified Julian Day in B bits 1–0
    /// and C bits 15–1, the UTC hour in C bit 0 and D bits 15–12, the
    /// minute in D bits 11–6 (the local offset in D bits 5–0 is not used:
    /// the player has its own time zone setting).
    fn clock_group(&mut self, b: u16, c: u16, d: u16) -> u8 {
        let mjd = u32::from(b & 3) << 15 | u32::from(c >> 1);
        let hour = u32::from(c & 1) << 4 | u32::from(d >> 12);
        let minute = u32::from((d >> 6) & 0x3F);
        if mjd < MJD_MIN || hour > 23 || minute > 59 {
            self.ct_cand = 0xFFFF;
            return 0;
        }
        let t = (mjd - MJD_UNIX) * 1440 + hour * 60 + minute;
        let low = t as u16;
        let agrees = self.ct_cand != 0xFFFF && (low == self.ct_cand || low == self.ct_cand.wrapping_add(1));
        self.ct_cand = low;
        if agrees && t != self.ct {
            self.ct = t;
            return changed::CT;
        }
        0
    }

    /// Whether this station sent a plausible clock-time group (4A) since it
    /// was tuned, confirmed or not (`fm status` diagnostics, 2026-11-27;
    /// the decoder has no room for a counter).
    #[must_use]
    pub const fn clock_heard(&self) -> bool {
        self.ct != 0 || self.ct_cand != 0xFFFF
    }

    /// The last confirmed clock time, seconds since 1970 UTC (to the
    /// minute: the group is sent at the start of the minute it gives).
    #[must_use]
    pub const fn clock(&self) -> Option<u64> {
        if self.ct == 0 { None } else { Some(self.ct as u64 * 60) }
    }

    fn ps_pair(&mut self, seg: usize, pair: [u8; 2]) -> u8 {
        let i = seg * 2;
        let (Some(cand), Some(cur)) = (self.ps_cand.get_mut(i..i + 2), self.ps.get_mut(i..i + 2)) else {
            return 0;
        };
        if *cand != pair {
            cand.copy_from_slice(&pair);
            return 0;
        }
        let was_ok = self.ps_ok & (1 << seg) != 0;
        self.ps_ok |= 1 << seg;
        if *cur != pair || !was_ok {
            cur.copy_from_slice(&pair);
            if self.ps_ok == 0xF {
                return changed::PS;
            }
        }
        0
    }

    fn rt_chars(&mut self, at: usize, chars: &[u8]) -> u8 {
        let mut ch = 0;
        for (k, &c) in chars.iter().enumerate() {
            let pos = at + k;
            let Some(slot) = self.rt.get_mut(pos) else { break };
            if c == 0x0D {
                if usize::from(self.rt_end) != pos {
                    self.rt_end = pos as u8;
                    ch = changed::RT;
                }
                break;
            }
            let new = self.rt_seen & (1 << pos) == 0;
            if *slot != c || new {
                *slot = c;
                self.rt_seen |= 1 << pos;
                ch = changed::RT;
            }
        }
        ch
    }

    /// The service name once all four pairs are confirmed, spaces trimmed.
    #[must_use]
    pub fn ps(&self) -> &[u8] {
        if self.ps_ok != 0xF {
            return &[];
        }
        trim(&self.ps)
    }

    /// The radio text received so far (from the start, contiguous, up to
    /// the end mark), spaces trimmed.
    #[must_use]
    pub fn rt(&self) -> &[u8] {
        let mut n = 0;
        while n < usize::from(self.rt_end) && n < RT_LEN && self.rt_seen & (1 << n) != 0 {
            n += 1;
        }
        trim(self.rt.get(..n).unwrap_or(&[]))
    }
}

fn trim(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|&c| c != b' ').unwrap_or(s.len());
    let end = s.iter().rposition(|&c| c != b' ').map_or(start, |e| e + 1);
    s.get(start..end).unwrap_or(&[])
}

/// The upper half of the RDS basic character table (0x80–0xFF), Annex E
/// **[inferred]**; `'\0'` = not mapped (shown as a space).
const HIGH: [char; 128] = [
    'á', 'à', 'é', 'è', 'í', 'ì', 'ó', 'ò', 'ú', 'ù', 'Ñ', 'Ç', 'Ş', 'ß', '¡', 'Ĳ', // 0x80
    'â', 'ä', 'ê', 'ë', 'î', 'ï', 'ô', 'ö', 'û', 'ü', 'ñ', 'ç', 'ş', 'ğ', 'ı', 'ĳ', // 0x90
    'ª', 'α', '©', '‰', 'Ğ', 'ě', 'ň', 'ő', 'π', '€', '£', '$', '←', '↑', '→', '↓', // 0xA0
    'º', '¹', '²', '³', '±', 'İ', 'ń', 'ű', 'µ', '¿', '÷', '°', '¼', '½', '¾', '§', // 0xB0
    'Á', 'À', 'É', 'È', 'Í', 'Ì', 'Ó', 'Ò', 'Ú', 'Ù', 'Ř', 'Č', 'Š', 'Ž', 'Ð', 'Ŀ', // 0xC0
    'Â', 'Ä', 'Ê', 'Ë', 'Î', 'Ï', 'Ô', 'Ö', 'Û', 'Ü', 'ř', 'č', 'š', 'ž', 'đ', 'ŀ', // 0xD0
    'Ã', 'Å', 'Æ', 'Œ', 'ŷ', 'Ý', 'Õ', 'Ø', 'Þ', 'Ŋ', 'Ŕ', 'Ć', 'Ś', 'Ź', 'Ŧ', 'ð', // 0xE0
    'ã', 'å', 'æ', 'œ', 'ŵ', 'ý', 'õ', 'ø', 'þ', 'ŋ', 'ŕ', 'ć', 'ś', 'ź', 'ŧ', '\0', // 0xF0
];

/// One RDS character as Unicode (control characters become spaces).
#[must_use]
pub fn char_of(b: u8) -> char {
    match b {
        0x20..=0x7E => char::from(b),
        0x80..=0xFF => match HIGH.get(usize::from(b - 0x80)).copied() {
            Some('\0') | None => ' ',
            Some(c) => c,
        },
        _ => ' ',
    }
}

/// RDS text as UTF-8 into `out`; returns the bytes written (whole
/// characters only). Encoded by hand: `char::encode_utf8` has a panic
/// path that brings `core::fmt` into the firmware.
pub fn to_utf8(raw: &[u8], out: &mut [u8]) -> usize {
    let mut n = 0;
    for &b in raw {
        let c = u32::from(char_of(b));
        // The table holds Basic Multilingual Plane characters only.
        let (len, bytes) = if c < 0x80 {
            (1, [c as u8, 0, 0])
        } else if c < 0x800 {
            (2, [0xC0 | (c >> 6) as u8, 0x80 | (c & 0x3F) as u8, 0])
        } else {
            (3, [0xE0 | (c >> 12) as u8, 0x80 | ((c >> 6) & 0x3F) as u8, 0x80 | (c & 0x3F) as u8])
        };
        let Some(dst) = out.get_mut(n..n + len) else { break };
        for (d, s) in dst.iter_mut().zip(bytes) {
            *d = s;
        }
        n += len;
    }
    n
}

/// Encode one group of type 0A (service name pair `seg`) — for fakes and
/// tests.
#[must_use]
pub fn group_0a(pi: u16, seg: u8, ps: &[u8; PS_LEN]) -> [u16; 4] {
    let i = usize::from(seg & 3) * 2;
    let pair = [ps.get(i).copied().unwrap_or(b' '), ps.get(i + 1).copied().unwrap_or(b' ')];
    [pi, u16::from(seg & 3), 0xE0CD, u16::from_be_bytes(pair)]
}

/// Encode one group of type 4A (clock time, `unix` seconds UTC, local
/// offset `half_hours`) — for fakes and tests.
#[must_use]
pub fn group_4a(pi: u16, unix: u64, half_hours: i8) -> [u16; 4] {
    let min = unix / 60;
    let mjd = (min / 1440) as u32 + MJD_UNIX;
    let (hour, minute) = (((min % 1440) / 60) as u16, (min % 60) as u16);
    let off = u16::from(half_hours.unsigned_abs() & 0x1F) | if half_hours < 0 { 0x20 } else { 0 };
    [
        pi,
        0x4000 | ((mjd >> 15) & 3) as u16,
        ((mjd & 0x7FFF) as u16) << 1 | (hour >> 4),
        (hour & 15) << 12 | minute << 6 | off,
    ]
}

/// Encode one group of type 2A (radio text segment `seg`, flag `ab`) from
/// a 64-byte text — for fakes and tests.
#[must_use]
pub fn group_2a(pi: u16, seg: u8, ab: bool, rt: &[u8; RT_LEN]) -> [u16; 4] {
    let i = usize::from(seg & 15) * 4;
    let c = |k: usize| rt.get(i + k).copied().unwrap_or(b' ');
    [
        pi,
        0x2000 | if ab { 0x10 } else { 0 } | u16::from(seg & 15),
        u16::from_be_bytes([c(0), c(1)]),
        u16::from_be_bytes([c(2), c(3)]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ps8(s: &str) -> [u8; PS_LEN] {
        let mut b = [b' '; PS_LEN];
        b[..s.len()].copy_from_slice(s.as_bytes());
        b
    }

    fn rt64(s: &str) -> [u8; RT_LEN] {
        let mut b = [b' '; RT_LEN];
        b[..s.len()].copy_from_slice(s.as_bytes());
        if s.len() < RT_LEN {
            b[s.len()] = 0x0D;
        }
        b
    }

    #[test]
    fn service_name_needs_each_pair_twice() {
        let mut r = Rds::new();
        let ps = ps8("ROCK FM");
        for seg in 0..4 {
            assert_eq!(r.feed(group_0a(0xD123, seg, &ps), 0, 0), 0);
        }
        assert_eq!(r.ps(), b"", "seen once: not yet");
        let mut ch = 0;
        for seg in 0..4 {
            ch |= r.feed(group_0a(0xD123, seg, &ps), 0, 0);
        }
        assert_eq!(ch, changed::PS);
        assert_eq!(r.ps(), b"ROCK FM");
        assert_eq!(r.pi(), 0xD123);
    }

    #[test]
    fn corrupt_pairs_and_block_b_errors_are_not_shown() {
        let mut r = Rds::new();
        let ps = ps8("KLASSIK");
        let bad = ps8("KLAXXIK");
        // B with errors: dropped.
        assert_eq!(r.feed(group_0a(1, 0, &ps), 0, 2), 0);
        assert_eq!(r.bad, 1);
        for seg in 0..4 {
            r.feed(group_0a(1, seg, &ps), 0, 0);
            // A corrupt copy in between resets the candidate.
            r.feed(group_0a(1, seg, &bad), 0, 1);
            r.feed(group_0a(1, seg, &ps), 0, 0);
        }
        assert_eq!(r.ps(), b"", "no pair arrived twice in a row");
        for _ in 0..2 {
            for seg in 0..4 {
                r.feed(group_0a(1, seg, &ps), 0, 0);
                // Another group type between repeats (ignored).
                r.feed([0, 0x1000 | u16::from(seg), 0, 0], 3, 1);
            }
        }
        assert_eq!(r.ps(), b"KLASSIK");
    }

    #[test]
    fn radio_text_contiguous_and_ab_flag_clears() {
        let mut r = Rds::new();
        let t = rt64("Now: Queen - Don't Stop Me Now");
        r.feed(group_2a(7, 0, false, &t), 0, 0);
        r.feed(group_2a(7, 2, false, &t), 0, 0);
        assert_eq!(r.rt(), b"Now:", "segment 1 missing: only the first");
        r.feed(group_2a(7, 1, false, &t), 0, 0);
        assert_eq!(r.rt(), b"Now: Queen -");
        for seg in 3..9 {
            r.feed(group_2a(7, seg, false, &t), 0, 0);
        }
        assert_eq!(r.rt(), b"Now: Queen - Don't Stop Me Now");
        // A new text (flag B).
        let t2 = rt64("News");
        let ch = r.feed(group_2a(7, 0, true, &t2), 0, 0);
        assert_eq!(ch, changed::RT);
        assert_eq!(r.rt(), b"News");
    }

    #[test]
    fn a_new_station_clears_and_duplicates_count_once() {
        let mut r = Rds::new();
        let ps = ps8("ONE");
        for _ in 0..2 {
            for seg in 0..4 {
                r.feed(group_0a(0x1111, seg, &ps), 0, 0);
            }
        }
        assert_eq!(r.ps(), b"ONE");
        let g = r.groups;
        let x = group_0a(0x1111, 0, &ps);
        r.feed(x, 0, 0);
        r.feed(x, 0, 0);
        assert_eq!(r.groups, g + 1);
        let ch = r.feed(group_0a(0x2222, 0, &ps8("TWO")), 0, 0);
        assert_eq!(ch, changed::PS | changed::RT);
        assert_eq!(r.ps(), b"");
        assert_eq!(r.pi(), 0x2222);
    }

    #[test]
    fn text_of_version_b_groups() {
        let mut r = Rds::new();
        // 2B: two characters in block D.
        r.feed([5, 0x2800, 0, u16::from_be_bytes(*b"Hi")], 0, 0);
        r.feed([5, 0x2801, 0, u16::from_be_bytes([b'!', 0x0D])], 0, 0);
        assert_eq!(r.rt(), b"Hi!");
    }

    #[test]
    fn utf8_matches_the_standard_encoder() {
        for b in 0..=255u8 {
            let mut ours = [0u8; 4];
            let n = to_utf8(&[b], &mut ours);
            let mut std_ = [0u8; 4];
            let s = char_of(b).encode_utf8(&mut std_);
            assert_eq!(&ours[..n], s.as_bytes(), "byte {b:#04x}");
        }
    }

    #[test]
    fn utf8_mapping() {
        let mut out = [0u8; 32];
        let n = to_utf8(&[b'C', b'a', b'f', 0x82, 0x01, 0x91], &mut out);
        assert_eq!(core::str::from_utf8(&out[..n]).unwrap(), "Café ä");
        // Never cuts a character.
        let mut small = [0u8; 4];
        let n = to_utf8(&[b'a', b'b', b'c', 0x82], &mut small);
        assert_eq!(&small[..n], b"abc");
        assert_eq!(char_of(0xFF), ' ');
    }

    /// Diagnostics (2026-11-27): `fm status` shows whether this station
    /// sends clock time at all: one plausible group is heard before it is
    /// confirmed, and a new station starts unheard.
    #[test]
    fn clock_heard_per_station() {
        let t = 1_795_595_400u64;
        let mut r = Rds::new();
        assert!(!r.clock_heard());
        r.feed(group_4a(0xD123, t, 2), 0, 0);
        assert!(r.clock_heard() && r.clock().is_none());
        r.feed(group_4a(0xD123, t + 60, 2), 0, 0);
        assert!(r.clock_heard() && r.clock().is_some());
        r.reset();
        assert!(!r.clock_heard(), "a new station starts unheard");
    }

    /// Group 4A (VER-13, Settings › Date & time › Set from RDS): the clock
    /// time is taken when the next minute's group agrees, at the minute;
    /// a corrupted or implausible group, or one that disagrees, is not.
    #[test]
    fn clock_time_needs_two_agreeing_minutes() {
        // 2026-11-25 08:30 UTC.
        let t = 1_795_595_400u64;
        let g = group_4a(0xD123, t, 2);
        // MJD 61369 = 2026-11-25; hour 8; minute 30; +1 h.
        assert_eq!(g[1] & 0xF800, 0x4000);
        assert_eq!(u32::from(g[1] & 3) << 15 | u32::from(g[2] >> 1), 61_369);
        assert_eq!((g[3] >> 12, (g[3] >> 6) & 0x3F, g[3] & 0x3F), (8, 30, 2));
        let mut r = Rds::new();
        assert_eq!(r.feed(g, 0, 0), 0, "one group is not enough");
        assert_eq!(r.clock(), None);
        // Another group in between (as on air), then the next minute.
        r.feed(group_0a(0xD123, 0, &ps8("ROCK FM")), 0, 0);
        assert_eq!(r.feed(group_4a(0xD123, t + 60, 2), 0, 0), changed::CT);
        assert_eq!(r.clock(), Some(t + 60));
        assert_eq!(r.feed(group_4a(0xD123, t + 120, 2), 0, 0), changed::CT);
        assert_eq!(r.clock(), Some(t + 120));
        // A group that jumps (a corrupted C or D block) is not taken, nor
        // is the one after it unless it follows it.
        let mut r = Rds::new();
        r.feed(group_4a(1, t, 0), 0, 0);
        assert_eq!(r.feed(group_4a(1, t + 3 * 3600, 0), 0, 0), 0);
        assert_eq!(r.clock(), None);
        // Implausible: before 2020, hour 24, minute 60.
        let mut r = Rds::new();
        let old = group_4a(1, 1_500_000_000, 0);
        r.feed(old, 0, 0);
        r.feed(group_0a(1, 0, &ps8("X")), 0, 0);
        assert_eq!(r.feed(old, 0, 0), 0);
        let mut bad = g;
        bad[3] = (bad[3] & 0x0FFF) | 0x8000;
        bad[2] |= 1;
        r.feed(bad, 0, 0);
        bad[3] ^= 0x0040;
        assert_eq!(r.feed(bad, 0, 0), 0, "hour 24");
        assert_eq!(r.clock(), None);
        // The decoder lives in the radio's SRAM3 block (fw fm.rs), whose
        // window is full: the clock time took no room.
        assert_eq!(core::mem::size_of::<Rds>(), 112);
        // Block B in error: the group is not used at all.
        let mut r = Rds::new();
        r.feed(group_4a(1, t, 0), 0, 0);
        assert_eq!(r.feed(group_4a(1, t + 60, 0), 0, 2), 0);
        assert_eq!(r.clock(), None);
    }
}
