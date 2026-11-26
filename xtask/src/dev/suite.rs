//! `cargo xtask dev suite`: the on-device functional suite (VER-7, 07 §5.7).
//!
//! Journeys (`tests/suite/journeys.toml`) are scripts in the simulator's
//! language. Each one runs twice: in the simulator, which gives the
//! expected frames (`tests/suite/expected/<journey>/<shot>.png`, rewritten
//! by `--update-expected`), and on the player through the developer
//! channel: keys, `expect-screen:` through `ui top`, `shot:` compared with
//! the expected frame under the masks of `tests/suite/masks.toml`, and
//! `cmd:` service checks. Every journey starts at Home (hold Back, Home's
//! focus brought to the first app by matching the screenshot) and ends
//! there. A journey may name variants: states of the player it accepts
//! (the owner's vault, the card's folders, the clock are never changed to
//! suit a test); a shot passes if it matches one variant's frame. The run
//! writes `target/orpheus/suite/<timestamp>/`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use orpheus_sim::AppSim;
use orpheus_ui::Key;
use serde::Deserialize;

use super::mask::{self, Compared, MaskFile, Masks};
use super::player::{self, Player, SimPlayer, UsbPlayer};
use super::script::{self, Item, Step};
use crate::args::Args;
use crate::util::{log, out_dir, root};

/// Where `--setup` puts the fixtures (CLAUDE.md rule 5: under `/.orpheus/`).
pub const FIXTURES: &str = "/.orpheus/test/suite";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteFile {
    /// Simulator commands run before every reference journey: the player's
    /// appearance (e.g. `theme:light`; none: Dark, Blue, English, which
    /// `ui test-look on` gives) and fixed facts of the player the simulator
    /// does not share by default (`set:dual=0`: the full-flash layout).
    #[serde(default)]
    pub appearance: String,
    pub journey: Vec<Journey>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journey {
    pub name: String,
    pub about: String,
    /// Shows card content (masked; the result depends on the card).
    #[serde(default)]
    pub card: bool,
    /// Needs the fixtures of `--setup`.
    #[serde(default)]
    pub setup: bool,
    pub script: String,
    /// States of the player the journey accepts (the first is the one the
    /// simulator dry run takes); none: the simulator's.
    #[serde(default)]
    pub variants: Vec<Variant>,
    /// Per-shot masks and tolerance, added to the screen's.
    #[serde(default)]
    pub shot: BTreeMap<String, ShotOpts>,
}

/// A state of the player a journey accepts: simulator commands (`set:…`)
/// that put the simulator there before the journey starts.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    pub name: String,
    pub sim: String,
}

