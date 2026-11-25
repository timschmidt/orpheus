//! Battery-drain log (PWR-4 / PWR-5 measurement tool): one-second samples
//! summed into one CSV row per minute, the rows kept in a small RAM log
//! until the card task appends them to `/.orpheus/test/drain-<n>-<label>.csv`.
//!
//! Everything here is pure (no hardware, no `core::fmt`); the firmware
//! feeds [`Sample`]s, keeps a [`Log`] behind a critical section and writes
//! what [`Log::header`] and [`format_row`] produce. `cargo xtask drain
//! report` reads the files back ([`COLUMNS`] is the contract).
//!
//! File format (UTF-8, `\n` line ends):
//!
//! ```text
//! # orpheus drain log v1 label=normal file=3 perf=normal stay=0 row_s=60
//! uptime_s,mv,mv_raw,percent,charge,cable,clock_mhz,s_low,s_base,s_mid,s_high,s_turbo,s_max,playing_s,backlight,screen,bt,card_io
//! 60,3912,3906,63,none,0,48,0,52,8,0,0,0,60,0,0,off,41
//! # lost 2 rows
//! ```
//!
//! * `mv`: the smoothed battery voltage the gauge uses; `mv_raw`: the last
//!   single reading. `percent`, `charge`, `cable`: as the UI shows them.
//! * `clock_mhz`: the level at the row's end; `s_low` … `s_max`: seconds
//!   spent at each level since the last row (sampled once a second, so
//!   they add up to the row's length).
//! * `playing_s`: seconds of the row with audio playing.
//! * `backlight` (0–16), `screen` (0/1), `bt` (the Bluetooth app's state
//!   word): at the row's end.
//! * `card_io`: card read/write operations since the last row.
//! * Lines starting with `#` are comments: the header, rows lost while the
//!   card was away (`# lost N rows`), and `# stop` / `# power-off` at the end.
//!
//! No panic sites: in the firmware every one keeps a file-path string in a
//! resident section (`.rodata.str1.1`), so indexing goes through `get`.

use crate::charger::ChargeState;
use crate::governor::Level;

/// A byte string in a named `static`. In the firmware a literal used in
/// code may land in a mergeable section (`.rodata.str*`, `.rodata.cst*`),
/// which stays resident; a named static is paged with its users.
macro_rules! bs {
    ($s:expr) => {{
        static S: [u8; $s.len()] = copy($s);
        let s: &'static [u8] = &S;
        s
    }};
}

const fn copy<const N: usize>(s: &[u8]) -> [u8; N] {
    let mut a = [0u8; N];
    let mut i = 0;
    while i < N {
        a[i] = s[i];
        i += 1;
    }
    a
}

/// The column line (the second line of every file).
pub const COLUMNS: &str = "uptime_s,mv,mv_raw,percent,charge,cable,clock_mhz,s_low,s_base,s_mid,s_high,s_turbo,s_max,playing_s,backlight,screen,bt,card_io";
/// Seconds per row.
pub const ROW_SECONDS: u8 = 60;
/// Rows kept in RAM ([`Log`]); the card task is asked to write at
/// [`FLUSH_ROWS`], so one more minute may pass before rows are lost.
pub const LOG_ROWS: usize = 6;
/// Rows that make a write due.
pub const FLUSH_ROWS: usize = 5;
/// Longest scenario label (bytes, `[a-z0-9_-]`).
pub const LABEL_MAX: usize = 16;
/// The folder the files go to.
pub const DIR: &str = "/.orpheus/test";
/// Longest formatted row (bytes, with the line end).
pub const ROW_MAX: usize = 128;
/// Longest header (two lines).
pub const HEADER_MAX: usize = 96 + COLUMNS.len();

/// [`DIR`] (from a paged static).
#[must_use]
pub fn dir() -> &'static str {
    core::str::from_utf8(bs!(DIR.as_bytes())).unwrap_or_default()
}

