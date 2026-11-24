//! `cargo xtask drain report FILE.csv [FILE2.csv …]` (WP PWR-5): reads the
//! battery logger's files (`orpheus_power::drain`, written by the firmware
//! to `/.orpheus/test/drain-<n>-<label>.csv`) and prints, per file, the
//! duration, the voltage drop, mV per hour from the end points and from a
//! least-squares line, and the share of time at each clock level and
//! playing; then one comparison table across the files.
//!
//! Rows taken with the cable in are not discharge (the player runs from
//! VBUS): they are left out of the voltage figures when the file has any
//! rows without it.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use orpheus_power::drain::COLUMNS;

use crate::args::Args;

const USAGE: &str = "usage: cargo xtask drain report FILE.csv [FILE2.csv …]";
const LEVELS: [&str; 6] = ["24", "48", "96", "144", "192", "288"];

pub fn cmd(args: &mut Args) -> Result<String> {
    match args.next_positional().as_deref() {
        Some("report") => {}
        _ => bail!("{USAGE}"),
    }
    let mut files = Vec::new();
    while let Some(p) = args.next_positional() {
        files.push(p);
    }
    if files.is_empty() {
        bail!("{USAGE}");
    }
    let mut logs = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).with_context(|| format!("reading {f}"))?;
        let name = Path::new(f).file_name().map_or_else(|| f.clone(), |n| n.to_string_lossy().into_owned());
        logs.push(parse(&name, &text).with_context(|| format!("parsing {f}"))?);
    }
    print!("{}", report(&logs));
    Ok(format!("{} file(s)", logs.len()))
}

/// One CSV row.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub uptime_s: u32,
    pub mv: u32,
    pub mv_raw: u32,
    pub percent: u32,
    pub charge: String,
    pub cable: bool,
    pub clock_mhz: u32,
    /// Seconds at 24, 48, 96, 144, 192, 288 MHz.
    pub secs: [u32; 6],
    pub playing_s: u32,
    pub backlight: u32,
    pub screen: bool,
    pub bt: String,
    pub card_io: u32,
}

impl Row {
    /// Seconds the row covers.
    fn len(&self) -> u32 {
        self.secs.iter().sum()
    }
}

/// One file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrainLog {
    pub name: String,
    pub label: String,
    pub perf: String,
    pub rows: Vec<Row>,
    /// `# lost N rows` notes, summed.
    pub lost: u32,
    /// `# stop` or `# power-off`.
    pub end: Option<String>,
}

fn field<'a>(header: &'a str, key: &str) -> Option<&'a str> {
    header.split_whitespace().find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

/// Parse a file's text.
pub fn parse(name: &str, text: &str) -> Result<DrainLog> {
    let mut log = DrainLog { name: name.to_string(), ..DrainLog::default() };
    let mut columns = false;
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(c) = line.strip_prefix('#') {
            let c = c.trim();
            if c.starts_with("orpheus drain log") {
                log.label = field(c, "label").unwrap_or("").to_string();
                log.perf = field(c, "perf").unwrap_or("").to_string();
            } else if let Some(n) = c.strip_prefix("lost ").and_then(|r| r.split_whitespace().next()) {
                log.lost += n.parse::<u32>().unwrap_or(0);
            } else if c == "stop" || c == "power-off" {
                log.end = Some(c.to_string());
            }
            continue;
        }
        if line.starts_with("uptime_s") {
            if line != COLUMNS {
                bail!("line {}: unknown columns (this tool reads `{COLUMNS}`)", i + 1);
            }
            columns = true;
            continue;
        }
        if !columns {
            bail!("line {}: a row before the column line", i + 1);
        }
        log.rows.push(parse_row(line).with_context(|| format!("line {}", i + 1))?);
    }
    if log.rows.is_empty() {
        bail!("no rows");
    }
    Ok(log)
}

