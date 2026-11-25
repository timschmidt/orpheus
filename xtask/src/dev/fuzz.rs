//! `cargo xtask dev fuzz-ui` (VER-8 groundwork, 07 §5.7): random key
//! sequences on the player, drawn from the `ui_keys` fuzz grammar
//! (`sim/fuzz/src/lib.rs`: 3-byte operations, seeded from the committed
//! corpus `tests/fuzz-corpus/ui_keys/` and fresh random bytes), and after
//! every K keys the oracles: no new fault record, uptime monotonic (no
//! reset), stack free above `budgets.toml [hil] stack_free_min_bytes`, no
//! underruns while playing; at the end, the card's root listing unchanged
//! (writes only under `/.orpheus/`). A failing sequence is replayed and
//! minimised by delta debugging into `tests/regress/ui-device/`.
//!
//! Safety (CLAUDE.md rules 4–6): no Power key; before every key the UI's
//! top screen is read (`ui top`) and a key that could do harm becomes Back:
//! anything in a dialog (delete, format, factory reset, restart, wipe), OK
//! where it would hand the card to the computer, switch Bluetooth, record
//! outside `/.orpheus/`, set up the vault with an unknown PIN, lock the
//! keys, hide Home apps or (without `--settings`) change settings. Locked
//! keys stop the session. Every session ends in a reboot.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use orpheus_ui::Key;

use super::player::{self, Player, Top, UsbPlayer};
use super::script::{self, Item, Step};
use crate::args::{Args, num};
use crate::util::{log, out_dir, root};

/// Keys of the grammar, in `sim/fuzz`'s order (`KEYS[a % 10]`).
pub const KEYS: [&str; 10] = ["up", "down", "prev", "next", "ok", "back", "menu", "vol+", "vol-", "power"];

/// Longest wait and hold sent to the player (the grammar allows 9.4 s and
/// 5.3 s; the player's time is real).
const MAX_WAIT_MS: u64 = 2000;
const MAX_HOLD_MS: u64 = 2000;

/// The device-safe part of `sim/fuzz`'s `command(op, a, b)`: keys (never
/// Power), holds, waits, long Back. `set:`, `goto:`, appearance and
/// language operations have no key equivalent and are skipped; `press:ok`
/// becomes a tap and `release:ok` is dropped (a held OK released over a
/// dialog would confirm it).
pub fn op_step(op: u8, a: u8, b: u8) -> Option<Step> {
    let name = KEYS[usize::from(a) % KEYS.len()];
    let k = Key::from_name(name)?;
    let keyed = |s: Step| (k != Key::Power).then_some(s);
    match op % 16 {
        0..=5 => keyed(Step::Tap(k, 1)),
        6 => keyed(Step::Hold(k, (200 + u64::from(b) * 20).min(MAX_HOLD_MS))),
        7 if k == Key::Ok => Some(Step::Tap(Key::Ok, 1)),
        7 => keyed(Step::Press(k)),
        8 if k == Key::Ok => None,
        8 => keyed(Step::Release(k)),
        9 => Some(Step::Wait((u64::from(b) * 37 + 1).min(MAX_WAIT_MS))),
        10..=14 => None,
        _ => Some(Step::Hold(Key::Back, 700 + u64::from(b) * 4)),
    }
}

/// splitmix64.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// The committed `ui_keys` inputs (corpus and regressions) as operations.
pub fn corpus() -> Vec<Vec<[u8; 3]>> {
    let mut v = Vec::new();
    for d in ["tests/fuzz-corpus/ui_keys", "tests/regress/ui_keys"] {
        let mut files: Vec<PathBuf> = std::fs::read_dir(root().join(d))
            .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_file()).collect())
            .unwrap_or_default();
        files.sort();
        for f in files {
            if let Ok(b) = std::fs::read(&f)
                && let Some((_, ops)) = b.split_first()
            {
                v.push(ops.as_chunks::<3>().0.to_vec());
            }
        }
    }
    v
}

/// `n` device steps: alternately a run from a corpus input (a random
/// slice, as the fuzzer mutates it) and fresh random operations.
pub fn generate(rng: &mut Rng, corpus: &[Vec<[u8; 3]>], n: usize) -> Vec<Step> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let ops: Vec<[u8; 3]> = if !corpus.is_empty() && rng.next() & 1 == 0 {
            let c = &corpus[rng.below(corpus.len())];
            let start = rng.below(c.len());
            c[start..].iter().take(4 + rng.below(12)).copied().collect()
        } else {
            (0..4 + rng.below(12))
                .map(|_| {
                    let r = rng.next().to_le_bytes();
                    [r[0], r[1], r[2]]
                })
                .collect()
        };
        for [op, a, b] in ops {
            if out.len() < n
                && let Some(s) = op_step(op, a, b)
            {
                out.push(s);
            }
        }
    }
    out
}