/// The Bluetooth states the log knows (`bt` column), in the order of
/// `orpheus_apps::services::BtPhase`.
pub const BT_WORDS: [&str; 8] =
    ["off", "starting", "ready", "scanning", "connecting", "connected", "streaming", "notresponding"];

/// One second's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub uptime_s: u32,
    pub mv: u16,
    pub mv_raw: u16,
    pub percent: u8,
    pub charge: ChargeState,
    pub cable: bool,
    pub level: Level,
    pub playing: bool,
    pub backlight: u8,
    pub screen: bool,
    /// The Bluetooth state ([`BT_WORDS`] index).
    pub bt: u8,
    /// Card read/write operations since boot (wrapping).
    pub card_io: u32,
}

/// One row: a minute (or less, the last one) of samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub uptime_s: u32,
    pub mv: u16,
    pub mv_raw: u16,
    pub percent: u8,
    pub charge: ChargeState,
    pub cable: bool,
    pub level: Level,
    /// Seconds at each level ([`Level::ALL`] order).
    pub secs: [u8; 6],
    pub playing_s: u8,
    pub backlight: u8,
    pub screen: bool,
    /// [`BT_WORDS`] index.
    pub bt: u8,
    pub card_io: u32,
}

impl Row {
    /// All zero (so a [`Log`] in a `static` costs no initialised data).
    pub const EMPTY: Row = Row {
        uptime_s: 0,
        mv: 0,
        mv_raw: 0,
        percent: 0,
        charge: ChargeState::NotPresent,
        cable: false,
        level: Level::Low,
        secs: [0; 6],
        playing_s: 0,
        backlight: 0,
        screen: false,
        bt: 0,
        card_io: 0,
    };
}

/// Sums samples into rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sampler {
    /// The row being made (the latest sample's values, the sums).
    cur: Row,
    n: u8,
    /// Card I/O count at the row's start (valid once `io_set`).
    io0: u32,
    io_set: bool,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    #[must_use]
    pub const fn new() -> Sampler {
        Sampler { cur: Row::EMPTY, n: 0, io0: 0, io_set: false }
    }

    /// Take a second's sample; a row every [`ROW_SECONDS`] samples.
    pub fn feed(&mut self, s: Sample) -> Option<Row> {
        if !self.io_set {
            self.io0 = s.card_io;
            self.io_set = true;
        }
        let c = &mut self.cur;
        if let Some(x) = c.secs.get_mut(s.level as usize) {
            *x = x.saturating_add(1);
        }
        c.playing_s = c.playing_s.saturating_add(u8::from(s.playing));
        c.uptime_s = s.uptime_s;
        c.mv = s.mv;
        c.mv_raw = s.mv_raw;
        c.percent = s.percent;
        c.charge = s.charge;
        c.cable = s.cable;
        c.level = s.level;
        c.backlight = s.backlight;
        c.screen = s.screen;
        c.bt = s.bt;
        c.card_io = s.card_io.wrapping_sub(self.io0);
        self.n = self.n.saturating_add(1);
        if self.n >= ROW_SECONDS { self.cut() } else { None }
    }

    /// The row of the samples so far (none since the last row: `None`).
    pub fn cut(&mut self) -> Option<Row> {
        if self.n == 0 {
            return None;
        }
        let r = self.cur;
        *self = Sampler { io0: self.io0.wrapping_add(r.card_io), io_set: true, ..Sampler::new() };
        Some(r)
    }
}

/// How a log ends (the comment line written after its last row).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Stop,
    PowerOff,
}

/// The logger's state: whether it runs, its label and file, the rows not
/// yet on the card. [`Log::new`] is all zero.
#[derive(Clone, Copy, Debug)]
pub struct Log {
    on: bool,
    stay: bool,
    perf_max: bool,
    label: [u8; LABEL_MAX],
    label_len: u8,
    /// The file's number (0: not chosen yet).
    pub file: u16,
    /// The header is on the card.
    pub header_done: bool,
    sampler: Sampler,
    rows: [Row; LOG_ROWS],
    /// Oldest row's slot and the number of rows held.
    head: u8,
    n: u8,
    /// Rows written to the card, and rows dropped (the log was full while
    /// the card was away) since the last write.
    pub written: u32,
    pub lost: u32,
    /// 0 running, 1 ending with `# stop`, 2 with `# power-off` (after the
    /// next write it is off).
    ending: u8,
}