impl Journey {
    /// The variants to draw: one `None` for a journey without any.
    fn variant_list(&self) -> Vec<Option<&Variant>> {
        if self.variants.is_empty() { vec![None] } else { self.variants.iter().map(Some).collect() }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShotOpts {
    #[serde(default)]
    pub rects: Vec<mask::Rect>,
    pub pixels: Option<u32>,
    pub channel: Option<u8>,
    /// `false`: ignore masks.toml for this shot.
    #[serde(default = "yes")]
    pub defaults: bool,
}

fn yes() -> bool {
    true
}

impl ShotOpts {
    fn masks(&self) -> Masks {
        Masks { rects: self.rects.clone(), pixels: self.pixels, channel: self.channel }
    }
}

pub fn suite_dir() -> PathBuf {
    root().join("tests").join("suite")
}

pub fn load() -> Result<SuiteFile> {
    let p = suite_dir().join("journeys.toml");
    let t = std::fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
    let f: SuiteFile = toml::from_str(&t).with_context(|| format!("parse {}", p.display()))?;
    let mut seen = std::collections::BTreeSet::new();
    for j in &f.journey {
        if !seen.insert(j.name.clone()) {
            bail!("journey `{}` twice", j.name);
        }
        let items = script::parse_journey(&j.script).with_context(|| format!("journey `{}`", j.name))?;
        let shots: Vec<&str> = items
            .iter()
            .filter_map(|i| match i {
                Item::Step(_, Step::Shot(n)) => Some(n.as_str()),
                _ => None,
            })
            .collect();
        for s in j.shot.keys() {
            if !shots.contains(&s.as_str()) {
                bail!("journey `{}`: options for shot `{s}`, which it does not take", j.name);
            }
        }
        let mut uniq = std::collections::BTreeSet::new();
        for s in &shots {
            if !uniq.insert(*s) {
                bail!("journey `{}`: shot `{s}` twice", j.name);
            }
        }
        let mut names = std::collections::BTreeSet::new();
        for v in &j.variants {
            let ok = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-';
            if v.name.is_empty() || !v.name.bytes().all(ok) || !names.insert(v.name.as_str()) {
                bail!("journey `{}`: variant name `{}` empty, twice, or not [a-z0-9-]", j.name, v.name);
            }
            if script::tokens(&v.sim).any(|t| t == "power" || t.contains(":power") || t.starts_with("power*"))
            {
                bail!("journey `{}`: variant `{}` presses Power", j.name, v.name);
            }
        }
    }
    Ok(f)
}

/// A frame the simulator gave for a shot (in one variant).
pub struct RefShot {
    pub name: String,
    pub variant: Option<String>,
    pub screen: String,
    pub fb: Vec<u16>,
}

fn sim_with(appearance: &str) -> Result<AppSim> {
    let mut sim = AppSim::new();
    if !appearance.trim().is_empty() {
        sim.run_script(appearance).context("appearance")?;
    }
    Ok(sim)
}

/// Run a journey in the simulator (the same prologue and epilogue as on
/// the player), once per variant: its shots, or the first command that
/// failed.
pub fn reference(appearance: &str, j: &Journey) -> Result<Vec<RefShot>> {
    let items = script::parse_journey(&j.script)?;
    let mut shots = Vec::new();
    for v in j.variant_list() {
        let what = match v {
            Some(v) => format!("journey `{}` ({}) in the simulator", j.name, v.name),
            None => format!("journey `{}` in the simulator", j.name),
        };
        let mut sim = sim_with(appearance)?;
        if let Some(v) = v {
            sim.run_script(&v.sim).with_context(|| what.clone())?;
        }
        sim.run_script("hold:back expect-screen:Home")?;
        for it in &items {
            match it {
                Item::Step(_, Step::Shot(n)) => {
                    sim.draw();
                    shots.push(RefShot {
                        name: n.clone(),
                        variant: v.map(|v| v.name.clone()),
                        screen: sim.screen().to_string(),
                        fb: sim.fb.clone(),
                    });
                }
                Item::Step(t, _) | Item::Sim(t) => {
                    sim.run_script(t).with_context(|| what.clone())?;
                }
                Item::Dev(_) | Item::Cmd { .. } => {}
            }
        }
        // As on the player: a long Back, a short one first if a dialog
        // stays open (the Authenticator's set-up note).
        let mut home = false;
        for _ in 0..3 {
            sim.run_script("hold:back")?;
            if sim.screen() == "Home" {
                home = true;
                break;
            }
            sim.run_script("back")?;
        }
        if !home {
            bail!("{what}: does not end at Home after a long Back (top is {})", sim.screen());
        }
    }
    Ok(shots)
}

/// `expected/<journey>/<shot>.png`, or `<shot>.<variant>.png`.
fn expected_path(journey: &str, shot: &str, variant: Option<&str>) -> PathBuf {
    let file = match variant {
        Some(v) => format!("{shot}.{v}.png"),
        None => format!("{shot}.png"),
    };
    suite_dir().join("expected").join(journey).join(file)
}

/// Rewrite `tests/suite/expected/` from the simulator; returns (written,
/// unchanged, removed).
pub fn update_expected(f: &SuiteFile) -> Result<(usize, usize, usize)> {
    let dir = suite_dir().join("expected");
    let mut keep = std::collections::BTreeSet::new();
    let (mut written, mut same) = (0, 0);
    for j in &f.journey {
        for s in reference(&f.appearance, j)? {
            let p = expected_path(&j.name, &s.name, s.variant.as_deref());
            keep.insert(p.clone());
            let png = mask::png_frame(&s.fb)?;
            if std::fs::read(&p).ok().as_deref() == Some(png.as_slice()) {
                same += 1;
                continue;
            }
            std::fs::create_dir_all(p.parent().unwrap_or(&dir))?;
            std::fs::write(&p, png)?;
            written += 1;
        }
    }
    let mut removed = 0;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for d in rd.filter_map(Result::ok) {
            let sub = d.path();
            for e in std::fs::read_dir(&sub)
                .map(|r| r.filter_map(Result::ok).collect())
                .unwrap_or_else(|_| Vec::new())
            {
                if !keep.contains(&e.path()) {
                    std::fs::remove_file(e.path())?;
                    removed += 1;
                }
            }
            let _ = std::fs::remove_dir(&sub);
        }
    }
    Ok((written, same, removed))
}

/// Home with the focus on each app in turn (the simulator's grid order).
pub fn home_frames(appearance: &str) -> Result<Vec<Vec<u16>>> {
    let mut sim = sim_with(appearance)?;
    let mut v: Vec<Vec<u16>> = Vec::new();
    for _ in 0..=orpheus_apps::App::COUNT {
        sim.draw();
        if v.first() == Some(&sim.fb) {
            break;
        }
        v.push(sim.fb.clone());
        sim.tap(Key::Next);
    }
    Ok(v)
}

/// Start of every journey: screen on, keys not locked, Home, focus on the
/// first app.
pub fn to_home(p: &mut dyn Player, homes: &[Vec<u16>], masks: &MaskFile) -> Result<()> {
    let t = player::top(p)?;
    if t.locked {
        bail!("the player's keys are locked: unlocking needs the Power key (by hand)");
    }
    if !t.screen_on {
        player::tap(p, Key::Back)?;
        p.sleep(300);
    }
    player::hold(p, Key::Back, script::HOLD_MS)?;
    player::expect_screen(p, "Home", 2000).context("a long Back did not return to Home")?;
    let f = player::settled_frame(p, 5)?;
    let m = masks.for_shot("Home", None, true);
    let n = homes.len();
    let cmps: Vec<Compared> = homes.iter().map(|h| mask::compare(&f, h, &m).0).collect();
    let Some(k) = cmps.iter().position(Compared::pass) else {
        let (best, c) = cmps
            .iter()
            .enumerate()
            .min_by_key(|(_, c)| c.differing)
            .ok_or_else(|| anyhow!("no Home frames"))?;
        bail!(
            "Home on the player matches none of the simulator's {n} Home frames: closest is focus {best} with {} \
             differing pixels in {:?} (x0, y0, x1, y1): appearance (test look on?), Home apps, or something playing?",
            c.differing,
            c.bbox
        );
    };
    let (key, times) = if k <= n - k { (Key::Prev, k) } else { (Key::Next, n - k) };
    for _ in 0..times {
        player::tap(p, key)?;
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct ShotResult {
    pub name: String,
    /// The variant whose frame matched (or came closest).
    pub variant: Option<String>,
    pub screen: String,
    pub player_screen: String,
    pub cmp: Compared,
}

impl ShotResult {
    pub fn pass(&self) -> bool {
        self.cmp.pass() && self.screen == self.player_screen
    }
}

#[derive(Debug, Default)]
pub struct JourneyResult {
    pub name: String,
    pub error: Option<String>,
    pub shots: Vec<ShotResult>,
    pub cmds: Vec<(String, Result<String, String>)>,
    pub seconds: f32,
    pub estimate_s: f32,
}

impl JourneyResult {
    pub fn pass(&self) -> bool {
        self.error.is_none()
            && self.shots.iter().all(ShotResult::pass)
            && self.cmds.iter().all(|c| c.1.is_ok())
    }
}

/// One expected frame of a shot: its variant, screen name and pixels.
pub struct ExpFrame {
    pub variant: Option<String>,
    pub screen: String,
    pub fb: Vec<u16>,
}

/// What a run compares against: the committed expected frames (one per
/// variant).
pub struct Expected {
    pub shots: BTreeMap<(String, String), Vec<ExpFrame>>,
}

/// Load the expected frames and check them against the simulator (a stale
/// file means the UI changed: `--update-expected` after review).
pub fn expected(f: &SuiteFile, js: &[&Journey]) -> Result<Expected> {
    let mut shots = BTreeMap::new();
    let mut stale = Vec::new();
    for j in js {
        for s in reference(&f.appearance, j)? {
            let p = expected_path(&j.name, &s.name, s.variant.as_deref());
            match mask::read_frame(&p) {
                Ok(fb) if fb == s.fb => {}
                _ => stale.push(match &s.variant {
                    Some(v) => format!("{}/{}.{v}", j.name, s.name),
                    None => format!("{}/{}", j.name, s.name),
                }),
            }
            let e = ExpFrame { variant: s.variant, screen: s.screen, fb: s.fb };
            shots.entry((j.name.clone(), s.name.clone())).or_insert_with(Vec::new).push(e);
        }
    }
    if !stale.is_empty() {
        bail!(
            "expected frames missing or stale (the simulator draws them differently): {}\n\
             review, then `cargo xtask dev suite --update-expected`",
            stale.join(", ")
        );
    }
    Ok(Expected { shots })
}

/// How long a shot that does not match yet is read again (ms): a list
/// marquee rests 1 s in every cycle (it starts 800 ms after the focus
/// moved, and the player takes longer than that from a key to the shot),
/// and the UI redraws in its 1 s loop.
pub const SHOT_RETRY_MS: u64 = 6000;

/// The expected frame `fb` matches best: the first that passes, else the
/// one with the fewest differing pixels; with the comparison and its diff.
pub fn best_match<'a>(
    fb: &[u16],
    refs: &'a [ExpFrame],
    masks: &dyn Fn(&str) -> Masks,
) -> (&'a ExpFrame, Compared, Vec<u8>) {
    let mut best: Option<(&ExpFrame, Compared, Vec<u8>)> = None;
    for e in refs {
        let (c, d) = mask::compare(fb, &e.fb, &masks(&e.screen));
        if c.pass() {
            return (e, c, d);
        }
        if best.as_ref().is_none_or(|b| c.differing < b.1.differing) {
            best = Some((e, c, d));
        }
    }
    best.unwrap_or_else(|| (&refs[0], Compared::default(), Vec::new()))
}

/// Run one journey on a player; frames go to `out/<journey>/`.
pub fn run_journey(
    p: &mut dyn Player,
    f: &SuiteFile,
    j: &Journey,
    exp: &Expected,
    masks: &MaskFile,
    homes: &[Vec<u16>],
    out: &Path,
) -> JourneyResult {
    let t0 = Instant::now();
    let mut r = JourneyResult { name: j.name.clone(), ..JourneyResult::default() };
    let items = match script::parse_journey(&j.script) {
        Ok(i) => i,
        Err(e) => {
            r.error = Some(format!("{e:#}"));
            return r;
        }
    };
    r.estimate_s = (script::estimate_ms(&items) + 2500) as f32 / 1000.0;
    p.begin_journey();
    let dir = out.join(&j.name);
    let _ = std::fs::create_dir_all(&dir);
    let body = (|| -> Result<()> {
        // The simulator player takes the first variant's state (the real
        // player is in one of them already).
        let first = j.variants.first().map_or("", |v| v.sim.as_str());
        for t in script::tokens(&f.appearance).chain(script::tokens(first)) {
            p.mirror(t)?;
        }
        to_home(p, homes, masks)?;
        for it in &items {
            match it {
                Item::Step(_, Step::Shot(n)) => {
                    let refs = exp
                        .shots
                        .get(&(j.name.clone(), n.clone()))
                        .filter(|v| !v.is_empty())
                        .ok_or_else(|| anyhow!("no expected frame for {n}"))?;
                    let opts = j.shot.get(n);
                    let m = |screen: &str| {
                        masks.for_shot(
                            screen,
                            opts.map(ShotOpts::masks).as_ref(),
                            opts.is_none_or(|o| o.defaults),
                        )
                    };
                    let mut fb = player::settled_frame(p, 5)?;
                    let (mut e, mut cmp, mut diff) = best_match(&fb, refs, &m);
                    // Not yet: read again for a while (a marquee, the 1 s
                    // loop); `waited` is the simulator's clock.
                    let t0 = Instant::now();
                    let mut waited = 0;
                    while !cmp.pass()
                        && waited < SHOT_RETRY_MS
                        && t0.elapsed().as_millis() < u128::from(SHOT_RETRY_MS)
                    {
                        p.sleep(150);
                        waited += 150;
                        let g = p.frame()?;
                        let (e2, c2, d2) = best_match(&g, refs, &m);
                        if c2.pass() || c2.differing < cmp.differing {
                            (fb, e, cmp, diff) = (g, e2, c2, d2);
                        }
                    }
                    let top = player::top(p)?;
                    std::fs::write(dir.join(format!("{n}.png")), mask::png_frame(&fb)?)?;
                    std::fs::write(dir.join(format!("{n}.expected.png")), mask::png_frame(&e.fb)?)?;
                    std::fs::write(dir.join(format!("{n}.diff.png")), mask::png_rgb(&diff)?)?;
                    r.shots.push(ShotResult {
                        name: n.clone(),
                        variant: e.variant.clone(),
                        screen: e.screen.clone(),
                        player_screen: top.name,
                        cmp,
                    });
                }
                Item::Step(_, Step::Expect(n)) | Item::Dev(Step::Expect(n)) => {
                    player::expect_screen(p, n, 2000)?;
                }
                Item::Step(_, s) | Item::Dev(s) => player::key_step(p, s)?,
                Item::Sim(t) => p.mirror(t)?,
                Item::Cmd { text, want } => {
                    let re = want.as_deref().map(regex::Regex::new).transpose()?;
                    let res = player::command(p, text, re.as_ref(), 5000).map_err(|e| format!("{e:#}"));
                    let failed = res.is_err();
                    r.cmds.push((text.clone(), res));
                    // The steps after a command rely on its result (OK on
                    // Music's first row is Shuffle all when nothing plays):
                    // stop here.
                    if failed {
                        bail!("stopped after `{text}` failed");
                    }
                }
            }
        }
        Ok(())
    })();
    if let Err(e) = body {
        r.error = Some(format!("{e:#}"));
        // A journey cut short may leave something playing (`playdir`):
        // the next one would find the mini-player on Home.
        let _ = player::command(p, "stop", None, 0);
    }
    // Back to Home whatever happened (a dialog takes a short Back first).
    let back = (|| -> Result<()> {
        for _ in 0..3 {
            player::hold(p, Key::Back, script::HOLD_MS)?;
            if player::expect_screen(p, "Home", 1500).is_ok() {
                return Ok(());
            }
            player::tap(p, Key::Back)?;
        }
        bail!("could not return to Home")
    })();
    if let Err(e) = back
        && r.error.is_none()
    {
        r.error = Some(format!("{e:#}"));
    }
    r.seconds = t0.elapsed().as_secs_f32();
    r
}

/// Length of each `--setup` tone (s): the queue must still play when the
/// nowplaying journey checks `status` (the first run's 6 s + 3 s had ended:
/// the journey took 23 s on the player).
pub const FIXTURE_SECS: f32 = 30.0;

/// The `--setup` fixtures: two tones (WAV, 44.1 kHz mono 16 bit, the rate
/// the player plays most; 2.6 MB each, about 50 s each to upload at the
/// developer channel's 50 KB/s), and the three MIDI demo songs (SYN-3:
/// `orpheus_midi::demo::SONGS`, 3–6 KB each; `orpheus-midi`'s
/// `tests/goldens-play.txt` holds the frames and CRC the player's `play`
/// reports for each with `dsp off` and `crc on`), the same bytes on every
/// run.
pub fn fixtures() -> Vec<(String, Vec<u8>)> {
    let mut v = vec![
        (format!("{FIXTURES}/music/01 Tone 440.wav"), wav_tone(440.0, FIXTURE_SECS)),
        (format!("{FIXTURES}/music/02 Tone 880.wav"), wav_tone(880.0, FIXTURE_SECS)),
    ];
    for (name, song) in orpheus_midi::demo::SONGS {
        let mut buf = vec![0u8; 64 * 1024];
        let n = song(&mut buf).unwrap_or(0);
        buf.truncate(n);
        v.push((format!("{FIXTURES}/midi/{name}"), buf));
    }
    v
}

pub fn wav_tone(hz: f32, secs: f32) -> Vec<u8> {
    let rate = 44_100u32;
    let n = (rate as f32 * secs) as u32;
    let mut v = Vec::with_capacity(44 + n as usize * 2);
    let data = n * 2;
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * 2).to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data.to_le_bytes());
    for i in 0..n {
        let s = (i as f32 / rate as f32 * hz * std::f32::consts::TAU).sin() * 8000.0;
        v.extend_from_slice(&(s as i16).to_le_bytes());
    }
    v
}