/// Screens where OK (tap, hold) becomes Back, and why.
pub const GUARD: [(&str, &str); 10] = [
    ("UsbConnect", "a personality change hands the card to the computer"),
    ("QuickPanel", "Lock (needs Power to undo), Bluetooth power"),
    ("Bluetooth", "module power and commands (CLAUDE.md rule 6)"),
    ("BtScan", "pairing (rule 6)"),
    ("BtPaired", "connect/forget (rule 6)"),
    ("BtConnect", "connect (rule 6)"),
    ("Recorder", "records to /Recordings, outside /.orpheus/ (rule 5)"),
    ("Authenticator", "vault set-up with an unknown PIN, PIN back-off"),
    ("HomeApps", "hides Home apps"),
    ("Settings", "settings change (allowed with --settings)"),
];

/// The step actually sent on `top`: harmful ones become a Back tap.
pub fn guard(top: &Top, s: &Step, settings: bool) -> Step {
    let back = Step::Tap(Key::Back, 1);
    let is_key = !matches!(s, Step::Wait(_) | Step::Shot(_) | Step::Expect(_));
    if top.dialog && is_key {
        return back;
    }
    let ok = matches!(s, Step::Tap(Key::Ok, _) | Step::Hold(Key::Ok, _) | Step::Press(Key::Ok));
    let side = matches!(s, Step::Tap(Key::Prev | Key::Next, _) | Step::Hold(Key::Prev | Key::Next, _));
    let denied = GUARD.iter().any(|(n, _)| *n == top.name && (*n != "Settings" || !settings));
    if denied && (ok || (top.name == "QuickPanel" || top.name == "UsbConnect") && side) {
        return back;
    }
    s.clone()
}

/// Turn the firmware's settings test session on (RAM-only settings until
/// the next reboot); false when the firmware has none.
pub fn session_on(p: &mut dyn Player) -> Result<bool> {
    let re = regex::Regex::new("^test-session|^commands:|^card:")?;
    let r = player::command(p, "settings test-session on", Some(&re), 3000).unwrap_or_default();
    Ok(session_reply_ok(&r))
}

/// Whether a `settings test-session on` reply confirms the session.
fn session_reply_ok(r: &str) -> bool {
    r.starts_with("test-session on")
}

/// What an oracle found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Fault,
    Reset,
    Stack,
    Underrun,
    Card,
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Fault => "fault",
            Kind::Reset => "reset",
            Kind::Stack => "stack",
            Kind::Underrun => "underrun",
            Kind::Card => "card",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub kind: Kind,
    pub detail: String,
}

/// State the oracles compare against.
#[derive(Clone, Debug, Default)]
pub struct Baseline {
    pub uptime_ms: u32,
    pub fault_count: Option<u32>,
    /// (frames, underruns) of the track playing at the last check.
    pub play: Option<(u64, u64)>,
    pub stack_min: u32,
}

fn field(reply: &str, key: &str) -> Option<u64> {
    reply.split_whitespace().find_map(|w| w.strip_prefix(key)?.strip_prefix('=')?.parse().ok())
}

pub fn baseline(p: &mut dyn Player, stack_min: u32) -> Result<Baseline> {
    Ok(Baseline {
        uptime_ms: p.health()?.uptime_ms,
        fault_count: p.fault()?.map(|f| f.count),
        play: None,
        stack_min,
    })
}

