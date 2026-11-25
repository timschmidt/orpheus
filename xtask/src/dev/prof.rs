//! `cargo xtask dev prof` (WP-T-8, 07 §5.6): run the PC sampler or the
//! fault log on the player while something happens, read the profiler
//! block, symbolise and report (`crate::profile`), and optionally turn the
//! fault profile into a `hot.toml` cluster.
//!
//! ```text
//! cargo xtask dev prof --pc SECONDS | --faults SECONDS
//!     [--burst] [--period-us N] [--window-ms N] [--top N] [--any-build]
//!     [--during "<dev subcommand>"] [--only "<soak action text>"]
//!     [--update-hot NAME [--lines N] [--ro-lines N]]
//! cargo xtask prof target/orpheus/prof/<stamp> [--update-hot NAME …]
//! ```
//!
//! The second form (no player, so not under `dev`) reports a saved capture
//! again, against the current `target/orpheus/layout.json` and `hot.toml`.
//!
//! With `--during` the subcommand runs in this process (one process talks
//! to the player, CLAUDE.md rule 7), one step after the other: the capture
//! starts, the subcommand runs, the capture stops. A soak (`soak --replay
//! FILE`) starts and ends with a reboot, so it calls [`hook`] instead:
//! after its prologue, around each action (`--only` limits the capture to
//! the first action whose text contains the given words, e.g. `browse
//! pictures`) and before its final reboot. `SECONDS` is ignored then.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use orpheus_host::devchan::DevChannel;
use orpheus_proto::devchan::REQ_TEST;
use orpheus_proto::prof::{self as p, Capture};
use serde::{Deserialize, Serialize};

use crate::args::{Args, num};
use crate::pagelink::{Layout, artefacts};
use crate::profile::{self, Budget, SymTab, Totals};
use crate::rules;
use crate::util::{log, out_dir, root};

/// Subcommands `--during` may run: those that leave the firmware running
/// and write no NOR.
const DURING_OK: [&str; 14] = [
    "soak",
    "keys",
    "cmd",
    "suite",
    "fuzz-ui",
    "stats",
    "info",
    "mem",
    "shot",
    "put",
    "time",
    "usbtest",
    "usb-cancel",
    "usbimage-test",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Pc,
    Faults,
}

/// What was read from the player, as saved in `meta.json` beside
/// `capture.bin`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Meta {
    pub kind: Kind,
    pub command: String,
    pub during: Option<String>,
    pub only: Option<String>,
    pub totals: Totals,
    pub build_id: String,
    pub stamp: String,
    /// The log ring's lines when the capture stopped (`art: cover search`,
    /// `pcm: head`, … say what the code did meanwhile).
    #[serde(default)]
    pub log: Vec<String>,
}