impl Default for Log {
    fn default() -> Self {
        Self::new()
    }
}

impl Log {
    #[must_use]
    pub const fn new() -> Log {
        Log {
            on: false,
            stay: false,
            perf_max: false,
            label: [0; LABEL_MAX],
            label_len: 0,
            file: 0,
            header_done: false,
            sampler: Sampler::new(),
            rows: [Row::EMPTY; LOG_ROWS],
            head: 0,
            n: 0,
            written: 0,
            lost: 0,
            ending: 0,
        }
    }

    /// Start a new log (a running one is replaced: its unwritten rows go).
    /// `label` is cleaned up with [`clean_label`]; empty means `run`.
    pub fn start(&mut self, label: &[u8], stay: bool, perf_max: bool) {
        self.reset();
        let n = clean_label(label, &mut self.label);
        self.label_len = if n == 0 {
            let run = bs!(b"run");
            for (o, c) in self.label.iter_mut().zip(run) {
                *o = *c;
            }
            run.len() as u8
        } else {
            n as u8
        };
        self.on = true;
        self.stay = stay;
        self.perf_max = perf_max;
    }

    /// Back to [`Log::new`] in place (no 250-byte temporary on the stack).
    fn reset(&mut self) {
        self.on = false;
        self.stay = false;
        self.perf_max = false;
        self.label = [0; LABEL_MAX];
        self.label_len = 0;
        self.file = 0;
        self.header_done = false;
        self.sampler = Sampler::new();
        self.head = 0;
        self.n = 0;
        self.written = 0;
        self.lost = 0;
        self.ending = 0;
    }

    /// Running (or ending: rows still to write).
    #[must_use]
    pub fn on(&self) -> bool {
        self.on
    }

    /// How it ends, once [`Log::end`] was called.
    #[must_use]
    pub fn ending(&self) -> Option<End> {
        match self.ending {
            0 => None,
            1 => Some(End::Stop),
            _ => Some(End::PowerOff),
        }
    }

    /// Auto-off is suppressed while this log runs (`drain start … stay`).
    #[must_use]
    pub fn stay(&self) -> bool {
        self.sampling() && self.stay
    }

    pub fn set_stay(&mut self, stay: bool) {
        self.stay = stay;
    }

    /// The `stay` choice itself (also while ending or off).
    #[must_use]
    pub fn stay_choice(&self) -> bool {
        self.stay
    }

    /// Running and not ending: samples are taken.
    #[must_use]
    pub fn sampling(&self) -> bool {
        self.on && self.ending == 0
    }

    #[must_use]
    pub fn label(&self) -> &[u8] {
        self.label.get(..usize::from(self.label_len)).unwrap_or(&[])
    }

    /// Rows waiting for the card.
    #[must_use]
    pub fn pending(&self) -> usize {
        usize::from(self.n)
    }

    /// Something for the card task: the header, the first row (so a new
    /// log shows data after a minute), then every [`FLUSH_ROWS`] rows, or
    /// the end.
    #[must_use]
    pub fn due(&self) -> bool {
        self.on
            && (!self.header_done
                || usize::from(self.n) >= FLUSH_ROWS
                || self.written == 0 && self.n > 0
                || self.ending != 0)
    }

    /// A second's sample (ignored when off or ending). True if a write is
    /// now due.
    pub fn second(&mut self, s: Sample) -> bool {
        if !self.sampling() {
            return false;
        }
        if let Some(r) = self.sampler.feed(s) {
            self.push(r);
        }
        self.due()
    }

    /// End the log: the partial row, then `end` after the last row.
    pub fn end(&mut self, end: End) {
        if !self.sampling() {
            return;
        }
        if let Some(r) = self.sampler.cut() {
            self.push(r);
        }
        self.ending = match end {
            End::Stop => 1,
            End::PowerOff => 2,
        };
    }