/// The oracles of 07 §5.7 (except the card, checked once at the end).
pub fn check(p: &mut dyn Player, b: &mut Baseline) -> Result<Option<Finding>> {
    // The fault record first: a crash also resets the uptime, and the
    // record says more.
    if let Some(f) = p.fault()?
        && Some(f.count) != b.fault_count
    {
        let kind = f.kind().map_or_else(|| format!("kind {:#x}", f.kind), |k| k.name().to_string());
        return Ok(Some(Finding {
            kind: Kind::Fault,
            detail: format!("fault record: {kind} pc={:#010x} lr={:#010x} {}", f.pc, f.lr, f.text_str()),
        }));
    }
    let h = p.health()?;
    if h.uptime_ms < b.uptime_ms {
        return Ok(Some(Finding {
            kind: Kind::Reset,
            detail: format!(
                "uptime went back from {} to {} ms (watchdog or reset)",
                b.uptime_ms, h.uptime_ms
            ),
        }));
    }
    b.uptime_ms = h.uptime_ms;
    if h.stack_free < b.stack_min {
        return Ok(Some(Finding {
            kind: Kind::Stack,
            detail: format!("stack free {} B of {} (budget {} B)", h.stack_free, h.stack_size, b.stack_min),
        }));
    }
    let s = player::command(p, "status", Some(&regex::Regex::new("underruns=|card: in use").unwrap()), 2000)
        .unwrap_or_default();
    if s.starts_with("playing") {
        let (fr, un) = (field(&s, "frames").unwrap_or(0), field(&s, "underruns").unwrap_or(0));
        let before = match b.play {
            Some((f0, u0)) if fr >= f0 => u0,
            _ => 0,
        };
        if un > before {
            return Ok(Some(Finding {
                kind: Kind::Underrun,
                detail: format!("{} underruns while playing: {s}", un - before),
            }));
        }
        b.play = Some((fr, un));
    } else {
        b.play = None;
    }
    Ok(None)
}

/// The card's root listing (`ls /`), without `/.orpheus/`.
pub fn root_listing(p: &mut dyn Player) -> Result<Option<Vec<String>>> {
    let re = regex::Regex::new(r"entries\)$|no such directory|card: in use").unwrap();
    let r = player::command(p, "ls /", Some(&re), 15_000)?;
    if !r.ends_with("entries)") {
        return Ok(None);
    }
    let mut v: Vec<String> = r
        .lines()
        .filter(|l| !l.starts_with('(') && !l.is_empty() && *l != ".orpheus/")
        .map(str::to_string)
        .collect();
    v.sort();
    Ok(Some(v))
}

/// Keys currently held by `press:` steps.
#[derive(Default)]
pub struct Exec {
    held: Vec<Key>,
    pub sent: Vec<Step>,
    pub settings: bool,
}

#[derive(Debug)]
pub struct Locked;

impl std::fmt::Display for Locked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the keys are locked (unlocking needs the Power key, by hand)")
    }
}

impl std::error::Error for Locked {}

impl Exec {
    pub fn release_all(&mut self, p: &mut dyn Player) -> Result<()> {
        for k in std::mem::take(&mut self.held) {
            p.key(k, false)?;
            self.sent.push(Step::Release(k));
        }
        Ok(())
    }

    /// Send one step through the guard.
    pub fn step(&mut self, p: &mut dyn Player, s: &Step) -> Result<()> {
        let t = player::top(p)?;
        if t.locked {
            return Err(Locked.into());
        }
        let g = guard(&t, s, self.settings);
        if g != *s && !self.held.is_empty() {
            self.release_all(p)?;
        }
        match &g {
            Step::Press(k) if self.held.contains(k) => return Ok(()),
            Step::Press(k) => self.held.push(*k),
            Step::Release(k) if !self.held.contains(k) => return Ok(()),
            Step::Release(k) => self.held.retain(|h| h != k),
            _ => {}
        }
        // Recorded before it is sent: a key that crashes the player is
        // part of the sequence.
        self.sent.push(g.clone());
        player::key_step(p, &g)
    }

    pub fn run(&mut self, p: &mut dyn Player, steps: &[Step]) -> Result<()> {
        for s in steps {
            self.step(p, s)?;
        }
        Ok(())
    }
}

/// Back to Home before a replay (keys released, dialogs closed).
pub fn reset_ui(p: &mut dyn Player) -> Result<()> {
    // Settings changed by keys stay in RAM: every session (and every
    // replay after a reboot) runs inside the firmware's test session.
    // Before 2026-11-25 `--settings` changed the owner's stored settings
    // through the menus (the equaliser, brightness, timer presets, …).
    session_on(p)?;
    let t = player::top(p)?;
    if t.locked {
        return Err(Locked.into());
    }
    if t.dialog || !t.screen_on {
        player::tap(p, Key::Back)?;
    }
    player::hold(p, Key::Back, script::HOLD_MS)?;
    p.sleep(200);
    Ok(())
}