/// One capture in progress or done (shared with the soak's hook).
struct Session {
    arm: String,
    only: Option<String>,
    any_build: bool,
    started: Option<[u32; 5]>,
    build_id: String,
    done: Option<Captured>,
    error: Option<String>,
    note: Option<String>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

/// Where a `--during` subcommand is (`dev::soak` calls [`hook`]).
#[derive(Clone, Copy, Debug)]
pub enum Phase<'a> {
    /// After the prologue (a fresh boot).
    Start,
    /// Before / after action `i` with its text.
    Before(usize, &'a str),
    After(usize, &'a str),
    /// Before the final reboot.
    End,
}

/// Called by a soak at each phase; does nothing unless `dev prof --during`
/// runs. Errors are kept for `prof` to report, never thrown at the soak.
pub fn hook(sim: bool, phase: Phase<'_>) {
    let Ok(mut g) = SESSION.lock() else { return };
    let Some(s) = g.as_mut() else { return };
    if sim {
        s.note = Some("the soak ran in the simulator: nothing to profile".into());
        return;
    }
    let (start, stop) = decide(s.only.as_deref(), phase);
    if start && s.started.is_none() && s.done.is_none() {
        if let Phase::Before(i, t) = phase {
            log(&format!("   prof: capture starts before #{i} {t}"));
        }
        match open().and_then(|d| begin(&d, &s.arm, s.any_build)) {
            Ok((w, id)) => {
                s.started = Some(w);
                s.build_id = id;
            }
            Err(e) => s.error = Some(format!("{e:#}")),
        }
    } else if stop && let Some(w) = s.started.take() {
        if let Phase::After(i, t) = phase {
            log(&format!("   prof: capture stops after #{i} {t}"));
        }
        match open().and_then(|d| finish(&d, w)) {
            Ok(r) => s.done = Some(r),
            Err(e) => s.error = Some(format!("{e:#}")),
        }
    }
}

/// Whether `phase` starts or stops the capture. Without `only`: from the
/// start (after the prologue) to the end; with it: around the actions whose
/// text contains it (the first one, as a capture is taken once), and the
/// end stops a capture still running.
fn decide(only: Option<&str>, phase: Phase<'_>) -> (bool, bool) {
    match (only, phase) {
        (None, Phase::Start) => (true, false),
        (Some(o), Phase::Before(_, t)) => (t.contains(o), false),
        (Some(o), Phase::After(_, t)) => (false, t.contains(o)),
        (_, Phase::End) => (false, true),
        _ => (false, false),
    }
}

fn open() -> Result<DevChannel> {
    DevChannel::open(Duration::from_secs(5)).context("open the developer channel")
}

/// A test command and its reply (the firmware answers `prof …` at once,
/// from the USB task; retried while the player is too busy to answer).
fn command(d: &DevChannel, text: &str) -> Result<String> {
    let mut last = None;
    for _ in 0..5 {
        match d.control_out(REQ_TEST, 0, 0, text.as_bytes()) {
            Ok(()) => {
                std::thread::sleep(Duration::from_millis(30));
                let r = d.control_in(REQ_TEST, 0, 0, 512)?;
                let r = String::from_utf8_lossy(&r).into_owned();
                if r.starts_with("prof:") || !text.starts_with("prof") {
                    return Ok(r);
                }
                last = Some(anyhow!("reply `{r}`"));
            }
            Err(e) => last = Some(e),
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(last.unwrap_or_else(|| anyhow!("no reply")).context(format!("`{text}`")))
}

/// Pager counters and uptime (STATS group 0: loads, code faults, rodata
/// faults, evictions, uptime ms).
fn counters(d: &DevChannel) -> Result<[u32; 5]> {
    let w = d.stats(0)?;
    if w.len() < 9 {
        bail!("short STATS reply ({} words)", w.len());
    }
    Ok([w[4], w[5], w[6], w[7], w[8]])
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Check the build, start the capture; the counters at the start and the
/// running build id.
fn begin(d: &DevChannel, arm: &str, any_build: bool) -> Result<([u32; 5], String)> {
    let info = d.info()?;
    let id = hex(&info.build_id);
    let layout = Layout::load(&artefacts("firmware").layout)?;
    if id != layout.build_id {
        let msg = format!(
            "the player runs build {id}, target/orpheus/layout.json is build {}: the symbols would be wrong (stage this build, or --any-build)",
            layout.build_id
        );
        if !any_build {
            bail!(msg);
        }
        log(&format!("   warning: {msg}"));
    }
    let r = command(d, "prof status")?;
    if !r.starts_with("prof:") {
        bail!("the firmware has no profiler (`prof status` said `{r}`): stage a T-8 build");
    }
    let w = counters(d)?;
    let r = command(d, arm)?;
    if r.contains("refused") || !r.contains(" on") {
        bail!("`{arm}`: {r}");
    }
    log(&format!("   {r}"));
    Ok((w, id))
}

/// Stop the capture and read the block; the pager counters over it.
/// The block, the pager counters over the capture, the log ring's lines.
type Captured = (Vec<u8>, Totals, Vec<String>);

fn finish(d: &DevChannel, w0: [u32; 5]) -> Result<Captured> {
    let r = command(d, "prof off")?;
    log(&format!("   {r}"));
    let w1 = counters(d)?;
    if w1[4] < w0[4] {
        bail!(
            "the player rebooted during the capture (uptime {} -> {} ms): the capture is lost",
            w0[4],
            w1[4]
        );
    }
    let block = d.peek(p::BASE, p::SIZE)?;
    let log = log_lines(d).unwrap_or_default();
    let d = |i: usize| u64::from(w1[i].wrapping_sub(w0[i]));
    Ok((block, Totals { loads: d(0), code_faults: d(1), ro_faults: d(2), evictions: d(3), ms: d(4) }, log))
}

fn log_lines(d: &DevChannel) -> Result<Vec<String>> {
    let area = d.log_area()?;
    let mut out = vec![0u8; area.len()];
    let Some((n, _)) = orpheus_proto::logring::unroll(&area, &mut out) else { return Ok(Vec::new()) };
    Ok(out[..n]
        .split(|b| *b == 0)
        .filter(|l| !l.is_empty())
        .map(|l| String::from_utf8_lossy(l).into_owned())
        .collect())
}

fn stamp() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (y, mo, d, h, mi, s) = super::civil(now);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// Report options.
struct Report {
    window_ms: u64,
    top: usize,
    update_hot: Option<String>,
    budget: Budget,
}

fn report_opts(args: &mut Args) -> Result<Report> {
    Ok(Report {
        window_ms: args.value("window-ms").map(|s| num(&s)).transpose()?.map_or(1000, u64::from),
        top: args.value("top").map(|s| num(&s)).transpose()?.map_or(40, |n| n as usize),
        update_hot: args.value("update-hot"),
        budget: {
            let code_lines = args.value("lines").map(|s| num(&s)).transpose()?.unwrap_or(24);
            let ro_lines = args.value("ro-lines").map(|s| num(&s)).transpose()?.unwrap_or(8);
            if code_lines == 0 || code_lines > 24 {
                bail!("--lines: 1 to 24 (a cluster leaves 8 of the 32 code lines for everything else)");
            }
            if ro_lines > 16 {
                bail!("--ro-lines: at most 16 (the rodata cache)");
            }
            Budget { code_lines, ro_lines }
        },
    })
}

/// `cargo xtask prof DIR …`: report a saved capture again (host only).
pub fn report_cmd(args: &mut Args) -> Result<String> {
    let rep = report_opts(args)?;
    let dir = args.positional("the capture directory (target/orpheus/prof/STAMP)")?;
    analyse(Path::new(&dir), &rep)
}

pub fn cmd(args: &mut Args) -> Result<String> {
    let rep = report_opts(args)?;
    let pc = args.flag_or_value("pc");
    let faults = args.flag_or_value("faults");
    let (kind, secs) = match (pc, faults) {
        (Some(s), None) => (Kind::Pc, s),
        (None, Some(s)) => (Kind::Faults, s),
        (Some(_), Some(_)) => bail!("one of --pc or --faults per run (the block holds one capture)"),
        (None, None) => {
            bail!("cargo xtask dev prof --pc SECONDS | --faults SECONDS [--during \"…\"] (see 07 §5.6)")
        }
    };
    let burst = args.flag("burst");
    let period = args.value("period-us").map(|s| num(&s)).transpose()?.unwrap_or(1000);
    let during = args.value("during");
    let only = args.value("only");
    let any_build = args.flag("any-build");
    let how = if burst { "burst" } else { "on" };
    let arm = match kind {
        Kind::Pc => format!("prof pc {how} {period}"),
        Kind::Faults => format!("prof faults {how}"),
    };
    let secs: f64 = match (&secs, &during) {
        (Some(s), _) => s.parse().with_context(|| format!("bad SECONDS `{s}`"))?,
        (None, Some(_)) => 0.0,
        (None, None) => bail!("--pc/--faults need SECONDS without --during"),
    };
    let (block, totals, log, build_id) = match &during {
        None => {
            let d = open()?;
            let (w0, id) = begin(&d, &arm, any_build)?;
            log(&format!("   capturing for {secs:.0} s"));
            let t0 = Instant::now();
            while t0.elapsed().as_secs_f64() < secs {
                std::thread::sleep(Duration::from_millis(200));
            }
            let (b, t, l) = finish(&d, w0)?;
            (b, t, l, id)
        }
        Some(text) => {
            let items: Vec<String> = text.split_whitespace().map(str::to_string).collect();
            let Some(sub) = items.first().cloned() else { bail!("--during needs a dev subcommand") };
            if !DURING_OK.contains(&sub.as_str()) {
                bail!("--during: not `{sub}` (allowed: {})", DURING_OK.join(", "));
            }
            let hooked = sub == "soak";
            if only.is_some() && !hooked {
                bail!("--only works with `--during \"soak …\"` (it names a soak action)");
            }
            *SESSION.lock().map_err(|_| anyhow!("lock"))? = Some(Session {
                arm: arm.clone(),
                only: only.clone(),
                any_build,
                started: None,
                build_id: String::new(),
                done: None,
                error: None,
                note: None,
            });
            if !hooked {
                hook(false, Phase::Start);
            }
            let r = super::dispatch(&sub, &mut Args::from_vec(items[1..].to_vec()));
            if !hooked {
                hook(false, Phase::End);
            }
            let s =
                SESSION.lock().map_err(|_| anyhow!("lock"))?.take().ok_or_else(|| anyhow!("no session"))?;
            match &r {
                Ok(m) => log(&format!("   {sub}: {m}")),
                Err(e) => log(&format!("   {sub} failed: {e:#}")),
            }
            if let Some(e) = s.error {
                bail!("prof: {e}");
            }
            let Some((b, t, l)) = s.done else {
                bail!(
                    "prof: nothing captured{}",
                    s.note.map_or_else(
                        || only.map_or(String::new(), |o| format!(" (no soak action contains `{o}`)")),
                        |n| format!(" ({n})")
                    )
                )
            };
            (b, t, l, s.build_id)
        }
    };
    let st = stamp();
    let dir = out_dir().join("prof").join(&st);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("capture.bin"), &block)?;
    let meta = Meta { kind, command: arm, during, only, totals, build_id, stamp: st, log };
    std::fs::write(dir.join("meta.json"), serde_json::to_string_pretty(&meta)?)?;
    analyse(&dir, &rep)
}

/// Symbolise and report a saved capture (`capture.bin` + `meta.json` in
/// `dir`); the reports go into the same directory.
fn analyse(dir: &Path, rep: &Report) -> Result<String> {
    let block =
        std::fs::read(dir.join("capture.bin")).with_context(|| format!("{}/capture.bin", dir.display()))?;
    let meta: Meta = serde_json::from_str(&std::fs::read_to_string(dir.join("meta.json"))?)?;
    let c = Capture::parse(&block).map_err(|e| anyhow!("the profiler block does not parse: {e:?}"))?;
    let a = artefacts("firmware");
    let layout = Layout::load(&a.layout)?;
    if !meta.build_id.is_empty() && meta.build_id != layout.build_id {
        log(&format!(
            "   warning: captured on build {}, symbolised against {} (target/orpheus/layout.json)",
            meta.build_id, layout.build_id
        ));
    }
    let elf = std::fs::read(&a.elf).ok();
    let hot_path = root().join("hot.toml");
    let clusters = rules::load_hot(&hot_path, "firmware")?;
    let tab = SymTab::new(&layout, elf.as_deref(), &clusters)?;
    let mut summary = Vec::new();
    if !meta.log.is_empty() {
        std::fs::write(dir.join("log.txt"), meta.log.join("\n") + "\n")?;
    }
    // What the cover search measured itself (AU-14: entries, card I/O and
    // line loads per search) while the capture ran.
    let art: Vec<&String> = meta.log.iter().filter(|l| l.contains("art: cover search")).collect();
    for l in &art {
        println!("log: {l}");
    }
    if !art.is_empty() {
        summary.push(format!("{} `art: cover search` lines in log.txt", art.len()));
    }
    match meta.kind {
        Kind::Pc => {
            let pr = profile::pc_profile(&c, &tab);
            let text = pr.text(rep.top);
            std::fs::write(dir.join("pc.txt"), &text)?;
            std::fs::write(dir.join("pc-functions.csv"), pr.csv_functions())?;
            std::fs::write(dir.join("pc-crates.csv"), pr.csv_crates())?;
            print!("{text}");
            summary.push(format!("{} PC samples", pr.samples));
            if rep.update_hot.is_some() {
                bail!("--update-hot needs a fault capture (--faults)");
            }
        }
        Kind::Faults => {
            let fp = profile::fault_profile(&c, &tab, rep.window_ms, Some(meta.totals));
            let text = fp.text(&tab, rep.top);
            std::fs::write(dir.join("faults.txt"), &text)?;
            std::fs::write(dir.join("faults-sections.csv"), fp.csv_sections(&tab))?;
            std::fs::write(dir.join("faults-functions.csv"), fp.csv_functions())?;
            std::fs::write(dir.join("faults-windows.csv"), fp.csv_windows())?;
            print!("{text}");
            summary.push(format!("{} faults logged (1 in {})", fp.events, fp.stride));
            if let Some(name) = &rep.update_hot {
                let g = profile::generate(name, &fp, &tab, rep.budget);
                let note = format!(
                    "{}, {}{}",
                    meta.stamp,
                    meta.during.as_deref().unwrap_or("manual"),
                    meta.only.as_ref().map_or(String::new(), |o| format!(", only `{o}`"))
                );
                let block = g.block(&note);
                std::fs::write(dir.join(format!("hot-{name}.toml")), &block)?;
                let text = std::fs::read_to_string(&hot_path)?;
                let new = profile::update_hot(&text, name, &block)?;
                std::fs::write(&hot_path, new)?;
                let gt = g.text();
                print!("\n{gt}");
                std::fs::write(dir.join(format!("hot-{name}.txt")), &gt)?;
                summary.push(format!(
                    "hot.toml cluster `{name}` written ({} + {} patterns); rebuild (`cargo xtask build`) and profile again",
                    g.code.len(),
                    g.ro.len()
                ));
            }
        }
    }
    summary.push(format!("reports in {}", rel(dir)));
    Ok(summary.join("; "))
}

fn rel(p: &Path) -> String {
    p.strip_prefix(root()).map_or_else(|_| p.display().to_string(), |r| r.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_spans_the_run_or_the_named_action() {
        assert_eq!(decide(None, Phase::Start), (true, false));
        assert_eq!(decide(None, Phase::Before(5, "browse pictures 23 1")), (false, false));
        assert_eq!(decide(None, Phase::End), (false, true));
        let only = Some("browse pictures");
        assert_eq!(decide(only, Phase::Start), (false, false));
        assert_eq!(decide(only, Phase::Before(2, "browse files 37 1")), (false, false));
        assert_eq!(decide(only, Phase::Before(5, "browse pictures 23 1669336479")), (true, false));
        assert_eq!(decide(only, Phase::After(5, "browse pictures 23 1669336479")), (false, true));
        assert_eq!(decide(only, Phase::After(6, "radio-keys 8 2")), (false, false));
        assert_eq!(decide(only, Phase::End), (false, true));
    }

    #[test]
    fn mode_options_take_an_optional_value() {
        let mut a = Args::from_vec(
            ["--pc", "30", "--faults", "--during", "soak --replay x"].map(String::from).to_vec(),
        );
        assert_eq!(a.flag_or_value("pc"), Some(Some("30".into())));
        assert_eq!(a.flag_or_value("faults"), Some(None));
        assert_eq!(a.value("during").as_deref(), Some("soak --replay x"));
        assert_eq!(a.flag_or_value("pc"), None);
        let mut b = Args::from_vec(vec!["--faults=12".into()]);
        assert_eq!(b.flag_or_value("faults"), Some(Some("12".into())));
    }

    #[test]
    fn meta_round_trips() {
        let m = Meta {
            kind: Kind::Faults,
            command: "prof faults on".into(),
            during: Some("soak --replay f".into()),
            only: Some("browse pictures".into()),
            totals: Totals { loads: 1, code_faults: 2, ro_faults: 3, evictions: 4, ms: 5 },
            build_id: "ab".into(),
            stamp: "20261125-120000".into(),
            log: vec!["art: cover search 253 ms, 37 entries".into()],
        };
        let t = serde_json::to_string(&m).unwrap();
        assert!(t.contains("\"kind\":\"faults\""));
        let back: Meta = serde_json::from_str(&t).unwrap();
        assert_eq!(back.totals, m.totals);
        assert_eq!(back.only, m.only);
        assert_eq!(back.log, m.log);
        // Older captures had no log.
        let old = t.replace(",\"log\":[\"art: cover search 253 ms, 37 entries\"]", "");
        assert!(serde_json::from_str::<Meta>(&old).unwrap().log.is_empty());
    }
}