    fn push(&mut self, r: Row) {
        if usize::from(self.n) == LOG_ROWS {
            // Full (the card is away): the oldest row goes.
            self.pop();
            self.lost += 1;
        }
        let slot = (usize::from(self.head) + usize::from(self.n)) % LOG_ROWS;
        if let Some(x) = self.rows.get_mut(slot) {
            *x = r;
            self.n += 1;
        }
    }

    /// The oldest row not yet written.
    pub fn pop(&mut self) -> Option<Row> {
        if self.n == 0 {
            return None;
        }
        let r = self.rows.get(usize::from(self.head)).copied();
        self.head = ((usize::from(self.head) + 1) % LOG_ROWS) as u8;
        self.n -= 1;
        r
    }

    /// After the card task has written everything popped: if the log was
    /// ending and nothing is left, it is off.
    pub fn written_out(&mut self, rows: u32) {
        self.written += rows;
        if self.ending != 0 && self.n == 0 {
            self.on = false;
        }
    }

    /// The header (two lines) into `out`; its length.
    pub fn header(&self, out: &mut [u8]) -> usize {
        let mut w = Writer::new(out);
        w.s(bs!(b"# orpheus drain log v1 label="))
            .s(self.label())
            .s(bs!(b" file="))
            .u(u32::from(self.file))
            .s(bs!(b" perf="))
            .s(if self.perf_max { bs!(b"maximum") } else { bs!(b"normal") })
            .s(bs!(b" stay="))
            .u(u32::from(self.stay))
            .s(bs!(b" row_s="))
            .u(u32::from(ROW_SECONDS))
            .b(b'\n')
            .s(bs!(COLUMNS.as_bytes()))
            .b(b'\n');
        w.len()
    }

    /// The file's name (without the folder) into `out`; its length.
    pub fn file_name(&self, out: &mut [u8]) -> usize {
        file_name(self.file, self.label(), out)
    }
}

/// `drain-<n>-<label>.csv` into `out`; its length.
pub fn file_name(n: u16, label: &[u8], out: &mut [u8]) -> usize {
    let mut w = Writer::new(out);
    // `.` and `csv` apart: a 4-byte constant would be merged into a
    // resident `.rodata.cst4` in the firmware.
    w.s(bs!(b"drain-")).u(u32::from(n)).b(b'-').s(label).b(b'.').s(bs!(b"csv"));
    w.len()
}

/// The number of a file named `drain-<n>…` (any case), else `None`.
#[must_use]
pub fn file_number(name: &str) -> Option<u16> {
    let b = name.as_bytes();
    let rest = b.get(..6).filter(|p| p.eq_ignore_ascii_case(bs!(b"drain-"))).and_then(|_| b.get(6..))?;
    let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || digits > 5 {
        return None;
    }
    if !matches!(rest.get(digits), Some(b'-' | b'.') | None) {
        return None;
    }
    let mut v = 0u32;
    for d in rest.iter().take(digits) {
        v = v * 10 + u32::from(d - b'0');
    }
    u16::try_from(v).ok()
}

/// A label for file names: ASCII letters (lower-cased), digits, `_` and
/// `-`; other bytes become `_`; at most [`LABEL_MAX`] bytes; leading and
/// trailing blanks dropped. Returns the length written to `out`.
pub fn clean_label(input: &[u8], out: &mut [u8; LABEL_MAX]) -> usize {
    let t = trim(input);
    let mut n = 0;
    for (o, &c) in out.iter_mut().zip(t) {
        *o = match c {
            b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' => c,
            b'A'..=b'Z' => c.to_ascii_lowercase(),
            _ => b'_',
        };
        n += 1;
    }
    n
}

fn trim(b: &[u8]) -> &[u8] {
    let s = b.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(b.len());
    let e = b.iter().rposition(|c| !c.is_ascii_whitespace()).map_or(s, |i| i + 1);
    b.get(s..e.max(s)).unwrap_or(&[])
}