/// Does `steps` (from Home, after a reboot for stack findings) give a
/// finding of `kind`?
pub fn reproduces(
    p: &mut dyn Player,
    steps: &[Step],
    kind: &Kind,
    settings: bool,
    stack_min: u32,
) -> Result<bool> {
    if *kind == Kind::Stack {
        p.reboot()?;
    }
    if let Err(e) = reset_ui(p) {
        p.reconnect().context(format!("{e:#}"))?;
        reset_ui(p)?;
    }
    let mut b = baseline(p, stack_min)?;
    let mut ex = Exec { settings, ..Exec::default() };
    let ran = ex.run(p, steps).and_then(|()| ex.release_all(p));
    let found = match ran {
        Ok(()) => check(p, &mut b)?,
        Err(e) if e.is::<Locked>() => return Err(e),
        Err(_) => {
            // The channel broke: the player reset under the sequence.
            p.reconnect()?;
            check(p, &mut b)?.or(Some(Finding { kind: Kind::Reset, detail: "channel lost".into() }))
        }
    };
    Ok(found.is_some_and(|f| same_kind(&f.kind, kind)))
}

/// A crash shows as a fault record, a reset or both.
fn same_kind(a: &Kind, b: &Kind) -> bool {
    a == b || matches!((a, b), (Kind::Fault | Kind::Reset, Kind::Fault | Kind::Reset))
}

/// Delta debugging (ddmin): a 1-minimal sub-sequence for which `test`
/// still holds, within `budget` tests.
pub fn ddmin<T: Clone>(
    items: Vec<T>,
    test: &mut dyn FnMut(&[T]) -> Result<bool>,
    budget: &mut u32,
) -> Result<Vec<T>> {
    let mut c = items;
    let mut n = 2usize;
    while c.len() >= 2 {
        let size = c.len().div_ceil(n);
        let parts: Vec<Vec<T>> = c.chunks(size).map(<[T]>::to_vec).collect();
        let mut reduced = false;
        for part in &parts {
            if *budget == 0 {
                return Ok(c);
            }
            *budget -= 1;
            if test(part)? {
                c = part.clone();
                n = 2;
                reduced = true;
                break;
            }
        }
        if !reduced && parts.len() > 2 {
            for i in 0..parts.len() {
                if *budget == 0 {
                    return Ok(c);
                }
                *budget -= 1;
                let comp: Vec<T> = parts
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .flat_map(|(_, x)| x.iter().cloned())
                    .collect();
                if test(&comp)? {
                    c = comp;
                    n = (n - 1).max(2);
                    reduced = true;
                    break;
                }
            }
        }
        if !reduced {
            if n >= c.len() {
                break;
            }
            n = (n * 2).min(c.len());
        }
    }
    Ok(c)
}

/// Taps of N keys become N single taps (finer minimisation).
fn single(steps: &[Step]) -> Vec<Step> {
    steps
        .iter()
        .flat_map(|s| match s {
            Step::Tap(k, n) => vec![Step::Tap(*k, 1); *n as usize],
            s => vec![s.clone()],
        })
        .collect()
}

/// Find the shortest reproducing tail of `history` (whole checks of
/// `chunk` steps, doubling), then ddmin it. `None`: it did not reproduce.
pub fn minimise(
    p: &mut dyn Player,
    history: &[Step],
    chunk: usize,
    kind: &Kind,
    settings: bool,
    stack_min: u32,
    budget: &mut u32,
) -> Result<Option<Vec<Step>>> {
    let mut tail = chunk.max(1);
    let start = loop {
        let from = history.len().saturating_sub(tail);
        if *budget == 0 {
            return Ok(None);
        }
        *budget -= 1;
        log(&format!("   replay: last {} steps", history.len() - from));
        if reproduces(p, &history[from..], kind, settings, stack_min)? {
            break from;
        }
        if from == 0 {
            return Ok(None);
        }
        tail *= 2;
    };
    let mut test = |s: &[Step]| -> Result<bool> {
        log(&format!("   ddmin: {} steps", s.len()));
        reproduces(p, s, kind, settings, stack_min)
    };
    Ok(Some(ddmin(single(&history[start..]), &mut test, budget)?))
}

/// A sequence as a `.keys` file (the simulator's script language).
pub fn keys_text(header: &[String], steps: &[Step]) -> String {
    let mut t: String = header.iter().map(|h| format!("# {h}\n")).collect();
    for line in steps.chunks(12) {
        let words: Vec<String> = line.iter().map(script::step_text).collect();
        t.push_str(&words.join(" "));
        t.push('\n');
    }
    t
}

/// Read a `.keys` file: key steps only.
pub fn read_keys(path: &Path) -> Result<Vec<Step>> {
    let t = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    script::parse_journey(&t)?
        .into_iter()
        .map(|i| match i {
            Item::Step(_, s) if !matches!(s, Step::Shot(_)) => Ok(s),
            other => Err(anyhow!("{}: only key steps in a .keys file, not {other:?}", path.display())),
        })
        .collect()
}