fn parse_row(line: &str) -> Result<Row> {
    let f: Vec<&str> = line.split(',').collect();
    let n = COLUMNS.split(',').count();
    if f.len() != n {
        bail!("{} fields, expected {n}", f.len());
    }
    let u =
        |i: usize| -> Result<u32> { f[i].parse().map_err(|e| anyhow!("field {} `{}`: {e}", i + 1, f[i])) };
    let mut secs = [0; 6];
    for (k, s) in secs.iter_mut().enumerate() {
        *s = u(7 + k)?;
    }
    Ok(Row {
        uptime_s: u(0)?,
        mv: u(1)?,
        mv_raw: u(2)?,
        percent: u(3)?,
        charge: f[4].to_string(),
        cable: u(5)? != 0,
        clock_mhz: u(6)?,
        secs,
        playing_s: u(13)?,
        backlight: u(14)?,
        screen: u(15)? != 0,
        bt: f[16].to_string(),
        card_io: u(17)?,
    })
}

/// Least-squares line through (x, y): (slope, intercept, r²).
#[must_use]
pub fn fit(pts: &[(f64, f64)]) -> Option<(f64, f64, f64)> {
    if pts.len() < 2 {
        return None;
    }
    let n = pts.len() as f64;
    let (sx, sy) = pts.iter().fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
    let (mx, my) = (sx / n, sy / n);
    let sxx: f64 = pts.iter().map(|(x, _)| (x - mx).powi(2)).sum();
    let syy: f64 = pts.iter().map(|(_, y)| (y - my).powi(2)).sum();
    let sxy: f64 = pts.iter().map(|(x, y)| (x - mx) * (y - my)).sum();
    if sxx == 0.0 {
        return None;
    }
    let slope = sxy / sxx;
    let r2 = if syy == 0.0 { 1.0 } else { sxy * sxy / (sxx * syy) };
    Some((slope, my - slope * mx, r2))
}

/// The figures of one file.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    /// Seconds covered by the rows.
    pub duration_s: u32,
    /// Rows (and of those, rows with the cable in).
    pub rows: usize,
    pub cable_rows: usize,
    /// Smoothed mV at the first and last discharge row.
    pub mv_first: u32,
    pub mv_last: u32,
    /// mV per hour from the end points (positive = falling).
    pub mv_per_h: Option<f64>,
    /// Least-squares mV per hour (positive = falling) and r².
    pub fit_mv_per_h: Option<f64>,
    pub fit_r2: Option<f64>,
    /// Share of time at each level, and playing (0..1).
    pub level_share: [f64; 6],
    pub playing_share: f64,
    pub screen_rows: usize,
    pub card_io_per_min: f64,
}

#[must_use]
pub fn summarise(log: &DrainLog) -> Summary {
    let total: u32 = log.rows.iter().map(Row::len).sum();
    let mut lv = [0u32; 6];
    for r in &log.rows {
        for (a, b) in lv.iter_mut().zip(r.secs) {
            *a += b;
        }
    }
    let tf = f64::from(total.max(1));
    let playing: u32 = log.rows.iter().map(|r| r.playing_s).sum();
    let cable_rows = log.rows.iter().filter(|r| r.cable).count();
    // Discharge rows; all rows if every one had the cable in.
    let dis: Vec<&Row> = if cable_rows < log.rows.len() {
        log.rows.iter().filter(|r| !r.cable).collect()
    } else {
        log.rows.iter().collect()
    };
    let (first, last) = (dis[0], dis[dis.len() - 1]);
    let hours = f64::from(last.uptime_s.saturating_sub(first.uptime_s)) / 3600.0;
    let mv_per_h = (hours > 0.0).then(|| (f64::from(first.mv) - f64::from(last.mv)) / hours);
    let pts: Vec<(f64, f64)> =
        dis.iter().map(|r| (f64::from(r.uptime_s) / 3600.0, f64::from(r.mv))).collect();
    let f = fit(&pts);
    let io: u32 = log.rows.iter().map(|r| r.card_io).sum();
    Summary {
        duration_s: total,
        rows: log.rows.len(),
        cable_rows,
        mv_first: first.mv,
        mv_last: last.mv,
        mv_per_h,
        fit_mv_per_h: f.map(|(s, _, _)| -s),
        fit_r2: f.map(|(_, _, r2)| r2),
        level_share: lv.map(|s| f64::from(s) / tf),
        playing_share: f64::from(playing) / tf,
        screen_rows: log.rows.iter().filter(|r| r.screen).count(),
        card_io_per_min: f64::from(io) / (tf / 60.0),
    }
}