/// The charge state as a word.
#[must_use]
pub fn charge_word(c: ChargeState) -> &'static [u8] {
    match c {
        ChargeState::NotPresent => bs!(b"none"),
        ChargeState::Detecting => bs!(b"detecting"),
        ChargeState::NoBattery => bs!(b"nobattery"),
        ChargeState::Trickle => bs!(b"trickle"),
        ChargeState::Charging => bs!(b"charging"),
        ChargeState::Full => bs!(b"full"),
        ChargeState::Fault => bs!(b"fault"),
    }
}

/// The Bluetooth word of a [`BT_WORDS`] index.
#[must_use]
pub fn bt_word(i: u8) -> &'static [u8] {
    match i {
        0 => bs!(BT_WORDS[0].as_bytes()),
        1 => bs!(BT_WORDS[1].as_bytes()),
        2 => bs!(BT_WORDS[2].as_bytes()),
        3 => bs!(BT_WORDS[3].as_bytes()),
        4 => bs!(BT_WORDS[4].as_bytes()),
        5 => bs!(BT_WORDS[5].as_bytes()),
        6 => bs!(BT_WORDS[6].as_bytes()),
        _ => bs!(BT_WORDS[7].as_bytes()),
    }
}

/// A row as a CSV line (with `\n`) into `out`; its length. `out` should
/// hold [`ROW_MAX`] bytes (less is cut).
pub fn format_row(r: &Row, out: &mut [u8]) -> usize {
    let mut w = Writer::new(out);
    w.u(r.uptime_s)
        .c()
        .u(u32::from(r.mv))
        .c()
        .u(u32::from(r.mv_raw))
        .c()
        .u(u32::from(r.percent))
        .c()
        .s(charge_word(r.charge))
        .c()
        .u(u32::from(r.cable))
        .c()
        .u(u32::from(r.level.mhz()));
    for s in r.secs {
        w.c().u(u32::from(s));
    }
    w.c()
        .u(u32::from(r.playing_s))
        .c()
        .u(u32::from(r.backlight))
        .c()
        .u(u32::from(r.screen))
        .c()
        .s(bt_word(r.bt))
        .c()
        .u(r.card_io)
        .b(b'\n');
    w.len()
}

/// `# lost N rows\n`.
pub fn format_lost(n: u32, out: &mut [u8]) -> usize {
    let mut w = Writer::new(out);
    w.s(bs!(b"# lost ")).u(n).s(bs!(b" rows\n"));
    w.len()
}

/// `# stop\n` or `# power-off\n`.
pub fn format_end(e: End, out: &mut [u8]) -> usize {
    let mut w = Writer::new(out);
    w.s(match e {
        End::Stop => bs!(b"# stop\n"),
        End::PowerOff => bs!(b"# power-off\n"),
    });
    w.len()
}