pub struct Config {
    pub seed: u64,
    pub minutes: f64,
    /// Stop after this many steps (tests; the player runs by time).
    pub max_steps: Option<usize>,
    pub check_every: usize,
    pub settings: bool,
    pub minimise: bool,
    pub max_replays: u32,
    pub stack_min: u32,
    pub regress_dir: PathBuf,
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub steps: usize,
    pub checks: usize,
    pub finding: Option<Finding>,
    pub minimised: Option<Vec<Step>>,
    pub saved: Option<PathBuf>,
    pub session: Vec<Step>,
}

/// One session: generate, guard, send, check; on a finding replay and
/// minimise. The caller does the card listing and the final reboot.
pub fn session(p: &mut dyn Player, c: &Config) -> Result<Outcome> {
    let mut rng = Rng::new(c.seed);
    let corpus = corpus();
    let t0 = Instant::now();
    let mut out = Outcome::default();
    reset_ui(p)?;
    let mut b = baseline(p, c.stack_min)?;
    let mut ex = Exec { settings: c.settings, ..Exec::default() };
    loop {
        if t0.elapsed() > Duration::from_secs_f64(c.minutes * 60.0)
            || c.max_steps.is_some_and(|m| out.steps >= m)
        {
            break;
        }
        let steps = generate(&mut rng, &corpus, c.check_every);
        let ran = ex.run(p, &steps).and_then(|()| ex.release_all(p));
        out.steps += steps.len();
        let found = match ran {
            Ok(()) => {
                out.checks += 1;
                check(p, &mut b)?
            }
            Err(e) if e.is::<Locked>() => {
                out.session = ex.sent;
                return Err(e);
            }
            Err(e) => {
                log(&format!("   channel lost: {e:#}; waiting for the player"));
                p.reconnect()?;
                let f = check(p, &mut b)?;
                Some(f.unwrap_or(Finding { kind: Kind::Reset, detail: format!("channel lost: {e:#}") }))
            }
        };
        if let Some(f) = found {
            log(&format!("   FOUND ({}): {}", f.kind.name(), f.detail));
            let history = ex.sent.clone();
            let mut header = vec![
                format!("fuzz-ui seed={} oracle={}: {}", c.seed, f.kind.name(), f.detail),
                format!("{} steps sent before the finding", history.len()),
                "replay: cargo xtask dev fuzz-ui --replay <this file>".into(),
            ];
            let mut steps = history.clone();
            if c.minimise {
                let mut budget = c.max_replays;
                match minimise(p, &history, c.check_every, &f.kind, c.settings, c.stack_min, &mut budget)? {
                    Some(m) => {
                        header.push(format!(
                            "minimised to {} steps ({} replays)",
                            m.len(),
                            c.max_replays - budget
                        ));
                        out.minimised = Some(m.clone());
                        steps = m;
                    }
                    None => header.push("did not reproduce on replay: the whole session is kept".into()),
                }
            }
            std::fs::create_dir_all(&c.regress_dir)?;
            let path = c.regress_dir.join(format!("{}-seed{}-{}.keys", f.kind.name(), c.seed, history.len()));
            std::fs::write(&path, keys_text(&header, &steps))?;
            log(&format!("   saved {}", path.display()));
            out.saved = Some(path);
            out.finding = Some(f);
            break;
        }
    }
    ex.release_all(p)?;
    out.session = ex.sent;
    Ok(out)
}

fn stack_budget() -> Result<u32> {
    let t: toml::Table = toml::from_str(&std::fs::read_to_string(root().join("budgets.toml"))?)?;
    t.get("hil")
        .and_then(|h| h.get("stack_free_min_bytes"))
        .and_then(toml::Value::as_integer)
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| anyhow!("budgets.toml: [hil] stack_free_min_bytes missing"))
}

