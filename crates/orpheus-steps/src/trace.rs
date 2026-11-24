//! Accelerometer trace files (`acc trace <secs>`, SENS-1):
//! `/.orpheus/test/acc-<n>.csv`, one file per capture.
//!
//! ```text
//! # orpheus acc trace 3 rate=25 odr=0x06 secs=60
//! ms,x,y,z
//! 0,-12,8,1031
//! 40,-10,9,1029
//! …
//! # lost 25
//! # end
//! ```
//!
//! `ms` counts from the first sample; `x,y,z` are raw counts (1024 per g).
//! `# lost N` says N samples were dropped (the card was busy); `# truth=N`
//! (added by hand) is the number of steps really taken, for
//! `tests/traces.rs`. Lines starting with `#` are comments for every other
//! purpose.

/// A byte string from a paged `static`: a plain literal may be merged
/// into a resident constant section of the firmware.
macro_rules! t {
    ($s:literal) => {{
        static S: [u8; $s.len()] = *$s;
        let s: &'static [u8] = &S;
        s
    }};
}

/// The folder (shared with the battery logger).
pub const DIR: &str = "/.orpheus/test";
/// Column line.
pub const COLUMNS: &str = "ms,x,y,z";
/// Longest formatted sample line (`4294967295,-32768,-32768,-32768\n`).
pub const LINE_MAX: usize = 32;

/// One sample: time since the start of the capture and raw counts.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Sample {
    pub ms: u32,
    pub x: i16,
    pub y: i16,
    pub z: i16,
}

/// Formats into a byte buffer without `core::fmt`; output beyond the end
/// of the buffer is dropped.
pub struct Writer<'a> {
    buf: &'a mut [u8],
    n: usize,
}