fn stamp() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (y, mo, d, h, mi, s) = super::civil(now);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// Write `summary.txt`, `summary.json` and `index.html`; returns the text.
pub fn write_report(out: &Path, player: &str, results: &[JourneyResult], notes: &[String]) -> Result<String> {
    let mut t = String::new();
    let failed = results.iter().filter(|r| !r.pass()).count();
    let _ = writeln!(t, "Orpheus on-device suite ({player}): {} journeys, {failed} failed", results.len());
    for n in notes {
        let _ = writeln!(t, "note: {n}");
    }
    let _ = writeln!(
        t,
        "\n{:<16} {:<5} {:>7} {:>7}  shots (differing/allowed, masked)",
        "journey", "", "s", "est. s"
    );
    for r in results {
        let _ = write!(
            t,
            "{:<16} {:<5} {:>7.1} {:>7.1} ",
            r.name,
            if r.pass() { "pass" } else { "FAIL" },
            r.seconds,
            r.estimate_s
        );
        for s in &r.shots {
            let _ = write!(
                t,
                " {}{}{}:{}/{},{}",
                if s.pass() { "" } else { "!" },
                s.name,
                s.variant.as_deref().map(|v| format!("@{v}")).unwrap_or_default(),
                s.cmp.differing,
                s.cmp.allowed,
                s.cmp.masked
            );
        }
        t.push('\n');
        for s in r.shots.iter().filter(|s| s.screen != s.player_screen) {
            let _ = writeln!(t, "    {}: player on {}, simulator on {}", s.name, s.player_screen, s.screen);
        }
        for s in r.shots.iter().filter(|s| !s.cmp.pass()) {
            let _ = writeln!(t, "    {}: {} pixels differ in {:?}", s.name, s.cmp.differing, s.cmp.bbox);
        }
        for (c, res) in &r.cmds {
            match res {
                Ok(reply) => {
                    let _ = writeln!(t, "    cmd `{c}`: {}", reply.lines().next().unwrap_or(""));
                }
                Err(e) => {
                    let _ = writeln!(t, "    cmd `{c}` FAILED: {e}");
                }
            }
        }
        if let Some(e) = &r.error {
            let _ = writeln!(t, "    ERROR: {e}");
        }
    }
    std::fs::write(out.join("summary.txt"), &t)?;
    let json = serde_json::json!({
        "player": player,
        "pass": failed == 0,
        "notes": notes,
        "journeys": results.iter().map(|r| serde_json::json!({
            "name": r.name,
            "pass": r.pass(),
            "seconds": r.seconds,
            "estimate_s": r.estimate_s,
            "error": r.error,
            "shots": r.shots.iter().map(|s| serde_json::json!({
                "name": s.name,
                "variant": s.variant,
                "pass": s.pass(),
                "screen": s.screen,
                "player_screen": s.player_screen,
                "differing": s.cmp.differing,
                "allowed": s.cmp.allowed,
                "masked": s.cmp.masked,
                "bbox": s.cmp.bbox,
            })).collect::<Vec<_>>(),
            "cmds": r.cmds.iter().map(|(c, res)| serde_json::json!({
                "cmd": c,
                "pass": res.is_ok(),
                "reply": match res { Ok(s) | Err(s) => s },
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    std::fs::write(out.join("summary.json"), serde_json::to_string_pretty(&json)?)?;
    let mut h = String::from(
        "<!doctype html><meta charset=utf-8><title>Orpheus device suite</title>\
         <style>body{font:14px system-ui;background:#222;color:#ddd;margin:16px}\
         img{width:256px;height:256px;image-rendering:pixelated;background:#000}\
         td{padding:4px 8px;vertical-align:top}.bad{color:#f66}.ok{color:#6c6}</style>",
    );
    let _ = write!(
        h,
        "<h1>Device suite ({player}): {} journeys, {failed} failed</h1><pre>{}</pre>",
        results.len(),
        esc(&t)
    );
    h.push_str("<table><tr><th>shot</th><th>player</th><th>simulator</th><th>diff (magenta: differs, blue: masked)</th></tr>");
    for r in results {
        for s in &r.shots {
            let (j, n) = (&r.name, &s.name);
            let _ = write!(
                h,
                "<tr><td class={}>{j}/{n}<br>{} on {}<br>{} differ (≤ {}), {} masked</td>\
                 <td><img src=\"{j}/{n}.png\"></td><td><img src=\"{j}/{n}.expected.png\"></td>\
                 <td><img src=\"{j}/{n}.diff.png\"></td></tr>",
                if s.pass() { "ok" } else { "bad" },
                s.player_screen,
                s.screen,
                s.cmp.differing,
                s.cmp.allowed,
                s.cmp.masked
            );
        }
    }
    h.push_str("</table>");
    std::fs::write(out.join("index.html"), h)?;
    Ok(t)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Upload the fixtures (`--setup`).
fn setup(p: &mut UsbPlayer) -> Result<()> {
    for (remote, data) in fixtures() {
        if !remote.starts_with(&format!("{FIXTURES}/")) {
            bail!("fixture {remote} outside {FIXTURES}");
        }
        log(&format!("   upload {remote} ({} bytes)", data.len()));
        p.upload(&remote, &data)?;
    }
    Ok(())
}

pub fn list(f: &SuiteFile) -> Result<String> {
    let mut total = 0.0;
    println!("{:<3} {:<16} {:>6}  {:<5} about", "#", "journey", "est. s", "needs");
    for (i, j) in f.journey.iter().enumerate() {
        let items = script::parse_journey(&j.script)?;
        let s = (script::estimate_ms(&items) + 2500) as f32 / 1000.0;
        total += s;
        let needs = match (j.card, j.setup) {
            (_, true) => "setup",
            (true, _) => "card",
            _ => "",
        };
        println!("{:<3} {:<16} {:>6.1}  {:<5} {}", i + 1, j.name, s, needs, j.about);
    }
    Ok(format!("{} journeys, about {:.0} s on the player (plus a reboot at the end)", f.journey.len(), total))
}

pub fn cmd(args: &mut Args) -> Result<String> {
    let f = load()?;
    if args.flag("list") {
        return list(&f);
    }
    if args.flag("update-expected") {
        let (w, s, r) = update_expected(&f)?;
        return Ok(format!(
            "expected frames: {w} written, {s} unchanged, {r} removed (tests/suite/expected/)"
        ));
    }
    let sim = args.flag("sim");
    let do_setup = args.flag("setup");
    let no_reboot = args.flag("no-reboot");
    let mut names = Vec::new();
    while let Some(n) = args.next_positional() {
        names.push(n);
    }
    let js: Vec<&Journey> = if names.is_empty() {
        f.journey.iter().collect()
    } else {
        names
            .iter()
            .map(|n| {
                f.journey
                    .iter()
                    .find(|j| &j.name == n)
                    .ok_or_else(|| anyhow!("no journey `{n}` (see --list)"))
            })
            .collect::<Result<_>>()?
    };
    let masks = MaskFile::load(&suite_dir().join("masks.toml"))?;
    let exp = expected(&f, &js)?;
    let homes = home_frames(&f.appearance)?;
    let out = out_dir().join("suite").join(stamp());
    std::fs::create_dir_all(&out)?;
    let mut usb;
    let mut simp;
    let p: &mut dyn Player = if sim {
        simp = SimPlayer::default();
        &mut simp
    } else {
        usb = UsbPlayer::open()?;
        if do_setup {
            setup(&mut usb)?;
        }
        &mut usb
    };
    let mut notes = Vec::new();
    // Baseline: uptime, fault record; nothing playing (the mini-player
    // would change Home).
    let h0 = p.health()?;
    let f0 = p.fault()?.map(|r| r.count);
    player::command(p, "stop", None, 0)?;
    p.sleep(300);
    // The simulator's appearance for this session only (the owner's
    // settings stay as they are; off again at the end, and after a reboot).
    player::test_look(p, true)?;
    // Any setting a journey changes stays in RAM (the reboot at the end
    // restores the owner's).
    if !super::fuzz::session_on(p)? {
        log("   note: this firmware has no settings test session; journeys must not change settings");
    }
    if super::fuzz::store_mounted(p) == Some(false) {
        notes.push(
            "FAIL: the player's settings store is not mounted (store=none): it runs on defaults; restart it and run again"
                .into(),
        );
    }
    let mut results = Vec::new();
    for j in &js {
        log(&format!("== suite/{}", j.name));
        let r = run_journey(p, &f, j, &exp, &masks, &homes, &out);
        log(&format!(
            "   {} in {:.1} s{}",
            if r.pass() { "pass" } else { "FAIL" },
            r.seconds,
            r.error.as_deref().map(|e| format!(": {e}")).unwrap_or_default()
        ));
        let fatal = r
            .error
            .as_deref()
            .is_some_and(|e| e.contains("could not return to Home") || e.contains("locked"));
        results.push(r);
        if fatal {
            notes.push("suite stopped: the player is not at Home".into());
            break;
        }
    }
    // Idle Home must not thrash the code cache (2026-11-25: a layout where
    // the settings lookup and its closure shared a line made idle Home
    // load 2,939 lines/s, 1.7 s per UI loop; a healthy build ≈ 730/s).
    if let Some(note) = idle_home_check(p) {
        notes.push(note);
    }
    // Oracles over the whole run: no reset, no new fault record.
    match (p.health(), p.fault()) {
        (Ok(h1), Ok(f1)) => {
            if h1.uptime_ms < h0.uptime_ms {
                notes.push(format!(
                    "FAIL: the player reset during the suite (uptime {} -> {} ms)",
                    h0.uptime_ms, h1.uptime_ms
                ));
            }
            if let Some(rec) = f1.filter(|r| Some(r.count) != f0) {
                notes.push(format!(
                    "FAIL: new fault record: kind {:#x} pc {:#010x} (count {})",
                    rec.kind, rec.pc, rec.count
                ));
            }
            notes.push(format!("stack free {} of {} B", h1.stack_free, h1.stack_size));
        }
        (Err(e), _) | (_, Err(e)) => notes.push(format!("FAIL: player not answering after the suite: {e:#}")),
    }
    if let Err(e) = player::test_look(p, false) {
        notes.push(format!("test look not switched off: {e:#} (the reboot clears it)"));
    }
    if !no_reboot {
        // CLAUDE.md rule 4: every test ends in a reboot.
        p.reboot()?;
    }
    let text = write_report(&out, if sim { "simulator" } else { "player" }, &results, &notes)?;
    print!("{text}");
    let failed =
        results.iter().filter(|r| !r.pass()).count() + notes.iter().filter(|n| n.starts_with("FAIL")).count();
    if failed > 0 || results.len() < js.len() {
        bail!("{failed} failed; report in {}", out.display());
    }
    Ok(format!("{} journeys passed; report in {}", results.len(), out.display()))
}

/// Line loads per second the idle Home screen may cost (a healthy build
/// measured ≈ 730/s, the thrashing one 2,939/s).
const IDLE_LOADS_MAX_PER_S: u32 = 1_500;

/// The verdict on two `(uptime ms, line loads)` samples of idle Home.
fn idle_verdict(a: (u32, u32), b: (u32, u32)) -> String {
    let ms = b.0.saturating_sub(a.0).max(1);
    let rate = u64::from(b.1.wrapping_sub(a.1)) * 1000 / u64::from(ms);
    if rate > u64::from(IDLE_LOADS_MAX_PER_S) {
        format!(
            "FAIL: idle Home loads {rate} code lines/s (at most {IDLE_LOADS_MAX_PER_S}): the code cache thrashes (profile with `dev prof --faults 15`)"
        )
    } else {
        format!("idle Home: {rate} line loads/s")
    }
}

/// Sample the pager over 5 s of idle Home (the player only).
fn idle_home_check(p: &mut dyn Player) -> Option<String> {
    let a = p.pager_loads().ok().flatten()?;
    std::thread::sleep(Duration::from_secs(5));
    let b = p.pager_loads().ok().flatten()?;
    Some(idle_verdict(a, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The figures of 2026-11-25: the thrashing build (idle Home 2,939
    /// line loads/s, keys 3.3 s each) fails, the healthy one passes.
    #[test]
    fn regress_idle_home_thrash_fails_the_suite() {
        assert!(idle_verdict((27_512, 84_697), (43_400, 131_405)).starts_with("FAIL"));
        assert!(!idle_verdict((22_316, 16_327), (32_441, 23_673)).starts_with("FAIL"));
        assert!(!idle_verdict((0, u32::MAX - 10), (5_000, 100)).starts_with("FAIL"));
    }

    #[test]
    fn journeys_are_device_safe_and_end_at_home() {
        let f = load().unwrap();
        assert!(f.journey.len() >= 13);
        for j in &f.journey {
            // Parses as device steps (no Power, no simulator-only commands
            // outside `sim:` lines), runs in the simulator with its
            // `expect-screen:` checks, ends at Home after a long Back.
            let shots = reference(&f.appearance, j).unwrap_or_else(|e| panic!("{}: {e:#}", j.name));
            assert!(!shots.is_empty(), "{} takes no shot", j.name);
        }
    }

    /// `ORPHEUS_BLESS=1` rewrites them (as `--update-expected`, without
    /// the device lock).
    #[test]
    fn expected_frames_are_current() {
        let f = load().unwrap();
        if std::env::var("ORPHEUS_BLESS").is_ok_and(|v| !v.is_empty() && v != "0") {
            update_expected(&f).unwrap();
        }
        let js: Vec<&Journey> = f.journey.iter().collect();
        expected(&f, &js).unwrap();
    }

    /// The whole runner against the simulator as the player: every journey
    /// passes, shots equal their expected frames exactly, and the report is
    /// written.
    #[test]
    fn suite_passes_on_the_simulator() {
        let f = load().unwrap();
        let js: Vec<&Journey> = f.journey.iter().collect();
        let exp = expected(&f, &js).unwrap();
        let masks = MaskFile::load(&suite_dir().join("masks.toml")).unwrap();
        let homes = home_frames(&f.appearance).unwrap();
        let out = std::env::temp_dir().join(format!("orpheus-suite-{}", std::process::id()));
        let mut p = SimPlayer::default();
        let mut results = Vec::new();
        for j in &js {
            let r = run_journey(&mut p, &f, j, &exp, &masks, &homes, &out);
            assert!(r.pass(), "{}: {:?}", j.name, r);
            for s in &r.shots {
                assert_eq!(s.cmp.differing, 0, "{}/{}", j.name, s.name);
            }
            results.push(r);
        }
        let t = write_report(&out, "simulator", &results, &[]).unwrap();
        assert!(t.contains(", 0 failed"));
        assert!(out.join("index.html").exists() && out.join("summary.json").exists());
        assert!(out.join(&js[0].name).join(format!("{}.diff.png", results[0].shots[0].name)).exists());
        let _ = std::fs::remove_dir_all(&out);
    }

    /// Home's focus is found from the screenshot and brought back to the
    /// first app; a wrong appearance is reported.
    #[test]
    fn home_focus_is_normalised() {
        let masks = MaskFile::load(&suite_dir().join("masks.toml")).unwrap();
        let homes = home_frames("").unwrap();
        // One frame per app on Home: the player's (Games since GAME-2; not
        // Books, which the simulator has but does not show by default).
        let on_home = orpheus_apps::App::ALL.iter().filter(|a| a.on_player()).count();
        assert_eq!(homes.len(), on_home, "one frame per app (Games since GAME-2)");
        let mut p = SimPlayer::one_session();
        for _ in 0..7 {
            player::tap(&mut p, Key::Next).unwrap();
        }
        to_home(&mut p, &homes, &masks).unwrap();
        assert_eq!(p.frame().unwrap(), homes[0]);
        p.sim.run_script("theme:light").unwrap();
        assert!(to_home(&mut p, &homes, &masks).unwrap_err().to_string().contains("appearance"));
    }

    #[test]
    fn a_failing_shot_is_reported() {
        let r = JourneyResult {
            name: "x".into(),
            shots: vec![ShotResult {
                name: "s".into(),
                variant: Some("v".into()),
                screen: "Home".into(),
                player_screen: "Home".into(),
                cmp: Compared { differing: 3, masked: 10, bbox: Some((1, 2, 3, 4)), allowed: 0 },
            }],
            cmds: vec![("status".into(), Err("no match".into()))],
            ..JourneyResult::default()
        };
        assert!(!r.pass());
        let out = std::env::temp_dir().join(format!("orpheus-suite-r-{}", std::process::id()));
        std::fs::create_dir_all(&out).unwrap();
        let t = write_report(&out, "player", &[r], &[]).unwrap();
        assert!(t.contains("1 failed") && t.contains("3 pixels differ") && t.contains("FAILED: no match"));
        assert!(t.contains("!s@v:3/0,10"), "{t}");
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn list_estimates_every_journey() {
        let f = load().unwrap();
        let s = list(&f).unwrap();
        assert!(s.starts_with(&format!("{} journeys", f.journey.len())), "{s}");
    }

    #[test]
    fn tone_fixture_is_a_wav() {
        let w = wav_tone(440.0, 0.5);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(w.len(), 44 + 22_050 * 2);
        for (p, data) in fixtures() {
            assert!(p.starts_with("/.orpheus/test/suite/"));
            if p.contains("/midi/") {
                // A Standard MIDI File (SYN-3's fixtures).
                assert_eq!(&data[..4], b"MThd", "{p}");
                continue;
            }
            // Long enough to still play at the journey's `status` check.
            assert_eq!(data.len(), 44 + 44_100 * 30 * 2);
        }
        assert_eq!(fixtures()[0].1, fixtures()[0].1, "deterministic");
    }

    /// A shot is compared with every variant's frame: the player's state
    /// picks one (here the Authenticator with each vault state).
    #[test]
    fn a_shot_matches_the_variant_the_player_is_in() {
        let f = load().unwrap();
        let j = f.journey.iter().find(|j| j.name == "authenticator").unwrap();
        assert!(j.variants.len() >= 3);
        let masks = MaskFile::load(&suite_dir().join("masks.toml")).unwrap();
        let shots = reference(&f.appearance, j).unwrap();
        let refs: Vec<ExpFrame> = shots
            .iter()
            .map(|s| ExpFrame { variant: s.variant.clone(), screen: s.screen.clone(), fb: s.fb.clone() })
            .collect();
        let m = |screen: &str| masks.for_shot(screen, None, true);
        for s in &shots {
            let (e, c, _) = best_match(&s.fb, &refs, &m);
            assert!(c.pass());
            assert_eq!(e.variant, s.variant);
        }
        // A frame that is none of them is reported against the closest.
        let (_, c, _) = best_match(&vec![0xFFFF; mask::W * mask::H], &refs, &m);
        assert!(!c.pass() && c.differing > 0);
    }
}