pub fn cmd(args: &mut Args) -> Result<String> {
    let stack_min = stack_budget()?;
    let settings = args.flag("settings");
    let regress_dir = root().join("tests/regress/ui-device");
    let mut p = UsbPlayer::open()?;
    if settings && !session_on(&mut p)? {
        bail!(
            "--settings needs the firmware's settings test session (RAM-only settings): this firmware has none"
        );
    }
    if let Some(file) = args.value("replay") {
        let steps = read_keys(Path::new(&file))?;
        player::test_look(&mut p, true)?;
        reset_ui(&mut p)?;
        let mut b = baseline(&mut p, stack_min)?;
        let mut ex = Exec { settings, ..Exec::default() };
        let ran = ex.run(&mut p, &steps).and_then(|()| ex.release_all(&mut p));
        let found = match ran {
            Ok(()) => check(&mut p, &mut b)?,
            Err(e) => {
                p.reconnect()?;
                check(&mut p, &mut b)?.or(Some(Finding { kind: Kind::Reset, detail: format!("{e:#}") }))
            }
        };
        reset_ui(&mut p)?;
        let _ = player::test_look(&mut p, false);
        p.reboot()?;
        return match found {
            Some(f) => bail!("{}: {} ({})", file, f.kind.name(), f.detail),
            None => Ok(format!("{file}: {} steps, no finding", steps.len())),
        };
    }
    let c = Config {
        seed: u64::from(num(&args.value("seed").context("--seed N")?)?),
        minutes: args.value("minutes").context("--minutes M")?.parse().context("--minutes")?,
        max_steps: None,
        check_every: args.value("check-every").map(|s| num(&s)).transpose()?.unwrap_or(20) as usize,
        settings,
        minimise: !args.flag("no-minimise"),
        max_replays: args.value("max-replays").map(|s| num(&s)).transpose()?.unwrap_or(40),
        stack_min,
        regress_dir,
    };
    let play = args.flag("play");
    let before = root_listing(&mut p)?;
    // The simulator's appearance for the session (RAM only; a reboot, as
    // after a crash, clears it).
    player::test_look(&mut p, true)?;
    if play {
        let re = regex::Regex::new("playdir").unwrap();
        let r =
            player::command(&mut p, &format!("playdir {}/music", super::suite::FIXTURES), Some(&re), 10_000)?;
        if !r.contains("ok") {
            bail!("{r}: upload the fixtures first (`cargo xtask dev suite --setup home`)");
        }
    }
    log(&format!("fuzz-ui: seed {}, {} min, oracles every {} keys", c.seed, c.minutes, c.check_every));
    let r = session(&mut p, &c);
    // Clean up whatever happened: keys released, Home, playback stopped,
    // the card's root compared, then a reboot (CLAUDE.md rule 4).
    let _ = reset_ui(&mut p);
    let _ = player::command(&mut p, "stop", None, 0);
    let _ = player::test_look(&mut p, false);
    let card = match (before, root_listing(&mut p)) {
        (Some(a), Ok(Some(b))) if a != b => {
            let new: Vec<&String> = b.iter().filter(|x| !a.contains(x)).collect();
            let gone: Vec<&String> = a.iter().filter(|x| !b.contains(x)).collect();
            Some(Finding {
                kind: Kind::Card,
                detail: format!("card root changed: new {new:?}, gone {gone:?}"),
            })
        }
        _ => None,
    };
    let dir = out_dir().join("fuzz");
    std::fs::create_dir_all(&dir)?;
    let reboot = p.reboot();
    let o = r?;
    std::fs::write(
        dir.join(format!("session-seed{}.keys", c.seed)),
        keys_text(&[format!("fuzz-ui session seed={} ({} steps)", c.seed, o.session.len())], &o.session),
    )?;
    reboot?;
    let mut fails = Vec::new();
    if let Some(f) = &o.finding {
        fails.push(format!(
            "{}: {} (saved {})",
            f.kind.name(),
            f.detail,
            o.saved.as_ref().map_or_else(String::new, |p| p.display().to_string())
        ));
    }
    if let Some(f) = card {
        fails.push(format!("{}: {}", f.kind.name(), f.detail));
    }
    if !fails.is_empty() {
        bail!("{}", fails.join("; "));
    }
    Ok(format!("{} steps, {} oracle checks, no finding (session in {})", o.steps, o.checks, dir.display()))
}

#[cfg(test)]
mod tests {
    /// Regression (2026-11-25): `fuzz-ui --settings` changed the owner's
    /// stored settings through the menus; it now refuses to run unless the
    /// firmware confirms its RAM-only settings session.
    #[test]
    fn regress_settings_fuzzing_requires_the_ram_only_session() {
        assert!(super::session_reply_ok("test-session on held=0 store=ok keys=65 crc=1234abcd"));
        assert!(!super::session_reply_ok("commands: …"));
        assert!(!super::session_reply_ok(""));
    }

    use super::*;
    use crate::dev::player::{Health, SimPlayer};
    use orpheus_proto::fault::FaultRecord;