/// Bytes into a buffer; what does not fit is dropped.
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

    pub fn u(&mut self, mut v: u32) -> &mut Self {
        // Digits backwards into a small array, then out in order.
        let mut d = [0u8; 10];
        let mut n = 0;
        for slot in &mut d {
            *slot = b'0' + (v % 10) as u8;
            n += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        for c in d.iter().take(n).rev() {
            self.b(*c);
        }
        self
    }

    fn c(&mut self) -> &mut Self {
        self.b(b',')
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::String;

    fn sample(t: u32, level: Level, playing: bool, io: u32) -> Sample {
        Sample {
            uptime_s: t,
            mv: 3900 - (t / 60) as u16,
            mv_raw: 3895,
            percent: 63,
            charge: ChargeState::NotPresent,
            cable: false,
            level,
            playing,
            backlight: 0,
            screen: false,
            bt: 0,
            card_io: io,
        }
    }

    fn text(b: &[u8]) -> String {
        String::from_utf8(b.to_vec()).unwrap()
    }

    #[test]
    fn sixty_samples_make_a_row() {
        let mut s = Sampler::new();
        for t in 1..60 {
            let l = if t <= 45 { Level::Base } else { Level::Mid };
            assert_eq!(s.feed(sample(t, l, t % 2 == 0, 1000 + t * 3)), None);
        }
        let r = s.feed(sample(60, Level::Mid, true, 1200)).unwrap();
        assert_eq!(r.secs, [0, 45, 15, 0, 0, 0]);
        assert_eq!(r.secs.iter().map(|&x| u32::from(x)).sum::<u32>(), 60);
        assert_eq!(r.playing_s, 30);
        assert_eq!(r.uptime_s, 60);
        // The first row counts from its first sample (1003).
        assert_eq!(r.card_io, 197);
        assert_eq!(r.level, Level::Mid);
        // The next row counts from the last one's I/O.
        let r2 = (61..=120).filter_map(|t| s.feed(sample(t, Level::Max, false, 1200 + (t - 60)))).last();
        let r2 = r2.unwrap();
        assert_eq!(r2.card_io, 60);
        assert_eq!(r2.secs, [0, 0, 0, 0, 0, 60]);
        assert_eq!(s.cut(), None);
    }

    #[test]
    fn row_csv() {
        let r = Row {
            uptime_s: 3600,
            mv: 3912,
            mv_raw: 3906,
            percent: 63,
            charge: ChargeState::Charging,
            cable: true,
            level: Level::High,
            secs: [0, 52, 8, 0, 0, 0],
            playing_s: 60,
            backlight: 12,
            screen: true,
            bt: 6,
            card_io: 41,
        };
        let mut b = [0u8; ROW_MAX];
        let n = format_row(&r, &mut b);
        assert_eq!(text(&b[..n]), "3600,3912,3906,63,charging,1,144,0,52,8,0,0,0,60,12,1,streaming,41\n");
        assert_eq!(text(&b[..n]).trim_end().split(',').count(), COLUMNS.split(',').count());
        // Widest values still fit ROW_MAX.
        let w = Row {
            uptime_s: u32::MAX,
            mv: u16::MAX,
            mv_raw: u16::MAX,
            percent: 255,
            charge: ChargeState::NoBattery,
            secs: [255; 6],
            playing_s: 255,
            backlight: 255,
            bt: 7,
            card_io: u32::MAX,
            level: Level::Max,
            ..r
        };
        let mut big = [0u8; 256];
        assert!(format_row(&w, &mut big) <= ROW_MAX);
        // A short buffer cuts, never panics.
        let mut small = [0u8; 10];
        assert_eq!(format_row(&r, &mut small), 10);
    }

    /// Regression: the battery logger "never wrote" while idle (PWR-5,
    /// 2026-11-24, on the player): rows went out only every five; the first
    /// row is now due after the first minute.
    #[test]
    fn log_header_name_and_rows() {
        let mut l = Log::new();
        assert!(!l.on() && !l.due());
        l.start(b"  Normal Screen-off!  ", true, false);
        assert_eq!(l.label(), b"normal_screen-of");
        assert!(l.on() && l.stay() && l.due(), "the header is due at once");
        l.file = 3;
        let mut b = [0u8; HEADER_MAX];
        let n = l.header(&mut b);
        assert!(n < HEADER_MAX);
        let h = text(&b[..n]);
        assert_eq!(
            h,
            std::format!(
                "# orpheus drain log v1 label=normal_screen-of file=3 perf=normal stay=1 row_s=60\n{COLUMNS}\n"
            )
        );
        let n = l.file_name(&mut b);
        assert_eq!(text(&b[..n]), "drain-3-normal_screen-of.csv");
        l.header_done = true;
        assert!(!l.due());
        let mut t = 0;
        // The first row is due at once (a new log shows data after a
        // minute).
        for i in 0..60 {
            t += 1;
            assert_eq!(l.second(sample(t, Level::Base, true, t)), i == 59, "first row due");
        }
        assert_eq!(l.pop().map(|r| r.uptime_s), Some(60));
        l.written_out(1);
        assert!(!l.due());
        for _ in 0..4 {
            for _ in 0..60 {
                t += 1;
                assert!(!l.second(sample(t, Level::Base, true, t)));
            }
        }
        assert_eq!(l.pending(), 4);
        for i in 0..60 {
            t += 1;
            assert_eq!(l.second(sample(t, Level::Base, true, t)), i == 59, "then due every fifth row");
        }
        let mut k = 1;
        while let Some(r) = l.pop() {
            k += 1;
            assert_eq!(r.uptime_s, 60 * k);
        }
        assert_eq!(k, 6);
        l.written_out(5);
        assert!(l.on() && !l.due());
        // Stop: a partial row, then off once written.
        for _ in 0..10 {
            t += 1;
            l.second(sample(t, Level::Mid, false, t));
        }
        l.end(End::Stop);
        assert!(l.due() && !l.stay());
        assert!(!l.second(sample(t + 1, Level::Mid, false, 0)), "no samples while ending");
        let r = l.pop().unwrap();
        assert_eq!((r.uptime_s, r.secs[2], r.playing_s), (t, 10, 0));
        assert_eq!(l.pop(), None);
        l.written_out(1);
        assert!(!l.on() && !l.due());
        assert_eq!(l.written, 7);
    }

    #[test]
    fn full_log_drops_the_oldest() {
        let mut l = Log::new();
        l.start(b"", false, true);
        assert_eq!(l.label(), b"run");
        assert!(!l.stay());
        l.header_done = true;
        for t in 1..=60 * 8 {
            l.second(sample(t, Level::Base, false, 0));
        }
        assert_eq!(l.pending(), LOG_ROWS);
        assert_eq!(l.lost, 2);
        assert_eq!(l.pop().unwrap().uptime_s, 180, "rows 1 and 2 went");
        let mut b = [0u8; 32];
        let n = format_lost(2, &mut b);
        assert_eq!(text(&b[..n]), "# lost 2 rows\n");
        let n = format_end(End::PowerOff, &mut b);
        assert_eq!(text(&b[..n]), "# power-off\n");
        // Ending with no partial row still ends.
        let mut m = Log::new();
        m.start(b"x", false, false);
        m.end(End::PowerOff);
        assert_eq!(m.ending(), Some(End::PowerOff));
        assert_eq!(m.pop(), None);
        m.written_out(0);
        assert!(!m.on());
        // Ending an idle log does nothing.
        let mut o = Log::new();
        o.end(End::Stop);
        assert_eq!(o.ending(), None);
    }

    #[test]
    fn file_numbers() {
        assert_eq!(file_number("drain-3-normal.csv"), Some(3));
        assert_eq!(file_number("DRAIN-12.CSV"), Some(12));
        assert_eq!(file_number("drain-7"), Some(7));
        assert_eq!(file_number("drain-x.csv"), None);
        assert_eq!(file_number("drain-3x.csv"), None);
        assert_eq!(file_number("drain-999999.csv"), None);
        assert_eq!(file_number("drain-"), None);
        assert_eq!(file_number("other.csv"), None);
        assert_eq!(file_number("drä"), None);
        let mut b = [0u8; 8];
        assert_eq!(file_name(65535, b"label", &mut b), 8, "cut to the buffer");
    }

    #[test]
    fn labels() {
        let mut o = [0u8; LABEL_MAX];
        assert_eq!(clean_label(b"   ", &mut o), 0);
        let n = clean_label(b"Max-288 ok", &mut o);
        assert_eq!(&o[..n], b"max-288_ok");
        let n = clean_label("é".as_bytes(), &mut o);
        assert_eq!(&o[..n], b"__");
        assert_eq!(dir(), DIR);
        for (i, w) in BT_WORDS.iter().enumerate() {
            assert_eq!(bt_word(i as u8), w.as_bytes());
        }
        assert_eq!(bt_word(200), b"notresponding");
        assert_eq!(charge_word(ChargeState::Fault), b"fault");
    }
}