impl<'a> Writer<'a> {
    pub fn new(buf: &'a mut [u8]) -> Writer<'a> {
        Writer { buf, n: 0 }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.n
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn s(&mut self, s: &[u8]) -> &mut Self {
        for &b in s {
            self.b(b);
        }
        self
    }

    pub fn b(&mut self, b: u8) -> &mut Self {
        if let Some(o) = self.buf.get_mut(self.n) {
            *o = b;
            self.n += 1;
        }
        self
    }

    pub fn u(&mut self, v: u32) -> &mut Self {
        let mut d = [0u8; 10];
        let mut n = 0;
        let mut v = v;
        loop {
            if let Some(slot) = d.get_mut(n) {
                *slot = b'0' + (v % 10) as u8;
            }
            n += 1;
            v /= 10;
            if v == 0 || n == d.len() {
                break;
            }
        }
        while n > 0 {
            n -= 1;
            let c = d.get(n).copied().unwrap_or(b'0');
            self.b(c);
        }
        self
    }

    pub fn i(&mut self, v: i32) -> &mut Self {
        if v < 0 {
            self.b(b'-');
        }
        self.u(v.unsigned_abs())
    }
}

/// The two header lines: `# orpheus acc trace <file> rate=<Hz> odr=0x<code> secs=<n>`, columns.
pub fn format_header(file: u16, rate: u16, odr: u8, secs: u32, out: &mut [u8]) -> usize {
    // Hex digits by arithmetic (a 16-byte table would be a resident
    // constant in the firmware).
    let hex = |d: u8| if d < 10 { b'0' + d } else { b'a' + d - 10 };
    let mut w = Writer::new(out);
    w.s(t!(b"# orpheus acc trace "))
        .u(u32::from(file))
        .s(t!(b" rate="))
        .u(u32::from(rate))
        .s(t!(b" odr=0x"))
        .b(hex(odr >> 4))
        .b(hex(odr & 15))
        .s(t!(b" secs="))
        .u(secs)
        .b(b'\n')
        .s(COLUMNS.as_bytes())
        .b(b'\n');
    w.len()
}

/// One sample line.
pub fn format_sample(s: &Sample, out: &mut [u8]) -> usize {
    let mut w = Writer::new(out);
    w.u(s.ms).b(b',').i(i32::from(s.x)).b(b',').i(i32::from(s.y)).b(b',').i(i32::from(s.z)).b(b'\n');
    w.len()
}

/// `# lost N`.
pub fn format_lost(n: u32, out: &mut [u8]) -> usize {
    let mut w = Writer::new(out);
    w.s(t!(b"# lost ")).u(n).b(b'\n');
    w.len()
}

/// `# end`.
pub fn format_end(out: &mut [u8]) -> usize {
    let mut w = Writer::new(out);
    w.s(t!(b"# end\n"));
    w.len()
}

/// `acc-<n>.csv` → `n`.
#[must_use]
pub fn file_number(name: &str) -> Option<u16> {
    let b = name.as_bytes();
    let rest = match b {
        [a, c1, c2, b'-', rest @ ..]
            if a.eq_ignore_ascii_case(&b'a')
                && c1.eq_ignore_ascii_case(&b'c')
                && c2.eq_ignore_ascii_case(&b'c') =>
        {
            rest
        }
        _ => return None,
    };
    let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || digits > 5 || !matches!(rest.get(digits), Some(b'.')) {
        return None;
    }
    let mut v = 0u32;
    for d in rest.iter().take(digits) {
        v = v * 10 + u32::from(d - b'0');
    }
    u16::try_from(v).ok()
}

/// `/.orpheus/test/acc-<n>.csv` into `out`; returns its length.
pub fn file_path(n: u16, out: &mut [u8]) -> usize {
    let mut w = Writer::new(out);
    w.s(DIR.as_bytes()).s(t!(b"/acc-")).u(u32::from(n)).b(b'.').s(t!(b"csv"));
    w.len()
}

/// A sample line (`None` for comments, the column line and anything
/// malformed).
#[must_use]
pub fn parse_line(line: &str) -> Option<Sample> {
    let line = line.trim();
    if line.starts_with('#') {
        return None;
    }
    let mut it = line.split(',');
    let ms = it.next()?.trim().parse().ok()?;
    let x = it.next()?.trim().parse().ok()?;
    let y = it.next()?.trim().parse().ok()?;
    let z = it.next()?.trim().parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some(Sample { ms, x, y, z })
}

/// The value of a `# key=value` header field anywhere in the comment
/// lines (`truth`, `rate`, `tolerance`).
#[must_use]
pub fn header_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().filter(|l| l.starts_with('#')).find_map(|l| {
        l.split_whitespace().find_map(|w| w.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::String;

    #[test]
    fn format_and_parse() {
        let mut b = [0u8; 128];
        let n = format_header(3, 25, 6, 60, &mut b);
        let h = core::str::from_utf8(&b[..n]).unwrap();
        assert_eq!(h, "# orpheus acc trace 3 rate=25 odr=0x06 secs=60\nms,x,y,z\n");
        assert_eq!(header_value(h, "rate"), Some("25"));
        assert_eq!(header_value(h, "truth"), None);
        let s = Sample { ms: 40, x: -12, y: 8, z: 1031 };
        let n = format_sample(&s, &mut b);
        assert_eq!(&b[..n], b"40,-12,8,1031\n");
        assert_eq!(parse_line("40,-12,8,1031"), Some(s));
        let worst = Sample { ms: u32::MAX, x: i16::MIN, y: i16::MIN, z: i16::MIN };
        assert_eq!(format_sample(&worst, &mut b), LINE_MAX);
        assert_eq!(parse_line("ms,x,y,z"), None);
        assert_eq!(parse_line("# lost 3"), None);
        assert_eq!(parse_line("1,2,3"), None);
        assert_eq!(parse_line("1,2,3,4,5"), None);
        let n = format_lost(25, &mut b);
        assert_eq!(&b[..n], b"# lost 25\n");
        let n = format_end(&mut b);
        assert_eq!(&b[..n], b"# end\n");
        // A short buffer is cut, never overrun.
        let mut small = [0u8; 4];
        assert_eq!(format_sample(&s, &mut small), 4);
    }

    #[test]
    fn file_names() {
        let mut b = [0u8; 64];
        let n = file_path(12, &mut b);
        assert_eq!(core::str::from_utf8(&b[..n]).unwrap(), "/.orpheus/test/acc-12.csv");
        assert_eq!(file_number("acc-12.csv"), Some(12));
        assert_eq!(file_number("ACC-7.CSV"), Some(7));
        assert_eq!(file_number("acc-.csv"), None);
        assert_eq!(file_number("drain-3-normal.csv"), None);
        assert_eq!(file_number("acc-123456.csv"), None);
        let mut s = String::new();
        s.push_str("# truth=200 tolerance=5\n");
        assert_eq!(header_value(&s, "truth"), Some("200"));
    }
}