    #[test]
    fn grammar_matches_sim_fuzz() {
        // The grammar is `sim/fuzz`'s (a separate workspace): its key table
        // and operation numbers must not drift.
        let src = std::fs::read_to_string(root().join("sim/fuzz/src/lib.rs")).unwrap();
        assert!(src.contains(
            r#"const KEYS: [&str; 10] = ["up", "down", "prev", "next", "ok", "back", "menu", "vol+", "vol-", "power"];"#
        ));
        for arm in [
            "0..=5 => key.to_string()",
            r#"6 => format!("hold:{key}:{}", 200 + u32::from(b) * 20)"#,
            r#"7 => format!("press:{key}")"#,
            r#"8 => format!("release:{key}")"#,
            r#"9 => format!("wait:{}", u32::from(b) * 37 + 1)"#,
            r#"_ => format!("hold:back:{}", 700 + u32::from(b) * 4)"#,
        ] {
            assert!(src.contains(arm), "sim/fuzz `command` changed: {arm}");
        }
        assert_eq!(op_step(0, 4, 0), Some(Step::Tap(Key::Ok, 1)));
        assert_eq!(op_step(6, 1, 255), Some(Step::Hold(Key::Down, MAX_HOLD_MS)));
        assert_eq!(op_step(9, 0, 3), Some(Step::Wait(112)));
        assert_eq!(op_step(15, 0, 0), Some(Step::Hold(Key::Back, 700)));
        assert_eq!(op_step(12, 0, 0), None);
    }

    #[test]
    fn never_power() {
        let mut rng = Rng::new(7);
        let steps = generate(&mut rng, &corpus(), 5000);
        assert_eq!(steps.len(), 5000);
        for s in &steps {
            assert!(!script::step_text(s).contains("power"), "{s:?}");
            assert!(!matches!(s, Step::Press(Key::Ok) | Step::Release(Key::Ok)));
        }
        // Same seed, same sequence.
        assert_eq!(generate(&mut Rng::new(7), &corpus(), 5000), steps);
    }

    fn top(name: &str, dialog: bool) -> Top {
        Top { id: 0, name: name.into(), depth: 2, dialog, overlay: dialog, locked: false, screen_on: true }
    }

    #[test]
    fn guard_turns_harm_into_back() {
        let back = Step::Tap(Key::Back, 1);
        let ok = Step::Tap(Key::Ok, 1);
        assert_eq!(guard(&top("Files", true), &ok, true), back);
        assert_eq!(guard(&top("Files", true), &Step::Tap(Key::Down, 1), true), back);
        assert_eq!(guard(&top("Files", true), &Step::Wait(5), true), Step::Wait(5));
        assert_eq!(guard(&top("Files", false), &ok, false), ok);
        assert_eq!(guard(&top("Recorder", false), &Step::Hold(Key::Ok, 900), false), back);
        assert_eq!(guard(&top("Recorder", false), &Step::Tap(Key::Menu, 1), false), Step::Tap(Key::Menu, 1));
        assert_eq!(guard(&top("QuickPanel", false), &Step::Tap(Key::Next, 1), false), back);
        assert_eq!(guard(&top("Settings", false), &ok, false), back);
        assert_eq!(guard(&top("Settings", false), &ok, true), ok);
    }

    #[test]
    fn ddmin_finds_the_three_keys() {
        // Fails when menu, down, ok occur in this order.
        let has = |s: &[u8]| {
            let mut want = b"mdo".iter().peekable();
            for c in s {
                if want.peek() == Some(&c) {
                    want.next();
                }
            }
            want.peek().is_none()
        };
        let input: Vec<u8> = b"uuxmnnudpqqdooxbm".to_vec();
        let mut budget = 1000;
        let mut n = 0;
        let m = ddmin(
            input,
            &mut |s| {
                n += 1;
                Ok(has(s))
            },
            &mut budget,
        )
        .unwrap();
        assert_eq!(m, b"mdo");
        assert!(n < 100, "{n} tests");
    }

    /// A player with a planted bug: Menu, Down, OK in this order (with no
    /// Back in between) crash it (fault record, reboot).
    struct Buggy {
        seq: Vec<Key>,
        uptime: u64,
        faults: u32,
        crashed: bool,
        reply: String,
    }