fn hm(s: u32) -> String {
    format!("{}:{:02}", s / 3600, s % 3600 / 60)
}

fn opt(v: Option<f64>, prec: usize) -> String {
    v.map_or_else(|| "-".into(), |v| format!("{v:.prec$}"))
}

/// The whole report.
#[must_use]
pub fn report(logs: &[DrainLog]) -> String {
    let mut o = String::new();
    let sums: Vec<Summary> = logs.iter().map(summarise).collect();
    for (l, s) in logs.iter().zip(&sums) {
        let _ = writeln!(o, "{}  (label {}, perf {})", l.name, or_dash(&l.label), or_dash(&l.perf));
        let _ = writeln!(
            o,
            "  duration   {} h:min ({} rows{}{})",
            hm(s.duration_s),
            s.rows,
            if l.lost > 0 { format!(", {} lost", l.lost) } else { String::new() },
            l.end.as_ref().map(|e| format!(", ended by {e}")).unwrap_or_default()
        );
        if s.cable_rows > 0 {
            let what = if s.cable_rows == s.rows {
                "all rows: no discharge figures"
            } else {
                "left out of the voltage figures"
            };
            let _ = writeln!(o, "  cable      {} rows with the cable in ({what})", s.cable_rows);
        }
        let _ = writeln!(
            o,
            "  voltage    {} → {} mV, drop {} mV",
            s.mv_first,
            s.mv_last,
            i64::from(s.mv_first) - i64::from(s.mv_last)
        );
        let _ = writeln!(o, "  rate       {} mV/h (end points)", opt(s.mv_per_h, 1));
        let _ = writeln!(
            o,
            "  fit        {} mV/h (least squares, r² {})",
            opt(s.fit_mv_per_h, 1),
            opt(s.fit_r2, 3)
        );
        let lv: Vec<String> = LEVELS
            .iter()
            .zip(s.level_share)
            .filter(|(_, v)| *v > 0.0)
            .map(|(m, v)| format!("{m} MHz {:.1} %", v * 100.0))
            .collect();
        let _ = writeln!(o, "  clock      {}", if lv.is_empty() { "-".into() } else { lv.join(", ") });
        let _ = writeln!(
            o,
            "  playing    {:.1} %   screen on in {} of {} rows   card I/O {:.1}/min",
            s.playing_share * 100.0,
            s.screen_rows,
            s.rows,
            s.card_io_per_min
        );
        o.push('\n');
    }
    let _ = writeln!(
        o,
        "{:<28} {:<8} {:>7} {:>7} {:>8} {:>8} {:>6} {:>7} {:>7} {:>7}",
        "file", "perf", "h:min", "drop mV", "mV/h", "fit mV/h", "r²", "@288 %", "@144 %", "play %"
    );
    for (l, s) in logs.iter().zip(&sums) {
        let name: String = l.name.chars().take(28).collect();
        let _ = writeln!(
            o,
            "{:<28} {:<8} {:>7} {:>7} {:>8} {:>8} {:>6} {:>7.1} {:>7.1} {:>7.1}",
            name,
            or_dash(&l.perf),
            hm(s.duration_s),
            i64::from(s.mv_first) - i64::from(s.mv_last),
            opt(s.mv_per_h, 1),
            opt(s.fit_mv_per_h, 1),
            opt(s.fit_r2, 3),
            s.level_share[5] * 100.0,
            s.level_share[3] * 100.0,
            s.playing_share * 100.0
        );
    }
    if let [a, b, ..] = sums.as_slice()
        && let (Some(x), Some(y)) = (a.fit_mv_per_h, b.fit_mv_per_h)
        && x > 0.0
    {
        let _ = writeln!(o, "\nsecond file vs first: {:+.1} % mV/h (fit)", (y / x - 1.0) * 100.0);
    }
    o
}