    impl Player for Buggy {
        fn key(&mut self, k: Key, down: bool) -> Result<()> {
            if self.crashed {
                bail!("device gone");
            }
            if down && k == Key::Back {
                self.seq.clear();
            } else if down {
                self.seq.push(k);
                let mut want = [Key::Menu, Key::Down, Key::Ok].into_iter().peekable();
                for c in &self.seq {
                    if want.peek() == Some(c) {
                        want.next();
                    }
                }
                if want.peek().is_none() {
                    self.faults += 1;
                    self.crashed = true;
                }
            }
            Ok(())
        }
        fn sleep(&mut self, ms: u64) {
            self.uptime += ms;
        }
        fn send(&mut self, text: &str) -> Result<()> {
            if self.crashed {
                bail!("device gone");
            }
            self.reply = match text {
                "ui top" => top("Home", false).text(),
                "status" => "idle underruns=0".into(),
                _ => String::new(),
            };
            Ok(())
        }
        fn reply(&mut self) -> Result<String> {
            Ok(self.reply.clone())
        }
        fn frame(&mut self) -> Result<Vec<u16>> {
            Ok(vec![0; 128 * 128])
        }
        fn health(&mut self) -> Result<Health> {
            Ok(Health { uptime_ms: self.uptime as u32, stack_free: 5000, stack_size: 27_000 })
        }
        fn fault(&mut self) -> Result<Option<FaultRecord>> {
            Ok((self.faults > 0).then(|| FaultRecord {
                kind: 3,
                count: self.faults,
                ..FaultRecord::default()
            }))
        }
        fn reboot(&mut self) -> Result<()> {
            self.reconnect()
        }
        fn reconnect(&mut self) -> Result<()> {
            self.crashed = false;
            self.seq.clear();
            self.uptime = 0;
            Ok(())
        }
    }

    #[test]
    fn a_crash_is_found_and_minimised() {
        let dir = std::env::temp_dir().join(format!("orpheus-fuzz-{}", std::process::id()));
        let mut p = Buggy { seq: Vec::new(), uptime: 1000, faults: 0, crashed: false, reply: String::new() };
        let c = Config {
            seed: 3,
            minutes: 1.0,
            max_steps: Some(2000),
            check_every: 20,
            settings: false,
            minimise: true,
            max_replays: 200,
            stack_min: 2048,
            regress_dir: dir.clone(),
        };
        let o = session(&mut p, &c).unwrap();
        let f = o.finding.expect("the planted crash is found");
        assert!(matches!(f.kind, Kind::Fault | Kind::Reset), "{f:?}");
        let key_of = |s: &Step| match s {
            Step::Tap(k, _) | Step::Hold(k, _) | Step::Press(k) => Some(*k),
            _ => None,
        };
        let m = o.minimised.unwrap();
        assert_eq!(
            m.iter().map(key_of).collect::<Vec<_>>(),
            [Some(Key::Menu), Some(Key::Down), Some(Key::Ok)]
        );
        let saved = o.saved.unwrap();
        assert_eq!(read_keys(&saved).unwrap(), m);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// On the simulator: no finding, and the guard keeps every harmful
    /// request away (power, restart, recovery, lock, format, USB mode,
    /// factory reset).
    #[test]
    fn simulator_session_is_clean_and_harmless() {
        for seed in 1..=3 {
            let mut p = SimPlayer::one_session();
            let c = Config {
                seed,
                minutes: 10.0,
                max_steps: Some(400),
                check_every: 20,
                settings: false,
                minimise: false,
                max_replays: 0,
                stack_min: 2048,
                regress_dir: std::env::temp_dir(),
            };
            let o = session(&mut p, &c).unwrap();
            assert!(o.finding.is_none(), "{:?}", o.finding);
            assert!(o.steps >= 400 && o.session.len() > 300, "{} sent of {}", o.session.len(), o.steps);
            for l in &p.sim.sys.log {
                assert!(
                    !matches!(
                        l,
                        orpheus_sim::apps::Logged::Power(_)
                            | orpheus_sim::apps::Logged::Storage(_)
                            | orpheus_sim::apps::Logged::Usb(_)
                            | orpheus_sim::apps::Logged::FactoryReset
                    ),
                    "seed {seed}: {l:?}"
                );
            }
            assert!(!p.sim.shell.gate.locked);
        }
    }

    #[test]
    fn regressions_replay_in_the_simulator() {
        // Every committed device regression runs in the simulator too.
        let dir = root().join("tests/regress/ui-device");
        for e in
            std::fs::read_dir(&dir).map(|r| r.filter_map(Result::ok).collect()).unwrap_or_else(|_| Vec::new())
        {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "keys") {
                let steps = read_keys(&p).unwrap();
                let mut sim = orpheus_sim::AppSim::new();
                for s in &steps {
                    sim.run_script(&script::step_text(s))
                        .unwrap_or_else(|e| panic!("{}: {e:#}", p.display()));
                }
            }
        }
    }
}