fn or_dash(s: &str) -> &str {
    if s.is_empty() { "-" } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic file: `minutes` rows falling `mv_per_h`, all at one
    /// level (index `lvl`), playing.
    fn synth(label: &str, perf: &str, minutes: u32, mv_per_h: f64, lvl: usize, cable_first: u32) -> String {
        let mut s =
            format!("# orpheus drain log v1 label={label} file=1 perf={perf} stay=0 row_s=60\n{COLUMNS}\n");
        for m in 1..=minutes {
            let mv = (4100.0 - mv_per_h * f64::from(m) / 60.0).round() as u32;
            let mut secs = [0u32; 6];
            secs[lvl] = 60;
            let cable = u32::from(m <= cable_first);
            let _ = writeln!(
                s,
                "{},{mv},{},80,none,{cable},{},{},{},{},{},{},{},60,0,0,off,30",
                m * 60,
                mv - 3,
                LEVELS[lvl],
                secs[0],
                secs[1],
                secs[2],
                secs[3],
                secs[4],
                secs[5]
            );
        }
        s.push_str("# stop\n");
        s
    }

    #[test]
    fn parses_and_summarises() {
        let l = parse("drain-1-normal.csv", &synth("normal", "normal", 120, 60.0, 1, 0)).unwrap();
        assert_eq!((l.label.as_str(), l.perf.as_str(), l.rows.len()), ("normal", "normal", 120));
        assert_eq!(l.end.as_deref(), Some("stop"));
        let s = summarise(&l);
        assert_eq!(s.duration_s, 7200);
        assert_eq!(s.mv_first - s.mv_last, 119);
        assert!((s.mv_per_h.unwrap() - 60.0).abs() < 1.0, "{:?}", s.mv_per_h);
        assert!((s.fit_mv_per_h.unwrap() - 60.0).abs() < 0.5);
        assert!(s.fit_r2.unwrap() > 0.99);
        assert_eq!(s.level_share, [0.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
        assert!((s.playing_share - 1.0).abs() < 1e-9);
        assert!((s.card_io_per_min - 30.0).abs() < 1e-9);
    }

    #[test]
    fn cable_rows_are_left_out() {
        let l = parse("x.csv", &synth("max", "maximum", 60, 90.0, 5, 10)).unwrap();
        let s = summarise(&l);
        assert_eq!(s.cable_rows, 10);
        // First discharge row is minute 11.
        assert_eq!(s.mv_first, (4100.0 - 90.0 * 11.0 / 60.0_f64).round() as u32);
        assert!((s.fit_mv_per_h.unwrap() - 90.0).abs() < 1.0);
    }

    #[test]
    fn comparison_table() {
        let a = parse("drain-1-normal.csv", &synth("normal", "normal", 120, 60.0, 3, 0)).unwrap();
        let b = parse("drain-2-max.csv", &synth("max", "maximum", 120, 75.0, 5, 0)).unwrap();
        let r = report(&[a, b]);
        assert!(r.contains("drain-1-normal.csv  (label normal, perf normal)"), "{r}");
        assert!(r.contains("288 MHz 100.0 %"), "{r}");
        assert!(r.contains("144 MHz 100.0 %"), "{r}");
        assert!(r.contains("second file vs first: +25.0 % mV/h (fit)"), "{r}");
        let table: Vec<&str> =
            r.lines().skip_while(|l| !l.starts_with("file ")).take_while(|l| !l.is_empty()).collect();
        assert_eq!(table.len(), 3, "{r}");
        assert!(table[1].starts_with("drain-1-normal.csv"));
        assert!(table[2].contains("maximum"));
    }

    #[test]
    fn notes_and_errors() {
        let mut t = synth("x", "normal", 3, 60.0, 1, 0);
        t.push_str("# lost 2 rows\n# lost 1 rows\n");
        let l = parse("x.csv", &t).unwrap();
        assert_eq!(l.lost, 3);
        let s = summarise(&l);
        assert_eq!(s.rows, 3);
        assert!(parse("x.csv", "").is_err(), "no rows");
        assert!(parse("x.csv", "1,2,3\n").is_err(), "row before columns");
        assert!(parse("x.csv", "uptime_s,mv\n").is_err(), "other columns");
        let bad = format!("{COLUMNS}\n60,3900\n");
        assert!(parse("x.csv", &bad).is_err(), "short row");
        // One row: no rate, no fit; the report still prints.
        let one = parse("x.csv", &synth("one", "", 1, 60.0, 1, 0)).unwrap();
        let s = summarise(&one);
        assert_eq!((s.mv_per_h, s.fit_mv_per_h), (None, None));
        assert!(report(&[one]).contains("perf -"));
    }

    /// The firmware's own formatter (`orpheus_power::drain`) into this
    /// parser: the file format is one contract.
    #[test]
    fn reads_what_the_firmware_writes() {
        use orpheus_power::charger::ChargeState;
        use orpheus_power::drain::{self as d, End, Log, Sample};
        use orpheus_power::governor::Level;
        let mut log = Log::new();
        log.start(b"Max", false, true);
        log.file = 7;
        let mut text = vec![0u8; d::HEADER_MAX];
        let n = log.header(&mut text);
        text.truncate(n);
        let mut buf = [0u8; d::ROW_MAX];
        for t in 1..=(30 * 60 + 25) {
            let s = Sample {
                uptime_s: t,
                mv: (4000 - t / 36) as u16,
                mv_raw: 3990,
                percent: 70,
                charge: ChargeState::NotPresent,
                cable: false,
                level: if t % 4 == 0 { Level::Max } else { Level::High },
                playing: true,
                backlight: 0,
                screen: false,
                bt: 6,
                card_io: t * 2,
            };
            log.second(s);
            while let Some(r) = log.pop() {
                let k = d::format_row(&r, &mut buf);
                text.extend_from_slice(&buf[..k]);
            }
        }
        log.end(End::PowerOff);
        while let Some(r) = log.pop() {
            let k = d::format_row(&r, &mut buf);
            text.extend_from_slice(&buf[..k]);
        }
        let k = d::format_end(End::PowerOff, &mut buf);
        text.extend_from_slice(&buf[..k]);
        let l = parse("drain-7-max.csv", std::str::from_utf8(&text).unwrap()).unwrap();
        assert_eq!((l.label.as_str(), l.perf.as_str()), ("max", "maximum"));
        assert_eq!(l.rows.len(), 31, "30 full rows and the partial one");
        assert_eq!(l.rows[30].len(), 25);
        assert_eq!(l.end.as_deref(), Some("power-off"));
        assert_eq!(l.rows[0].bt, "streaming");
        let s = summarise(&l);
        assert_eq!(s.duration_s, 30 * 60 + 25);
        assert!((s.level_share[5] - 0.25).abs() < 0.01, "{:?}", s.level_share);
        assert!((s.fit_mv_per_h.unwrap() - 100.0).abs() < 2.0, "{:?}", s.fit_mv_per_h);
        assert!((s.card_io_per_min - 120.0).abs() < 1.0, "{}", s.card_io_per_min);
    }

    #[test]
    fn line_fit() {
        let (m, b, r2) = fit(&[(0.0, 1.0), (1.0, 3.0), (2.0, 5.0)]).unwrap();
        assert!((m - 2.0).abs() < 1e-12 && (b - 1.0).abs() < 1e-12 && (r2 - 1.0).abs() < 1e-12);
        assert_eq!(fit(&[(1.0, 1.0)]), None);
        assert_eq!(fit(&[(1.0, 1.0), (1.0, 2.0)]), None);
    }

    #[test]
    fn command_on_files() {
        let dir = std::env::temp_dir().join(format!("orpheus-drain-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("drain-1-normal.csv");
        std::fs::write(&p, synth("normal", "normal", 30, 60.0, 1, 0)).unwrap();
        let mut a = Args::from_vec(vec!["report".into(), p.to_string_lossy().into_owned()]);
        assert_eq!(cmd(&mut a).unwrap(), "1 file(s)");
        let mut a = Args::from_vec(vec!["report".into()]);
        assert!(cmd(&mut a).is_err());
        let mut a = Args::from_vec(vec!["other".into()]);
        assert!(cmd(&mut a).is_err());
        let mut a =
            Args::from_vec(vec!["report".into(), dir.join("missing.csv").to_string_lossy().into_owned()]);
        assert!(cmd(&mut a).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
