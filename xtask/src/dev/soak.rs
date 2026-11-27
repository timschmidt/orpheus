//! `cargo xtask dev soak` (VER-12, 07 §5.10): long mixed-use soaks.
//!
//! A scenario is a seeded loop of weighted actions (playback through test
//! commands, Now Playing keys, browsing through the fuzzer's guard, the
//! radio, recordings under `/.orpheus/`, session-only setting changes, card
//! hand-over and USB personality switches). Every few seconds, and at the
//! end, the oracles read the player: fault record, uptime, stack high-water,
//! underruns by cause, pager loads and evictions per second, the DAC's
//! headroom low-water, card and pager errors, played samples against the
//! host's clock. After the run: the card's root listing, the settings store
//! (CRC at the boot before and after) and the queue. A failing run is
//! replayed and reduced to a minimal action sequence in
//! `tests/regress/soak/`; `--replay FILE` runs one again.
//!
//! Safety (CLAUDE.md rules 4–7): never the Power key; keys go through the
//! fuzzer's guard; settings change only inside a test session (`settings
//! test-session on`: RAM only, the reboot that ends every run restores the
//! stored values); recordings go to `/.orpheus/test/rec/soak/`; the card
//! is lent read-only; every run starts and ends with a reboot.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use orpheus_ui::Key;

use super::fuzz::{self, Exec, Locked, Rng};
use super::mask::{self, Masks};
use super::player::{self, Player, SimPlayer, UsbPlayer};
use super::script::{self, Step};
use crate::args::{Args, num};
use crate::util::{log, out_dir, root};

/// Where the soak's own fixtures go (`--setup`; CLAUDE.md rule 5).
pub const FIXTURES: &str = "/.orpheus/test/soak";
/// The recorder's developer folder plus the soak's sub-folder.
pub const REC_DIR: &str = "/.orpheus/test/rec/soak";
/// Frames per DAC DMA half (the `played=` unit of `status`).
pub const HALF_FRAMES: u64 = 1536;

// ------------------------------------------------------------- budgets

/// `budgets.toml [hil]` and `[soak]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Budget {
    pub stack_free_min: u32,
    pub sample_every_s: u64,
    pub loads_per_s_max: f64,
    pub loads_per_s_mean_max: f64,
    pub evictions_per_s_max: f64,
    pub thrash_samples: u32,
    pub seek_window_s: u64,
    pub underruns_per_disruption_max: u64,
    pub head_min_frames: i64,
    pub card_errors_max: u32,
    pub load_errors_max: u32,
    pub drift_ppm_max: f64,
    pub drift_min_window_s: u64,
    pub clock_ppm_max: f64,
    pub battery_min_mv: u16,
    pub max_recordings: u32,
}

impl Budget {
    pub fn parse(text: &str) -> Result<Self> {
        let t: toml::Table = toml::from_str(text)?;
        let hil =
            t.get("hil").and_then(toml::Value::as_table).ok_or_else(|| anyhow!("budgets.toml: no [hil]"))?;
        let s = t
            .get("soak")
            .and_then(toml::Value::as_table)
            .ok_or_else(|| anyhow!("budgets.toml: no [soak]"))?;
        let int = |tb: &toml::Table, k: &str| -> Result<i64> {
            tb.get(k).and_then(toml::Value::as_integer).ok_or_else(|| anyhow!("budgets.toml: `{k}` missing"))
        };
        let flt = |k: &str| -> Result<f64> {
            s.get(k)
                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                .ok_or_else(|| anyhow!("budgets.toml: [soak] `{k}` missing"))
        };
        let u = |k: &str| -> Result<u64> { Ok(u64::try_from(int(s, k)?)?) };
        Ok(Budget {
            stack_free_min: u32::try_from(int(hil, "stack_free_min_bytes")?)?,
            sample_every_s: u("sample_every_s")?.max(1),
            loads_per_s_max: flt("loads_per_s_max")?,
            loads_per_s_mean_max: flt("loads_per_s_mean_max")?,
            evictions_per_s_max: flt("evictions_per_s_max")?,
            thrash_samples: u32::try_from(u("thrash_samples")?)?.max(1),
            seek_window_s: u("seek_window_s")?,
            underruns_per_disruption_max: u("underruns_per_disruption_max")?,
            head_min_frames: int(s, "head_min_frames")?,
            card_errors_max: u32::try_from(u("card_errors_max")?)?,
            load_errors_max: u32::try_from(u("pager_load_errors_max")?)?,
            drift_ppm_max: flt("drift_ppm_max")?,
            drift_min_window_s: u("drift_min_window_s")?,
            clock_ppm_max: flt("clock_ppm_max")?,
            battery_min_mv: u16::try_from(u("battery_min_mv")?)?,
            max_recordings: u32::try_from(u("max_recordings")?)?,
        })
    }

    pub fn load() -> Result<Self> {
        Self::parse(&std::fs::read_to_string(root().join("budgets.toml"))?)
    }
}

// ----------------------------------------------------------- scenarios

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    Music,
    Radio,
    Browse,
    Settings,
    Mixed,
    Stress,
}

impl Scenario {
    pub const ALL: [Scenario; 6] = [
        Scenario::Music,
        Scenario::Radio,
        Scenario::Browse,
        Scenario::Settings,
        Scenario::Mixed,
        Scenario::Stress,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Scenario::Music => "music",
            Scenario::Radio => "radio",
            Scenario::Browse => "browse-while-playing",
            Scenario::Settings => "settings-churn",
            Scenario::Mixed => "mixed",
            Scenario::Stress => "stress",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|x| x.name() == s || (s == "browse" && *x == Scenario::Browse))
    }

    /// 07 §5.10: 2 h each, `mixed` 8 h (and 24 h), `stress` 1 h.
    pub fn default_minutes(self) -> f64 {
        match self {
            Scenario::Mixed => 480.0,
            Scenario::Stress => 60.0,
            _ => 120.0,
        }
    }
}

/// Where a browsing action happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum App {
    Files,
    Library,
    Pictures,
    Settings,
    NowPlaying,
    /// The Games list (M14): random keys open and play the games (Blocks
    /// runs its 30 frames/s loop over the music); Back pauses and saves to
    /// `/.orpheus/games/`. Only as an explicit `browse games` step: the
    /// random scenarios' picks are unchanged (their seeds replay as before).
    Games,
}

impl App {
    fn name(self) -> &'static str {
        match self {
            App::Files => "files",
            App::Library => "library",
            App::Pictures => "pictures",
            App::Settings => "settings",
            App::NowPlaying => "nowplaying",
            App::Games => "games",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        [App::Files, App::Library, App::Pictures, App::Settings, App::NowPlaying, App::Games]
            .into_iter()
            .find(|a| a.name() == s)
    }
    /// Home position (the player's grid: `App::GRID` without the apps not
    /// on the player, such as the Sequencer, which the workspace's
    /// `orpheus-apps` has through the simulator; Games and Books before
    /// Settings since GAME-2 and BOOK-1/2) and screen name.
    fn home(self) -> (usize, &'static str) {
        let at = |a: orpheus_apps::App| {
            orpheus_apps::App::GRID.iter().filter(|g| g.on_player()).position(|&g| g == a).unwrap_or(0)
        };
        match self {
            App::Files => (at(orpheus_apps::App::Files), "Files"),
            App::Library | App::NowPlaying => (at(orpheus_apps::App::Music), "Music"),
            App::Pictures => (at(orpheus_apps::App::Pictures), "Pictures"),
            App::Settings => (at(orpheus_apps::App::Settings), "Settings"),
            App::Games => (at(orpheus_apps::App::Games), "Games"),
        }
    }
}

/// One step of a soak (a line of a `.soak` file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// `playdir PATH`: a folder queue (folder order).
    PlayDir(String),
    /// Now Playing: hold Next (forward) or Prev for `ms`.
    Seek {
        fwd: bool,
        ms: u64,
    },
    /// Now Playing: Prev once more than 3 s in (back to the start).
    SeekStart,
    /// Now Playing: OK, `ms`, OK.
    Pause(u64),
    /// Now Playing: `n` quick Next (or Prev) taps.
    Skip {
        fwd: bool,
        n: u32,
    },
    /// `n` × Vol+, then `n` × Vol− (never ends louder).
    Volume(u32),
    Dsp(bool),
    /// `sound g0 g1 g2 g3 g4 bass crossfeed`.
    Eq([i8; 7]),
    /// Now Playing: `n` × Down (its views; a setting, session only).
    NpView(u32),
    /// Untouched listening (seconds).
    Listen(u64),
    /// Nothing playing, screen off (`ui screen-off`), seconds.
    Idle(u64),
    /// Guarded random keys in an app.
    Browse {
        app: App,
        steps: u32,
        seed: u64,
    },
    /// Menus of the current screen.
    Menus {
        steps: u32,
        seed: u64,
    },
    RadioOn(Option<u32>),
    RadioSeek(bool),
    RadioScan,
    RadioOff,
    /// The Radio app's keys (steps, presets, mute pairs, seeks).
    RadioKeys {
        steps: u32,
        seed: u64,
    },
    /// A short recording (seconds) into `REC_DIR`.
    Record(u32),
    /// `settings test-set ID VALUE` (session only).
    Set {
        id: u16,
        value: i16,
    },
    /// The owner's look off, these settings, in-place keys, listening,
    /// the test look back on.
    Look {
        values: Vec<(u16, i16)>,
        steps: u32,
        seed: u64,
        secs: u64,
    },
    /// `Some(48)`: governor off, clock pinned; `None`: governor on.
    Clock(Option<u32>),
    /// The card to the computer (read-only card reader) for `secs`.
    Handover(u64),
    /// The keyboard personality for `secs`.
    Personality(u64),
}

impl Action {
    pub fn kind(&self) -> &'static str {
        match self {
            Action::PlayDir(_) => "playdir",
            Action::Seek { .. } => "seek",
            Action::SeekStart => "seek-start",
            Action::Pause(_) => "pause",
            Action::Skip { .. } => "skip",
            Action::Volume(_) => "volume",
            Action::Dsp(_) => "dsp",
            Action::Eq(_) => "eq",
            Action::NpView(_) => "np-view",
            Action::Listen(_) => "listen",
            Action::Idle(_) => "idle",
            Action::Browse { .. } => "browse",
            Action::Menus { .. } => "menus",
            Action::RadioOn(_) => "radio-on",
            Action::RadioSeek(_) => "radio-seek",
            Action::RadioScan => "radio-scan",
            Action::RadioOff => "radio-off",
            Action::RadioKeys { .. } => "radio-keys",
            Action::Record(_) => "record",
            Action::Set { .. } => "set",
            Action::Look { .. } => "look",
            Action::Clock(_) => "clock",
            Action::Handover(_) => "handover",
            Action::Personality(_) => "personality",
        }
    }

    pub fn text(&self) -> String {
        let dir = |f: bool| if f { "next" } else { "prev" };
        match self {
            Action::PlayDir(p) => format!("playdir {p}"),
            Action::Seek { fwd, ms } => format!("seek {} {ms}", dir(*fwd)),
            Action::SeekStart => "seek-start".into(),
            Action::Pause(ms) => format!("pause {ms}"),
            Action::Skip { fwd, n } => format!("skip {} {n}", dir(*fwd)),
            Action::Volume(n) => format!("volume {n}"),
            Action::Dsp(on) => format!("dsp {}", if *on { "on" } else { "off" }),
            Action::Eq(v) => {
                format!("eq {}", v.iter().map(ToString::to_string).collect::<Vec<_>>().join(" "))
            }
            Action::NpView(n) => format!("np-view {n}"),
            Action::Listen(s) => format!("listen {s}"),
            Action::Idle(s) => format!("idle {s}"),
            Action::Browse { app, steps, seed } => format!("browse {} {steps} {seed}", app.name()),
            Action::Menus { steps, seed } => format!("menus {steps} {seed}"),
            Action::RadioOn(None) => "radio-on".into(),
            Action::RadioOn(Some(k)) => format!("radio-on {k}"),
            Action::RadioSeek(up) => format!("radio-seek {}", if *up { "up" } else { "down" }),
            Action::RadioScan => "radio-scan".into(),
            Action::RadioOff => "radio-off".into(),
            Action::RadioKeys { steps, seed } => format!("radio-keys {steps} {seed}"),
            Action::Record(s) => format!("record {s}"),
            Action::Set { id, value } => format!("set {id:#06x} {value}"),
            Action::Look { values, steps, seed, secs } => format!(
                "look {} {steps} {seed} {secs}",
                values.iter().map(|(i, v)| format!("{i:#06x}={v}")).collect::<Vec<_>>().join(",")
            ),
            Action::Clock(Some(l)) => format!("clock {l}"),
            Action::Clock(None) => "clock gov".into(),
            Action::Handover(s) => format!("handover {s}"),
            Action::Personality(s) => format!("personality {s}"),
        }
    }

    pub fn parse(line: &str) -> Result<Action> {
        let line = line.trim();
        let (head, rest) = line.split_once(' ').unwrap_or((line, ""));
        let w: Vec<&str> = rest.split_whitespace().collect();
        let n = |i: usize| -> Result<u64> {
            w.get(i)
                .ok_or_else(|| anyhow!("`{line}`: argument {} missing", i + 1))?
                .parse::<u64>()
                .map_err(|e| anyhow!("`{line}`: {e}"))
        };
        let n32 = |i: usize| -> Result<u32> { Ok(u32::try_from(n(i)?)?) };
        let dir = |i: usize| -> Result<bool> {
            match w.get(i).copied() {
                Some("next" | "up") => Ok(true),
                Some("prev" | "down") => Ok(false),
                _ => bail!("`{line}`: next/prev (or up/down)"),
            }
        };
        let id = |s: &str| -> Result<u16> {
            Ok(match s.strip_prefix("0x") {
                Some(h) => u16::from_str_radix(h, 16)?,
                None => s.parse()?,
            })
        };
        Ok(match head {
            "playdir" if !rest.trim().is_empty() => Action::PlayDir(rest.trim().to_string()),
            "seek" => Action::Seek { fwd: dir(0)?, ms: n(1)? },
            "seek-start" => Action::SeekStart,
            "pause" => Action::Pause(n(0)?),
            "skip" => Action::Skip { fwd: dir(0)?, n: n32(1)? },
            "volume" => Action::Volume(n32(0)?),
            "dsp" => Action::Dsp(w.first() == Some(&"on")),
            "eq" => {
                let v: Vec<i8> = w.iter().map(|x| x.parse()).collect::<Result<_, _>>()?;
                Action::Eq(v.try_into().map_err(|_| anyhow!("`{line}`: seven numbers"))?)
            }
            "np-view" => Action::NpView(n32(0)?),
            "listen" => Action::Listen(n(0)?),
            "idle" => Action::Idle(n(0)?),
            "browse" => Action::Browse {
                app: w.first().and_then(|a| App::parse(a)).ok_or_else(|| anyhow!("`{line}`: app"))?,
                steps: n32(1)?,
                seed: n(2)?,
            },
            "menus" => Action::Menus { steps: n32(0)?, seed: n(1)? },
            "radio-on" => Action::RadioOn(if w.is_empty() { None } else { Some(n32(0)?) }),
            "radio-seek" => Action::RadioSeek(dir(0)?),
            "radio-scan" => Action::RadioScan,
            "radio-off" => Action::RadioOff,
            "radio-keys" => Action::RadioKeys { steps: n32(0)?, seed: n(1)? },
            "record" => Action::Record(n32(0)?),
            "set" => Action::Set {
                id: id(w.first().ok_or_else(|| anyhow!("`{line}`: id"))?)?,
                value: w.get(1).ok_or_else(|| anyhow!("`{line}`: value"))?.parse()?,
            },
            "look" => {
                let values = w
                    .first()
                    .ok_or_else(|| anyhow!("`{line}`: values"))?
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|kv| {
                        let (k, v) = kv.split_once('=').ok_or_else(|| anyhow!("`{kv}`: ID=VALUE"))?;
                        Ok((id(k)?, v.parse()?))
                    })
                    .collect::<Result<Vec<_>>>()?;
                Action::Look { values, steps: n32(1)?, seed: n(2)?, secs: n(3)? }
            }
            "clock" if w.first() == Some(&"gov") => Action::Clock(None),
            "clock" => Action::Clock(Some(n32(0)?)),
            "handover" => Action::Handover(n(0)?),
            "personality" => Action::Personality(n(0)?),
            _ => bail!("unknown soak action `{line}`"),
        })
    }

    /// Underruns are allowed for a while after it (seeks, track and source
    /// changes, card hand-over, clock changes).
    pub fn disrupts(&self) -> bool {
        match self {
            Action::PlayDir(_)
            | Action::Seek { .. }
            | Action::SeekStart
            | Action::Pause(_)
            | Action::Skip { .. }
            | Action::Idle(_)
            | Action::RadioOn(_)
            | Action::RadioSeek(_)
            | Action::RadioScan
            | Action::RadioOff
            | Action::RadioKeys { .. }
            | Action::Record(_)
            | Action::Clock(_)
            | Action::Handover(_)
            | Action::Personality(_) => true,
            // Shuffle and repeat re-queue.
            Action::Set { id, .. } => matches!(id, 0x0201 | 0x0202),
            _ => false,
        }
    }

    /// Long actions sample while they run (seconds they take at least).
    fn seconds(&self) -> u64 {
        match self {
            Action::Listen(s) | Action::Idle(s) | Action::Handover(s) | Action::Personality(s) => *s,
            Action::Look { secs, .. } => *secs,
            Action::Record(s) => u64::from(*s),
            _ => 0,
        }
    }
}

/// `settings test-set` accepts these (the firmware's `session_settable`,
/// fw/firmware/src/ui.rs; a host test keeps the generator inside it).
pub fn session_settable(id: u16) -> bool {
    (matches!(id >> 8, 0x01 | 0x02 | 0x03 | 0x0A) && id != 0x030E)
        || matches!(id, 0x0D01..=0x0D03 | 0x1001..=0x100D)
}

/// The values a setting takes (`orpheus_apps::settings`).
pub fn valid_values(id: u16) -> Vec<i16> {
    use orpheus_apps::settings::{Kind, SettingId, def};
    match def(SettingId(id)).map(|d| d.kind) {
        Some(Kind::Toggle) => vec![0, 1],
        Some(Kind::Choice(c)) => (0..i16::try_from(c.len()).unwrap_or(1)).collect(),
        Some(Kind::Range { min, max, step, .. }) => {
            (min..=max).step_by(usize::try_from(step.max(1)).unwrap_or(1)).collect()
        }
        // English, Russian, Japanese (the packs the simulator has).
        Some(Kind::Language) => vec![0, 0x7275, 0x6A61],
        _ => Vec::new(),
    }
}

const SOUND_IDS: [u16; 6] = [0x0101, 0x0102, 0x0103, 0x0104, 0x0105, 0x0106];
const PLAYBACK_IDS: [u16; 4] = [0x0201, 0x0202, 0x0203, 0x0205];
const DISPLAY_IDS: [u16; 3] = [0x030D, 0x0308, 0x030A];
/// Settings › Display › List roll-around (UI-8).
const LIST_ROLL: u16 = 0x030C;
/// Theme, accent, language, list roll-around, clock in the bar.
const LOOK_IDS: [u16; 5] = [0x0304, 0x0305, 0x0A01, 0x030C, 0x030A];

/// What the generator picks from (weights per scenario: [`weights`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    PlayDir,
    Shuffle,
    Repeat,
    Seek,
    SeekBack,
    SeekStart,
    SeekEnd,
    Pause,
    Skip,
    Volume,
    Dsp,
    Eq,
    NpView,
    Listen,
    LongListen,
    Idle,
    BrowseFiles,
    BrowseLibrary,
    BrowsePictures,
    BrowseSettings,
    BrowseNowPlaying,
    Menus,
    RadioOn,
    RadioSeek,
    RadioScan,
    RadioOff,
    RadioKeys,
    Record,
    SetSound,
    SetPlayback,
    SetDisplay,
    Look,
    Handover,
    Personality,
}

/// Weights of 07 §5.10 as built (the documentation prints this table).
pub fn weights(s: Scenario) -> &'static [(Pick, u32)] {
    use Pick::*;
    match s {
        Scenario::Music => &[
            (PlayDir, 8),
            (Shuffle, 3),
            (Repeat, 1),
            (Seek, 8),
            (SeekBack, 5),
            (SeekStart, 3),
            (SeekEnd, 2),
            (Pause, 5),
            (Skip, 5),
            (Volume, 4),
            (Dsp, 3),
            (Eq, 5),
            (NpView, 3),
            (Listen, 20),
        ],
        Scenario::Radio => &[
            (RadioOn, 8),
            (RadioSeek, 8),
            (RadioScan, 2),
            (RadioKeys, 10),
            (RadioOff, 4),
            (PlayDir, 6),
            (Record, 3),
            (Volume, 3),
            (Listen, 20),
        ],
        Scenario::Browse => &[
            (PlayDir, 4),
            (BrowseFiles, 20),
            (BrowseLibrary, 15),
            (BrowsePictures, 10),
            (BrowseNowPlaying, 6),
            (BrowseSettings, 4),
            (Menus, 8),
            (Seek, 3),
            (Skip, 3),
            (Listen, 8),
        ],
        Scenario::Settings => &[
            (Look, 15),
            (SetSound, 15),
            (SetPlayback, 6),
            (SetDisplay, 6),
            (Dsp, 3),
            (Eq, 3),
            (BrowseFiles, 4),
            (PlayDir, 4),
            (Listen, 25),
        ],
        Scenario::Mixed => &[
            (PlayDir, 6),
            (Shuffle, 2),
            (Seek, 5),
            (SeekBack, 3),
            (SeekStart, 2),
            (SeekEnd, 1),
            (Pause, 3),
            (Skip, 3),
            (Volume, 3),
            (Dsp, 2),
            (Eq, 3),
            (NpView, 2),
            (Listen, 10),
            (LongListen, 6),
            (Idle, 3),
            (BrowseFiles, 6),
            (BrowseLibrary, 5),
            (BrowsePictures, 4),
            (BrowseSettings, 2),
            (BrowseNowPlaying, 2),
            (Menus, 3),
            (RadioOn, 3),
            (RadioSeek, 2),
            (RadioScan, 1),
            (RadioKeys, 3),
            (RadioOff, 2),
            (Record, 1),
            (SetSound, 3),
            (SetPlayback, 1),
            (SetDisplay, 1),
            (Look, 3),
            (Handover, 1),
            (Personality, 1),
        ],
        Scenario::Stress => &[
            (PlayDir, 4),
            (Seek, 6),
            (SeekBack, 4),
            (Skip, 6),
            (Pause, 2),
            (Volume, 3),
            (Eq, 3),
            (Dsp, 2),
            (NpView, 3),
            (BrowseFiles, 12),
            (BrowseLibrary, 10),
            (BrowsePictures, 8),
            (BrowseNowPlaying, 4),
            (BrowseSettings, 3),
            (Menus, 6),
            (RadioKeys, 2),
            (SetSound, 3),
            (Look, 3),
            (Listen, 4),
        ],
    }
}

/// The seeded generator: same scenario, seed and folders, same actions.
pub struct Plan {
    pub scenario: Scenario,
    rng: Rng,
    folders: Vec<String>,
    started: bool,
}

impl Plan {
    pub fn new(scenario: Scenario, seed: u64, folders: Vec<String>) -> Self {
        Self { scenario, rng: Rng::new(seed ^ 0x50AC_0000), folders, started: false }
    }

    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.rng.next() % (hi - lo + 1)
    }

    fn pick_value(&mut self, ids: &[u16]) -> Action {
        let id = ids[self.rng.below(ids.len())];
        let v = valid_values(id);
        let value = v.get(self.rng.below(v.len())).copied().unwrap_or(0);
        Action::Set { id, value }
    }

    pub fn next(&mut self) -> Action {
        let stress = self.scenario == Scenario::Stress;
        if !self.started {
            self.started = true;
            // The stress run pins the clock at its lowest level first.
            if stress {
                return Action::Clock(Some(48));
            }
        }
        let w = weights(self.scenario);
        let total: u32 = w.iter().map(|x| x.1).sum();
        let mut r = (self.rng.next() % u64::from(total)) as u32;
        let mut pick = w[0].0;
        for &(p, n) in w {
            if r < n {
                pick = p;
                break;
            }
            r -= n;
        }
        let seed = self.rng.next() & 0xFFFF_FFFF;
        let steps = |s: &mut Self, lo: u64, hi: u64| -> u32 {
            let (lo, hi) = if stress { (lo * 2, hi * 3) } else { (lo, hi) };
            u32::try_from(s.range(lo, hi)).unwrap_or(u32::MAX)
        };
        let listen = |s: &mut Self, lo: u64, hi: u64| if stress { s.range(5, 20) } else { s.range(lo, hi) };
        match pick {
            Pick::PlayDir if self.folders.is_empty() => Action::Listen(30),
            Pick::PlayDir => Action::PlayDir(self.folders[self.rng.below(self.folders.len())].clone()),
            Pick::Shuffle => Action::Set { id: 0x0201, value: i16::from(self.rng.next() & 1 == 0) },
            Pick::Repeat => Action::Set { id: 0x0202, value: (self.rng.next() % 3) as i16 },
            Pick::Seek => Action::Seek { fwd: true, ms: self.range(700, if stress { 1500 } else { 3000 }) },
            Pick::SeekBack => {
                Action::Seek { fwd: false, ms: self.range(700, if stress { 1500 } else { 3000 }) }
            }
            Pick::SeekStart => Action::SeekStart,
            Pick::SeekEnd => Action::Seek { fwd: true, ms: 6000 },
            Pick::Pause => Action::Pause(if stress { self.range(300, 1500) } else { self.range(1000, 8000) }),
            Pick::Skip => Action::Skip {
                fwd: self.rng.below(10) < 7,
                n: u32::try_from(if stress { self.range(2, 8) } else { self.range(1, 5) }).unwrap_or(1),
            },
            Pick::Volume => Action::Volume(u32::try_from(self.range(1, 3)).unwrap_or(1)),
            Pick::Dsp => Action::Dsp(self.rng.next() & 1 == 0),
            Pick::Eq => {
                let mut v = [0i8; 7];
                for x in &mut v[..6] {
                    *x = (self.range(0, 12) as i8) - 6;
                }
                v[6] = self.range(0, 3) as i8;
                Action::Eq(v)
            }
            Pick::NpView => Action::NpView(u32::try_from(self.range(1, 4)).unwrap_or(1)),
            Pick::Listen => Action::Listen(match self.scenario {
                Scenario::Browse => listen(self, 30, 120),
                Scenario::Settings => listen(self, 60, 240),
                _ => listen(self, 60, 300),
            }),
            Pick::LongListen => Action::Listen(listen(self, 300, 900)),
            Pick::Idle => Action::Idle(self.range(120, 600)),
            Pick::BrowseFiles => Action::Browse { app: App::Files, steps: steps(self, 20, 60), seed },
            Pick::BrowseLibrary => Action::Browse { app: App::Library, steps: steps(self, 20, 60), seed },
            Pick::BrowsePictures => Action::Browse { app: App::Pictures, steps: steps(self, 10, 30), seed },
            Pick::BrowseSettings => Action::Browse { app: App::Settings, steps: steps(self, 10, 30), seed },
            Pick::BrowseNowPlaying => {
                Action::Browse { app: App::NowPlaying, steps: steps(self, 8, 20), seed }
            }
            Pick::Menus => Action::Menus { steps: steps(self, 6, 16), seed },
            Pick::RadioOn => Action::RadioOn(
                (self.rng.next() & 1 == 0).then(|| 87_500 + 100 * (self.rng.next() % 206) as u32),
            ),
            Pick::RadioSeek => Action::RadioSeek(self.rng.next() & 1 == 0),
            Pick::RadioScan => Action::RadioScan,
            Pick::RadioOff => Action::RadioOff,
            Pick::RadioKeys => Action::RadioKeys { steps: steps(self, 6, 20), seed },
            Pick::Record => Action::Record(u32::try_from(self.range(5, 12)).unwrap_or(5)),
            Pick::SetSound => self.pick_value(&SOUND_IDS),
            Pick::SetPlayback => self.pick_value(&PLAYBACK_IDS),
            Pick::SetDisplay => self.pick_value(&DISPLAY_IDS),
            Pick::Look => {
                let mut values = Vec::new();
                for id in LOOK_IDS {
                    if self.rng.below(3) > 0 {
                        let v = valid_values(id);
                        values.push((id, v.get(self.rng.below(v.len())).copied().unwrap_or(0)));
                    }
                }
                Action::Look { values, steps: steps(self, 6, 20), seed, secs: listen(self, 20, 90) }
            }
            Pick::Handover => Action::Handover(self.range(10, 40)),
            Pick::Personality => Action::Personality(self.range(10, 30)),
        }
    }
}

/// Random keys inside an app (deterministic per seed).
pub fn browse_steps(seed: u64, n: u32, fast: bool, radio: bool) -> Vec<Step> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::new();
    let tap = |out: &mut Vec<Step>, k: Key| {
        if fast {
            out.extend([Step::Press(k), Step::Wait(20), Step::Release(k), Step::Wait(20)]);
        } else {
            out.push(Step::Tap(k, 1));
        }
    };
    let hold = |rng: &mut Rng, lo: u64, hi: u64| lo + rng.next() % (hi - lo + 1);
    for _ in 0..n {
        let r = rng.below(100);
        if radio {
            match r {
                0..=19 => tap(&mut out, Key::Next),
                20..=34 => tap(&mut out, Key::Prev),
                35..=49 => tap(&mut out, Key::Up),
                50..=64 => tap(&mut out, Key::Down),
                65..=74 => out.extend([Step::Tap(Key::Ok, 1), Step::Wait(800), Step::Tap(Key::Ok, 1)]),
                75..=80 => out.extend([Step::Hold(Key::Next, 900), Step::Wait(3000)]),
                81..=84 => out.extend([Step::Hold(Key::Prev, 900), Step::Wait(3000)]),
                _ => out.push(Step::Wait(hold(&mut rng, 200, 1500))),
            }
            continue;
        }
        match r {
            0..=23 => tap(&mut out, Key::Down),
            24..=33 => tap(&mut out, Key::Up),
            34..=41 => out.push(Step::Hold(Key::Down, hold(&mut rng, 800, 3000))),
            42..=46 => out.push(Step::Hold(Key::Up, hold(&mut rng, 800, 2500))),
            47..=54 => tap(&mut out, Key::Next),
            55..=59 => tap(&mut out, Key::Prev),
            60..=71 => tap(&mut out, Key::Ok),
            72..=80 => tap(&mut out, Key::Back),
            81..=85 => tap(&mut out, Key::Menu),
            _ => {
                out.push(Step::Wait(if fast { hold(&mut rng, 20, 200) } else { hold(&mut rng, 150, 1500) }));
            }
        }
    }
    out
}

/// Menu sheets: open, move, close (OK goes through the guard).
pub fn menu_steps(seed: u64, n: u32) -> Vec<Step> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::new();
    for _ in 0..n {
        out.push(Step::Tap(Key::Menu, 1));
        out.push(Step::Wait(150));
        for _ in 0..rng.below(4) {
            out.push(Step::Tap(if rng.below(3) == 0 { Key::Up } else { Key::Down }, 1));
        }
        out.push(Step::Tap(Key::Back, 1));
        out.push(Step::Wait(100 + rng.next() % 400));
    }
    out
}

// ----------------------------------------------------------- readings

/// `status` (fw/firmware/src/audio.rs `status_reply`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub playing: bool,
    pub frames: u64,
    pub rate: u64,
    pub underruns: u64,
    pub corrupt: u64,
    pub written: u64,
    pub played: u64,
    pub pos_ms: Option<u64>,
    pub q_pos: u64,
    pub q_len: u64,
    pub title: String,
    /// `ub:` join fill serve pf prep lib art dec.
    pub ub: [u64; 8],
}

pub const UB: [&str; 8] = ["join", "fill", "serve", "pf", "prep", "lib", "art", "dec"];

fn field(reply: &str, key: &str) -> Option<u64> {
    reply.split_whitespace().find_map(|w| w.strip_prefix(key)?.strip_prefix('=')?.parse().ok())
}

pub fn parse_status(r: &str) -> Option<Status> {
    let playing = r.starts_with("playing ");
    if !playing && !r.starts_with("idle ") {
        return None;
    }
    let mut s = Status {
        playing,
        frames: field(r, "frames")?,
        rate: field(r, "rate").unwrap_or(0),
        underruns: field(r, "underruns").unwrap_or(0),
        corrupt: field(r, "corrupt").unwrap_or(0),
        written: field(r, "written").unwrap_or(0),
        played: field(r, "played").unwrap_or(0),
        pos_ms: field(r, "pos"),
        ..Status::default()
    };
    if let Some(q) = r.split_whitespace().find_map(|w| w.strip_prefix("q="))
        && let Some((a, b)) = q.split_once('/')
    {
        s.q_pos = a.parse().unwrap_or(0);
        s.q_len = b.parse().unwrap_or(0);
    }
    if let Some(i) = r.find(" title=") {
        let t = &r[i + 7..];
        s.title = t[..t.find(" join#").unwrap_or(t.len())].to_string();
    }
    if let Some(i) = r.find(" ub: ") {
        let tail = &r[i..];
        for (k, n) in UB.iter().enumerate() {
            s.ub[k] = field(tail, n).unwrap_or(0);
        }
    }
    Some(s)
}

/// STATS group 0 and INFO's battery.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub uptime_ms: u32,
    pub idle_us: u32,
    pub loads: u32,
    pub code_faults: u32,
    pub ro_faults: u32,
    pub evictions: u32,
    pub clock_level: u32,
    pub clock_switches: u32,
    /// DAC headroom low-water since the last read (frames).
    pub head_min: Option<i32>,
    pub card_errors: u32,
    pub load_errors: u32,
    pub battery_mv: u16,
    /// The firmware reports the VER-12 words (21–23).
    pub soak_words: bool,
    /// The USB controller's counters (USB-W1, words 0–2 and 24–30).
    pub usb: Option<UsbCounters>,
}

pub use orpheus_proto::devchan::usb::Counters as UsbCounters;

/// One line of USB counters (printed on a channel loss).
pub fn usb_line(u: &UsbCounters) -> String {
    use orpheus_proto::devchan::usb::error_name;
    format!(
        "usb: setups {} resets {} stalls {} aborts {} dma-errors {} re-inits {} inits {} comabt {}, \
         last error {} at {} s{}{}{}",
        u.setups,
        u.resets,
        u.stalls,
        u.aborts,
        u.dma_errors,
        u.reinits,
        u.inits,
        u.comabt,
        error_name(u.last_error),
        u.last_error_s,
        if u.vbus { "" } else { ", no VBUS" },
        if u.setup_waiting { ", a SETUP waiting" } else { "" },
        if u.wedged { ", WEDGED" } else { "" },
    )
}

/// After a lost channel: did the firmware recover USB by itself (same boot,
/// more re-initialisations than before)? `Some(text)` describes it.
pub fn self_recovery(
    before: Option<&UsbCounters>,
    after: Option<&UsbCounters>,
    rebooted: bool,
) -> Option<String> {
    let (b, a) = (before?, after?);
    if rebooted || a.reinits <= b.reinits {
        return None;
    }
    Some(format!(
        "USB recovered by itself ({} re-init{}, last error {}), channel lost and back without a reboot",
        a.reinits - b.reinits,
        if a.reinits - b.reinits == 1 { "" } else { "s" },
        orpheus_proto::devchan::usb::error_name(a.last_error),
    ))
}

impl Counters {
    /// Words as `xtask dev stats` reads them (fw/firmware/src/usbdev.rs).
    pub fn from_words(w: &[u32], battery_mv: u16) -> Self {
        let g = |i: usize| w.get(i).copied().unwrap_or(0);
        let head = g(21) as i32;
        Counters {
            usb: UsbCounters::from_words(w),
            uptime_ms: g(8),
            idle_us: g(3),
            loads: g(4),
            code_faults: g(5),
            ro_faults: g(6),
            evictions: g(7),
            clock_level: g(19),
            clock_switches: g(20),
            head_min: (w.len() > 21 && head != i32::MAX).then_some(head),
            card_errors: g(22),
            load_errors: g(23),
            battery_mv,
            soak_words: w.len() > 23,
        }
    }
}

/// One oracle reading.
/// Longest round trip of an uptime read used by the clock oracle (ms).
const UPTIME_RTT_MAX_MS: u32 = 200;

#[derive(Clone, Debug, Default)]
pub struct Sample {
    /// Host time (ms since the run started).
    pub t_ms: u64,
    pub uptime_ms: u32,
    /// Host time at the middle of the request that read `uptime_ms`, and
    /// that request's round trip (0, 0: use `t_ms`). A slow reply is left
    /// out of the clock oracle: stamped before the request, a 1.6 s late
    /// answer read as +1,613 ppm (soak, 2026-11-25).
    pub t_up_ms: u64,
    pub up_rtt_ms: u32,
    pub stack_free: u32,
    pub fault: Option<(u32, String)>,
    pub c: Counters,
    pub st: Option<Status>,
    pub screen: String,
    pub action: usize,
    pub kind: &'static str,
}

// ------------------------------------------------------------- oracles

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Fault,
    Reset,
    Stack,
    Underrun,
    Thrash,
    Ring,
    CardIo,
    Pager,
    Drift,
    Clock,
    Card,
    Settings,
    Queue,
    Seek,
    Pause,
    /// The channel was lost and USB came back by itself (USB-W1): logged,
    /// the run goes on.
    UsbRecovery,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::UsbRecovery => "usb-recovery",
            Kind::Fault => "fault",
            Kind::Reset => "reset",
            Kind::Stack => "stack",
            Kind::Underrun => "underrun",
            Kind::Thrash => "thrash",
            Kind::Ring => "ring",
            Kind::CardIo => "card-io",
            Kind::Pager => "pager",
            Kind::Drift => "drift",
            Kind::Clock => "clock",
            Kind::Card => "card",
            Kind::Settings => "settings",
            Kind::Queue => "queue",
            Kind::Seek => "seek",
            Kind::Pause => "pause",
        }
    }
    /// Found after the run (not by replaying actions).
    fn after_run(self) -> bool {
        matches!(self, Kind::Card | Kind::Settings | Kind::Queue | Kind::Drift | Kind::Clock)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub kind: Kind,
    pub detail: String,
    pub t_ms: u64,
    pub action: usize,
}

/// A crash shows as a fault record, a reset or both.
fn same_kind(a: Kind, b: Kind) -> bool {
    a == b || matches!((a, b), (Kind::Fault | Kind::Reset, Kind::Fault | Kind::Reset))
}

/// Run statistics for the summary and the history.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub samples: u64,
    /// Sample intervals with counter deltas (the means' divisor).
    pub intervals: u64,
    pub loads_per_s_sum: f64,
    pub loads_per_s_max: f64,
    pub evictions_per_s_max: f64,
    pub stack_free_min: Option<u32>,
    pub head_min: Option<i32>,
    pub underruns_total: u64,
    pub underruns_in_disruptions: u64,
    pub idle_pct_sum: f64,
    pub card_errors: u32,
    /// Longest DAC segment: (seconds, ppm against the host clock).
    pub drift: Option<(f64, f64)>,
    /// Device uptime against the host clock over the run.
    pub clock_ppm: Option<f64>,
    pub battery_mv_min: Option<u16>,
}

impl Stats {
    pub fn loads_per_s_mean(&self) -> f64 {
        if self.intervals == 0 { 0.0 } else { self.loads_per_s_sum / self.intervals as f64 }
    }
    pub fn idle_pct_mean(&self) -> f64 {
        if self.intervals == 0 { 0.0 } else { self.idle_pct_sum / self.intervals as f64 }
    }
}

/// The DAC segment the drift is measured over.
#[derive(Clone, Copy, Debug)]
struct Seg {
    t0: u64,
    rate: u64,
    halves: u64,
    last_played: u64,
    last_t: u64,
}

/// The oracles of 07 §5.10 over a stream of samples.
pub struct Oracles {
    pub b: Budget,
    base_uptime: u32,
    base_fault: Option<u32>,
    first: Option<(u64, u32)>,
    prev: Option<Sample>,
    /// The action the previous sample was taken in.
    prev_action: Option<usize>,
    disrupt_until: u64,
    /// Underruns allowed in the current disruption window, still left.
    window_left: u64,
    thrash_run: u32,
    evict_run: u32,
    seg: Option<Seg>,
    pub stats: Stats,
    /// Per-sample derived values (for the CSV): loads/s, evictions/s, idle %, ring halves.
    pub derived: Vec<[f64; 4]>,
}

impl Oracles {
    pub fn new(b: Budget, base_uptime: u32, base_fault: Option<u32>) -> Self {
        Self {
            b,
            base_uptime,
            base_fault,
            first: None,
            prev: None,
            prev_action: None,
            disrupt_until: 0,
            window_left: 0,
            thrash_run: 0,
            evict_run: 0,
            seg: None,
            stats: Stats::default(),
            derived: Vec::new(),
        }
    }

    /// A disruptive action ran at `t_ms` (host): underruns are allowed for
    /// the window that follows (bounded by the budget).
    pub fn disrupt(&mut self, t_ms: u64, extra_s: u64) {
        let until = t_ms + (self.b.seek_window_s + extra_s) * 1000;
        if t_ms > self.disrupt_until {
            self.window_left = self.b.underruns_per_disruption_max;
        }
        self.disrupt_until = self.disrupt_until.max(until);
        // A seek or a source change restarts the DAC segment.
        self.seg = None;
    }

    /// What a counter read at `s` covers: the low-water and the underrun
    /// count are over the whole time since the previous sample, which may
    /// lie in an earlier action (the VER-12 finding of 2026-11-25 read a
    /// low-water over 76 s that began in `browse pictures` and was reported
    /// in `radio-keys`).
    fn since(&self, s: &Sample, action: usize) -> String {
        let secs = |p: &Sample| s.t_ms.saturating_sub(p.t_ms) as f64 / 1000.0;
        match (&self.prev, self.prev_action) {
            (Some(p), Some(a)) if a != action => {
                format!("; over the {:.1} s since the sample in action #{a}", secs(p))
            }
            (Some(p), _) => format!("; over the {:.1} s since the last sample", secs(p)),
            _ => String::new(),
        }
    }

    pub fn check(&mut self, s: &Sample, action: usize) -> Vec<Finding> {
        let since = self.since(s, action);
        let mut out = Vec::new();
        let mut f = |kind: Kind, detail: String| out.push(Finding { kind, detail, t_ms: s.t_ms, action });
        if let Some((count, text)) = &s.fault
            && Some(*count) != self.base_fault
        {
            f(Kind::Fault, format!("fault record: {text}"));
            self.base_fault = Some(*count);
        }
        if s.uptime_ms < self.base_uptime {
            f(
                Kind::Reset,
                format!(
                    "uptime went back from {} to {} ms (watchdog or reset)",
                    self.base_uptime, s.uptime_ms
                ),
            );
            self.first = None;
        }
        self.base_uptime = s.uptime_ms;
        if s.stack_free < self.b.stack_free_min {
            f(Kind::Stack, format!("stack free {} B (budget {} B)", s.stack_free, self.b.stack_free_min));
        }
        self.stats.samples += 1;
        self.stats.stack_free_min =
            Some(self.stats.stack_free_min.map_or(s.stack_free, |m| m.min(s.stack_free)));
        if s.c.battery_mv != 0 {
            self.stats.battery_mv_min =
                Some(self.stats.battery_mv_min.map_or(s.c.battery_mv, |m| m.min(s.c.battery_mv)));
        }
        let disrupted = s.t_ms <= self.disrupt_until;
        let t_up = if s.up_rtt_ms == 0 && s.t_up_ms == 0 { s.t_ms } else { s.t_up_ms };
        let timely = s.up_rtt_ms <= UPTIME_RTT_MAX_MS;
        let first = if timely { *self.first.get_or_insert((t_up, s.uptime_ms)) } else { (u64::MAX, 0) };
        let span = t_up.saturating_sub(first.0);
        if timely && first.0 != u64::MAX && span >= 60_000 && s.uptime_ms >= first.1 {
            let dev = f64::from(s.uptime_ms - first.1);
            let ppm = (dev - span as f64) / span as f64 * 1e6;
            self.stats.clock_ppm = Some(ppm);
        }
        let mut derived = [0.0; 4];
        if let Some(p) = &self.prev {
            let dt_ms = s.c.uptime_ms.wrapping_sub(p.c.uptime_ms);
            if dt_ms > 0 && s.uptime_ms >= p.uptime_ms {
                let dt = f64::from(dt_ms) / 1000.0;
                let lps = f64::from(s.c.loads.wrapping_sub(p.c.loads)) / dt;
                let eps = f64::from(s.c.evictions.wrapping_sub(p.c.evictions)) / dt;
                let idle = f64::from(s.c.idle_us.wrapping_sub(p.c.idle_us)) / (dt * 1e4);
                derived = [lps, eps, idle.min(100.0), 0.0];
                self.stats.intervals += 1;
                self.stats.loads_per_s_sum += lps;
                self.stats.idle_pct_sum += idle.min(100.0);
                self.stats.loads_per_s_max = self.stats.loads_per_s_max.max(lps);
                self.stats.evictions_per_s_max = self.stats.evictions_per_s_max.max(eps);
                self.thrash_run = if lps > self.b.loads_per_s_max { self.thrash_run + 1 } else { 0 };
                self.evict_run = if eps > self.b.evictions_per_s_max { self.evict_run + 1 } else { 0 };
                if self.thrash_run == self.b.thrash_samples {
                    f(
                        Kind::Thrash,
                        format!(
                            "pager: {lps:.0} line loads/s for {} samples (budget {:.0})",
                            self.thrash_run, self.b.loads_per_s_max
                        ),
                    );
                }
                if self.evict_run == self.b.thrash_samples {
                    f(
                        Kind::Thrash,
                        format!(
                            "pager: {eps:.0} evictions/s for {} samples (budget {:.0})",
                            self.evict_run, self.b.evictions_per_s_max
                        ),
                    );
                }
                let ce = s.c.card_errors.wrapping_sub(p.c.card_errors);
                self.stats.card_errors += ce;
                if self.stats.card_errors > self.b.card_errors_max && ce > 0 {
                    f(Kind::CardIo, format!("{ce} card I/O errors (run total {})", self.stats.card_errors));
                }
                let le = s.c.load_errors.wrapping_sub(p.c.load_errors);
                if le > 0 && s.c.load_errors > self.b.load_errors_max {
                    f(Kind::Pager, format!("{le} failed pager line loads"));
                }
            }
            // Underruns of the track (per-track counter: a new track
            // starts from 0).
            if let (Some(a), Some(b)) = (&p.st, &s.st) {
                let same = a.title == b.title
                    && a.q_pos == b.q_pos
                    && a.q_len == b.q_len
                    && b.underruns >= a.underruns;
                let (d, ub): (u64, Vec<u64>) = if same {
                    (
                        b.underruns - a.underruns,
                        b.ub.iter().zip(a.ub.iter()).map(|(x, y)| x.saturating_sub(*y)).collect(),
                    )
                } else {
                    (b.underruns, b.ub.to_vec())
                };
                if d > 0 {
                    self.stats.underruns_total += d;
                    let causes: Vec<String> = UB
                        .iter()
                        .zip(&ub)
                        .filter(|(_, n)| **n > 0)
                        .map(|(k, n)| format!("{k}={n}"))
                        .collect();
                    if disrupted && d <= self.window_left {
                        self.window_left -= d;
                        self.stats.underruns_in_disruptions += d;
                    } else {
                        f(
                            Kind::Underrun,
                            format!(
                                "{d} underruns {} ({}) on `{}` q={}/{}{since}",
                                if disrupted { "beyond the disruption allowance" } else { "outside seeks" },
                                causes.join(" "),
                                b.title,
                                b.q_pos,
                                b.q_len
                            ),
                        );
                    }
                }
            }
        }
        if let Some(h) = s.c.head_min {
            self.stats.head_min = Some(self.stats.head_min.map_or(h, |m| m.min(h)));
            if i64::from(h) < self.b.head_min_frames && !disrupted {
                f(
                    Kind::Ring,
                    format!("DAC headroom fell to {h} frames (budget ≥ {}){since}", self.b.head_min_frames),
                );
            }
        }
        // Played samples against the host's clock.
        if let Some(st) = &s.st {
            derived[3] = st.written.wrapping_sub(st.played) as f64;
            self.drift(s.t_ms, st);
        }
        self.derived.push(derived);
        self.prev = Some(s.clone());
        self.prev_action = Some(action);
        out
    }

    fn drift(&mut self, t: u64, st: &Status) {
        let fresh = Seg { t0: t, rate: st.rate, halves: 0, last_played: st.played, last_t: t };
        if !st.playing || st.rate == 0 {
            self.seg = None;
            return;
        }
        let Some(mut g) = self.seg else {
            self.seg = Some(fresh);
            return;
        };
        let dt = t.saturating_sub(g.last_t) as f64 / 1000.0;
        if st.rate != g.rate || st.played < g.last_played || dt <= 0.0 {
            self.seg = Some(fresh);
            return;
        }
        let d = st.played - g.last_played;
        let audio = (d * HALF_FRAMES) as f64 / g.rate as f64;
        // A pause or a restart in between: start again.
        if (audio - dt).abs() > dt * 0.2 + 0.1 {
            self.seg = Some(fresh);
            return;
        }
        g.halves += d;
        g.last_played = st.played;
        g.last_t = t;
        self.seg = Some(g);
        let span = (t - g.t0) as f64 / 1000.0;
        if span >= 60.0 {
            let audio = (g.halves * HALF_FRAMES) as f64 / g.rate as f64;
            let ppm = (audio - span) / span * 1e6;
            if self.stats.drift.is_none_or(|(s, _)| span >= s) {
                self.stats.drift = Some((span, ppm));
            }
        }
    }

    /// Checks over the whole run.
    pub fn finish(&self, t_ms: u64, action: usize) -> Vec<Finding> {
        let mut out = Vec::new();
        let st = &self.stats;
        if st.intervals >= 3 && st.loads_per_s_mean() > self.b.loads_per_s_mean_max {
            out.push(Finding {
                kind: Kind::Thrash,
                detail: format!(
                    "mean {:.0} line loads/s over the run (budget {:.0})",
                    st.loads_per_s_mean(),
                    self.b.loads_per_s_mean_max
                ),
                t_ms,
                action,
            });
        }
        if let Some((span, ppm)) = st.drift
            && span >= self.b.drift_min_window_s as f64
            && ppm.abs() > self.b.drift_ppm_max
        {
            out.push(Finding {
                kind: Kind::Drift,
                detail: format!(
                    "DAC {ppm:+.0} ppm against the host clock over {span:.0} s (budget ±{:.0})",
                    self.b.drift_ppm_max
                ),
                t_ms,
                action,
            });
        }
        if let Some(ppm) = st.clock_ppm
            && ppm.abs() > self.b.clock_ppm_max
        {
            out.push(Finding {
                kind: Kind::Clock,
                detail: format!(
                    "uptime {ppm:+.0} ppm against the host clock (budget ±{:.0})",
                    self.b.clock_ppm_max
                ),
                t_ms,
                action,
            });
        }
        out
    }
}

// ------------------------------------------------------------------ rig

/// The player (or the simulator) as the soak drives it.
pub trait Rig: Player {
    /// Time since the run started (host clock; the simulator's clock).
    fn wall_ms(&mut self) -> u64;
    fn counters(&mut self) -> Result<Counters>;
    fn is_sim(&self) -> bool {
        false
    }
    /// Upload under `/.orpheus/` (`--setup`).
    fn put(&mut self, remote: &str, data: &[u8]) -> Result<()>;
    /// A personality switch: the player enumerates again.
    fn switch(&mut self, text: &str) -> Result<()>;
}

static T0: OnceLock<Instant> = OnceLock::new();

impl Rig for UsbPlayer {
    fn wall_ms(&mut self) -> u64 {
        u64::try_from(T0.get_or_init(Instant::now).elapsed().as_millis()).unwrap_or(u64::MAX)
    }
    fn counters(&mut self) -> Result<Counters> {
        let w = self.stats(0)?;
        let mv = self.battery_mv()?;
        Ok(Counters::from_words(&w, mv))
    }
    fn put(&mut self, remote: &str, data: &[u8]) -> Result<()> {
        self.upload(remote, data)
    }
    fn switch(&mut self, text: &str) -> Result<()> {
        // The switch re-enumerates the player, sometimes before the status
        // stage of this request completes: the error is only a note.
        if let Err(e) = self.send(text) {
            log(&format!("   ({text}: {e:#}; checking the result instead)"));
        }
        self.sleep(1500);
        self.reconnect()
    }
}

/// An action's counters are read at its end when the last read is older
/// than this (`Ctx::boundary`).
const BOUNDARY_MS: u64 = 2000;

/// A bug planted in the simulator (host tests of the oracles and the
/// minimiser): after the commands of `after` were sent in this order, each
/// later `status` reports one more underrun (cause `dec`).
#[derive(Clone, Debug, Default)]
pub struct Planted {
    pub after: Vec<&'static str>,
    seen: usize,
}

/// The simulator behind the soak: the suite's [`SimPlayer`] plus the test
/// commands a soak sends (playback with a DAC model, radio, recorder,
/// sessions, personalities, listings).
#[derive(Default)]
pub struct SoakSim {
    pub p: SimPlayer,
    own: Option<String>,
    session: bool,
    held: u32,
    dac_ms: u64,
    last_ms: u64,
    playing_at_last: bool,
    loads: u64,
    underruns: u64,
    pub planted: Option<Planted>,
    pub sent: Vec<String>,
    look: bool,
    looks: Vec<(u16, i16)>,
}

/// Settings the test look reads as their defaults.
fn look_id(id: u16) -> bool {
    (0x0304..=0x030C).contains(&id) || id == 0x0A01 || id == 0x0D03
}

impl SoakSim {
    fn now(&self) -> u64 {
        self.p.sim.now.0
    }
    fn playing(&self) -> bool {
        use orpheus_apps::services::PlayerService;
        self.p.sim.sys.player().source == orpheus_ui::event::Source::Playing
    }
    /// The DAC and the pager advance with the simulator's clock.
    fn tick(&mut self) {
        let now = self.now();
        let dt = now.saturating_sub(self.last_ms);
        if self.playing_at_last {
            self.dac_ms += dt;
        }
        self.loads += dt * if self.playing_at_last { 3 } else { 1 };
        self.last_ms = now;
        self.playing_at_last = self.playing();
    }
    fn status(&self) -> String {
        use orpheus_apps::services::PlayerService;
        let p = self.p.sim.sys.player();
        let playing = p.source == orpheus_ui::event::Source::Playing;
        let title = self.p.sim.sys.track().map(|t| t.title.to_string()).unwrap_or_default();
        let played = self.dac_ms * 44_100 / 1000 / HALF_FRAMES;
        format!(
            "{} frames={} rate={} underruns={} corrupt=0 where=0 written={} played={played} walk=0/0 d0 pos={} q={}/{} shuf=0 title={title} join#0: head=- dec: n=0 us=0 max_us=0 loads=0 ub: join=0 fill=0 serve=0 pf=0 prep=0 lib=0 art=0 dec={}",
            if playing { "playing" } else { "idle" },
            played * HALF_FRAMES,
            if playing { 44_100 } else { 0 },
            self.underruns,
            played + 1,
            p.position_ms,
            p.queue_pos + u32::from(p.queue_len > 0),
            p.queue_len,
            self.underruns,
        )
    }
    fn run(&mut self, script: &str) {
        let _ = self.p.sim.run_script(script);
    }
    fn ls(path: &str) -> String {
        match path.trim_end_matches('/') {
            "" => "Music/\n.orpheus/\n(2 entries)".into(),
            "/Music" => "Sigur Rós/\nКино/\nYOASOBI/\n(3 entries)".into(),
            "/Music/Sigur Rós" => "Takk…/\n(1 entries)".into(),
            "/Music/Кино" => "Звезда по имени Солнце/\n(1 entries)".into(),
            "/Music/YOASOBI" => "THE BOOK/\n(1 entries)".into(),
            p if p.starts_with("/Music/") => "01 One.mp3\n02 Two.flac\n03 Three.ogg\n(3 entries)".into(),
            "/.orpheus/test/suite/music" => "01 Tone 440.wav\n02 Tone 880.wav\n(2 entries)".into(),
            _ => "no such directory".into(),
        }
    }
    fn handle(&mut self, text: &str) -> Option<String> {
        if let Some(pl) = &mut self.planted
            && pl.seen < pl.after.len()
            && text.starts_with(pl.after[pl.seen])
        {
            pl.seen += 1;
        }
        let w: Vec<&str> = text.split_whitespace().collect();
        Some(match w.as_slice() {
            ["status"] => {
                if self.planted.as_ref().is_some_and(|p| p.seen == p.after.len()) {
                    self.underruns += 1;
                }
                self.status()
            }
            ["playdir", ..] => {
                let path = text["playdir ".len()..].trim();
                let q = if path.contains("Кино") {
                    "kino"
                } else if path.contains("YOASOBI") {
                    "yoasobi"
                } else {
                    "takk"
                };
                self.run(&format!("set:queue={q}"));
                self.underruns = 0;
                "playdir ok".into()
            }
            ["stop"] => {
                self.run("set:pause=1");
                return None;
            }
            ["ls", ..] => Self::ls(text["ls ".len()..].trim()),
            ["fm", "on"] => {
                self.run("set:radio=101900");
                "fm: busy".into()
            }
            ["fm", "tune", k] => {
                self.run(&format!("set:radio={k}"));
                "fm: busy".into()
            }
            ["fm", "off"] => {
                self.run("set:radio=off");
                "fm: busy".into()
            }
            ["fm", "seek" | "scan", ..] => "fm: busy".into(),
            ["rec", "start", ..] => "rec: starting".into(),
            ["rec", "status"] => "rec off".into(),
            ["rec", "stop"] => "rec: not recording".into(),
            ["settings", "test-session", "on"] => {
                self.session = true;
                format!("test-session on held={} store=ok keys=65 crc=1234abcd", self.held)
            }
            ["settings", "test-session"] => format!(
                "test-session {} held={} store=ok keys=65 crc=1234abcd",
                if self.session { "on" } else { "off" },
                self.held
            ),
            ["settings", "test-set", id, v] => {
                if !self.session {
                    return Some("test-set: no test session (settings test-session on)".into());
                }
                // As the firmware's `parse_num`: hexadecimal with `0x`,
                // decimal otherwise (the session sends decimal ids).
                let id = match id.strip_prefix("0x") {
                    Some(h) => u16::from_str_radix(h, 16).ok(),
                    None => id.parse().ok(),
                };
                match (id.filter(|i| session_settable(*i)), v.parse::<i16>()) {
                    (Some(i), Ok(v)) => {
                        self.looks.retain(|x| x.0 != i);
                        if look_id(i) {
                            self.looks.push((i, v));
                        }
                        if !(look_id(i) && self.look) {
                            self.p.sim.sys.settings.set(orpheus_apps::settings::SettingId(i), v);
                        }
                        self.held += 1;
                        format!("test-set {i}={v} ok")
                    }
                    _ => "test-set: not allowed".into(),
                }
            }
            ["ui", "test-look", on @ ("on" | "off")] => {
                // As the firmware: the look's settings read as their
                // defaults while it is on; the session's values otherwise.
                self.look = *on == "on";
                for id in (0x0304..=0x030Cu16).chain([0x0A01, 0x0D03]) {
                    let sid = orpheus_apps::settings::SettingId(id);
                    let Some(d) = orpheus_apps::settings::def(sid) else { continue };
                    let v = if self.look {
                        d.default
                    } else {
                        self.looks.iter().find(|x| x.0 == id).map_or(d.default, |x| x.1)
                    };
                    self.p.sim.sys.settings.set(sid, v);
                }
                format!("test-look {on}")
            }
            ["ui", "screen-off"] => {
                self.p.sim.shell.gate.screen_on = false;
                self.p.sim.shell.invalidate();
                "screen-off".into()
            }
            // No reply (as the firmware): the last one stays.
            _ => return None,
        })
    }
}

impl Player for SoakSim {
    fn key(&mut self, k: Key, down: bool) -> Result<()> {
        self.p.key(k, down)
    }
    fn sleep(&mut self, ms: u64) {
        self.p.sleep(ms);
    }
    fn send(&mut self, text: &str) -> Result<()> {
        self.tick();
        self.sent.push(text.to_string());
        match self.handle(text) {
            Some(r) => self.own = Some(r),
            None => {
                let quiet = ["stop", "dsp ", "sound ", "clock ", "gov ", "usb ", "charge ", "drain "]
                    .iter()
                    .any(|q| text.starts_with(q));
                if !quiet {
                    self.own = None;
                    self.p.send(text)?;
                }
            }
        }
        Ok(())
    }
    fn reply(&mut self) -> Result<String> {
        match &self.own {
            Some(r) => Ok(r.clone()),
            None => self.p.reply(),
        }
    }
    fn frame(&mut self) -> Result<Vec<u16>> {
        self.p.frame()
    }
    fn health(&mut self) -> Result<player::Health> {
        self.p.health()
    }
    fn fault(&mut self) -> Result<Option<orpheus_proto::fault::FaultRecord>> {
        self.p.fault()
    }
    fn reboot(&mut self) -> Result<()> {
        let planted = self.planted.take().map(|p| Planted { after: p.after, seen: 0 });
        *self = SoakSim { planted, sent: std::mem::take(&mut self.sent), ..SoakSim::default() };
        Ok(())
    }
    fn reconnect(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Rig for SoakSim {
    fn wall_ms(&mut self) -> u64 {
        self.now()
    }
    fn counters(&mut self) -> Result<Counters> {
        self.tick();
        let up = u32::try_from(self.now()).unwrap_or(u32::MAX);
        let loads = (self.loads & 0xFFFF_FFFF) as u32;
        let mut w = vec![0u32; 24];
        w[3] = up.wrapping_mul(500);
        w[4] = loads;
        w[7] = loads;
        w[8] = up;
        w[19] = 1;
        w[21] = if self.playing() { 1200 } else { i32::MAX as u32 };
        Ok(Counters::from_words(&w, 4000))
    }
    fn is_sim(&self) -> bool {
        true
    }
    fn put(&mut self, _remote: &str, _data: &[u8]) -> Result<()> {
        Ok(())
    }
    fn switch(&mut self, text: &str) -> Result<()> {
        self.send(text)
    }
}

// ------------------------------------------------------------ executor

/// Firmware abilities found at the start.
#[derive(Clone, Copy, Debug, Default)]
pub struct Caps {
    pub session: bool,
    pub screen_off: bool,
    pub pos: bool,
    pub soak_words: bool,
}

/// One run's state.
pub struct Ctx {
    pub b: Budget,
    pub caps: Caps,
    homes: Vec<Vec<u16>>,
    pub stress: bool,
    look_on: bool,
    /// List roll-around is off for this session (`goto` holds Up to reach
    /// a list's first row, which only stops there when the ends clamp).
    lists_clamped: bool,
    pub radio_on: bool,
    pub recordings: u32,
    clock_pinned: bool,
    nav_fail: u32,
    seek_fails: u32,
    pause_fails: u32,
    exec: Exec,
    pub oracles: Oracles,
    pub samples: Vec<Sample>,
    pub findings: Vec<Finding>,
    pub skipped: BTreeMap<&'static str, u32>,
    pub done: BTreeMap<&'static str, u32>,
    pub notes: Vec<String>,
    next_sample: u64,
    pub action_no: usize,
    pub kind: &'static str,
    /// Keep samples (off in replays: only the verdict counts).
    pub keep: bool,
    /// `--battery`: stop the run below this voltage (mV).
    pub battery_stop: Option<u16>,
    /// Why the run ended early (not a finding).
    pub stopped: Option<String>,
    /// USB self-recoveries (USB-W1): findings that do not stop the run.
    pub recoveries: Vec<Finding>,
    /// The USB counters of the last sample.
    pub last_usb: Option<UsbCounters>,
    /// The uptime of the last sample.
    last_uptime: u32,
}

/// Failed seek (or pause) checks in a row that make a finding.
const FUNCTIONAL_RUN: u32 = 3;

/// The Home grid compared without the status bar and the strip (the
/// mini-player and its toasts live there).
fn home_masks() -> Masks {
    let r = |y: u8, h: u8, why: &str| mask::Rect { x: 0, y, w: 128, h, why: why.into() };
    Masks {
        rects: vec![r(0, 14, "status bar"), r(110, 18, "strip: mini-player, toasts")],
        pixels: Some(0),
        channel: None,
    }
}

impl Ctx {
    pub fn new(
        b: Budget,
        caps: Caps,
        homes: Vec<Vec<u16>>,
        base_uptime: u32,
        base_fault: Option<u32>,
    ) -> Self {
        Self {
            oracles: Oracles::new(b.clone(), base_uptime, base_fault),
            b,
            caps,
            homes,
            stress: false,
            look_on: true,
            lists_clamped: false,
            radio_on: false,
            recordings: 0,
            clock_pinned: false,
            nav_fail: 0,
            seek_fails: 0,
            pause_fails: 0,
            exec: Exec::default(),
            samples: Vec::new(),
            findings: Vec::new(),
            skipped: BTreeMap::new(),
            done: BTreeMap::new(),
            notes: Vec::new(),
            next_sample: 0,
            action_no: 0,
            kind: "start",
            keep: true,
            battery_stop: None,
            stopped: None,
            recoveries: Vec::new(),
            last_usb: None,
            last_uptime: 0,
        }
    }

    /// Read every oracle once.
    pub fn sample(&mut self, rig: &mut dyn Rig) -> Result<()> {
        let s = take_sample(rig, self)?;
        self.last_usb = s.c.usb;
        self.last_uptime = s.uptime_ms;
        let found = self.oracles.check(&s, self.action_no);
        for f in &found {
            log(&format!("   FOUND ({}): {}", f.kind.name(), f.detail));
        }
        self.findings.extend(found);
        if let Some(min) = self.battery_stop
            && s.c.battery_mv != 0
            && s.c.battery_mv < min
            && self.stopped.is_none()
        {
            let why = format!("battery {} mV, below {min} mV: run stopped", s.c.battery_mv);
            log(&format!("   {why}"));
            self.notes.push(why.clone());
            self.stopped = Some(why);
        }
        if self.keep {
            self.samples.push(s);
        } else {
            self.oracles.derived.clear();
        }
        self.next_sample = rig.wall_ms() + self.b.sample_every_s * 1000;
        Ok(())
    }

    /// Let `ms` pass, sampling when due; returns early on a finding.
    pub fn wait(&mut self, rig: &mut dyn Rig, ms: u64) -> Result<()> {
        let end = rig.wall_ms() + ms;
        loop {
            let now = rig.wall_ms();
            if now >= end || !self.findings.is_empty() || self.stopped.is_some() {
                return Ok(());
            }
            if now >= self.next_sample {
                self.sample(rig)?;
                continue;
            }
            rig.sleep((end.min(self.next_sample) - now).clamp(1, 1000));
        }
    }

    fn skip(&mut self, why: &str) {
        *self.skipped.entry(self.kind).or_default() += 1;
        if self.skipped.values().sum::<u32>() <= 20 {
            log(&format!("   skipped: {why}"));
        }
    }

    /// Guarded keys (the fuzzer's guard; Settings rows never change).
    fn keys(&mut self, rig: &mut dyn Rig, steps: &[Step]) -> Result<bool> {
        let before = self.exec.sent.len();
        for s in steps {
            self.exec.step(rig, s)?;
            if rig.wall_ms() >= self.next_sample {
                self.exec.release_all(rig)?;
                self.sample(rig)?;
                if !self.findings.is_empty() {
                    break;
                }
            }
        }
        self.exec.release_all(rig)?;
        Ok(self.exec.sent[before..].iter().any(|s| matches!(s, Step::Tap(Key::Ok, _))))
    }

    /// Turn list roll-around off for the session (UI-8 made a held Up wrap
    /// past the first row, so `goto` opened Music's Shuffle all instead of
    /// Now Playing); `false` without a test session.
    fn clamp_lists(&mut self, rig: &mut dyn Rig) -> Result<bool> {
        if !self.lists_clamped {
            self.lists_clamped = self.set(rig, LIST_ROLL, 0)?;
        }
        Ok(self.lists_clamped)
    }

    fn look(&mut self, rig: &mut dyn Rig, on: bool) -> Result<()> {
        if self.look_on != on {
            player::test_look(rig, on)?;
            self.look_on = on;
        }
        Ok(())
    }

    /// Home with the focus found from the screenshot; `false`: not found.
    fn home(&mut self, rig: &mut dyn Rig) -> Result<Option<usize>> {
        self.look(rig, true)?;
        let t = player::top(rig)?;
        if t.locked {
            return Err(Locked.into());
        }
        if !t.screen_on {
            player::tap(rig, Key::Back)?;
            rig.sleep(400);
        }
        if t.dialog {
            player::tap(rig, Key::Back)?;
        }
        for _ in 0..2 {
            player::hold(rig, Key::Back, script::HOLD_MS)?;
            if player::expect_screen(rig, "Home", 2500).is_ok() {
                break;
            }
            player::tap(rig, Key::Back)?;
        }
        let m = home_masks();
        for _ in 0..3 {
            let f = player::settled_frame(rig, 5)?;
            if let Some(k) = self.homes.iter().position(|h| mask::compare(&f, h, &m).0.pass()) {
                return Ok(Some(k));
            }
            rig.sleep(1200);
        }
        self.nav_fail += 1;
        Ok(None)
    }

    /// Open an app from Home. `false`: it did not open (skipped).
    fn goto(&mut self, rig: &mut dyn Rig, app: App) -> Result<bool> {
        let top = player::top(rig)?;
        let (want, name) = app.home();
        if app == App::NowPlaying && top.name == "NowPlaying" {
            return Ok(true);
        }
        if app == App::NowPlaying && !self.clamp_lists(rig)? {
            return Ok(false);
        }
        let Some(k) = self.home(rig)? else {
            if self.nav_fail >= 5 {
                bail!("Home's focus not found 5 times: the player's Home differs from the simulator's");
            }
            return Ok(false);
        };
        self.nav_fail = 0;
        let n = self.homes.len();
        let (key, times) = if (want + n - k) % n <= (k + n - want) % n {
            (Key::Next, (want + n - k) % n)
        } else {
            (Key::Prev, (k + n - want) % n)
        };
        for _ in 0..times {
            player::tap(rig, key)?;
        }
        player::tap(rig, Key::Ok)?;
        if player::expect_screen(rig, name, 3000).is_err() {
            return Ok(false);
        }
        if app == App::NowPlaying {
            // Music's first row is Now Playing while a queue exists; a
            // held key stops at the end of a list when the ends clamp. The
            // test look reads list roll-around as its default (on), so the
            // session's "off" (`clamp_lists`) holds only without it.
            self.look(rig, false)?;
            rig.sleep(300);
            player::hold(rig, Key::Up, 1500)?;
            player::tap(rig, Key::Ok)?;
            return Ok(player::expect_screen(rig, "NowPlaying", 2000).is_ok());
        }
        rig.sleep(300);
        Ok(true)
    }

    fn status(&mut self, rig: &mut dyn Rig) -> Option<Status> {
        let re = regex::Regex::new("underruns=|card: in use").ok()?;
        player::command(rig, "status", Some(&re), 2000).ok().and_then(|r| parse_status(&r))
    }

    fn cmd(&mut self, rig: &mut dyn Rig, text: &str, want: &str, timeout_ms: u64) -> Result<Option<String>> {
        let re = regex::Regex::new(want)?;
        Ok(player::command(rig, text, Some(&re), timeout_ms).ok())
    }

    /// `settings test-set`; `false`: refused or no session.
    fn set(&mut self, rig: &mut dyn Rig, id: u16, value: i16) -> Result<bool> {
        if !self.caps.session {
            return Ok(false);
        }
        let q = self.cmd(rig, &format!("settings test-set {id} {value}"), "^test-set", 3000)?;
        if q.as_deref() != Some("test-set: queued") {
            return Ok(q.is_some_and(|r| r.ends_with(" ok")));
        }
        // The UI task answers within its loop (≤ 1 s).
        for _ in 0..15 {
            rig.sleep(100);
            let r = rig.reply()?;
            if r.starts_with(&format!("test-set {id}=")) {
                return Ok(r.ends_with(" ok"));
            }
        }
        Ok(false)
    }

    /// Run one action (sampling while it lasts).
    pub fn run(&mut self, rig: &mut dyn Rig, a: &Action) -> Result<()> {
        self.kind = a.kind();
        let t0 = rig.wall_ms();
        let mut disrupted = a.disrupts();
        match a {
            Action::PlayDir(p) => {
                if self.radio_on && rig.is_sim() {
                    self.radio_on = false;
                }
                match self.cmd(rig, &format!("playdir {p}"), "^playdir", 10_000)? {
                    Some(r) if r.contains("ok") => self.wait(rig, 1500)?,
                    other => {
                        self.skip(&format!("playdir {p}: {}", other.unwrap_or_else(|| "no reply".into())));
                    }
                }
            }
            Action::Seek { fwd, ms } => {
                if !self.goto(rig, App::NowPlaying)? {
                    self.skip("Now Playing not reachable (nothing queued?)");
                } else {
                    let s0 = self.status(rig);
                    let k = if *fwd { Key::Next } else { Key::Prev };
                    let t = rig.wall_ms();
                    player::hold(rig, k, *ms)?;
                    self.oracles.disrupt(rig.wall_ms(), 0);
                    rig.sleep(600);
                    let s1 = self.status(rig);
                    let el = rig.wall_ms() - t;
                    self.check_seek(s0.as_ref(), s1.as_ref(), Some(*fwd), el, rig.wall_ms());
                }
            }
            Action::SeekStart => {
                if self.goto(rig, App::NowPlaying)? {
                    let s0 = self.status(rig);
                    if s0.as_ref().and_then(|s| s.pos_ms).is_some_and(|p| p > 4000) {
                        let t = rig.wall_ms();
                        player::tap(rig, Key::Prev)?;
                        self.oracles.disrupt(rig.wall_ms(), 0);
                        rig.sleep(600);
                        let s1 = self.status(rig);
                        let el = rig.wall_ms() - t;
                        self.check_seek(s0.as_ref(), s1.as_ref(), None, el, rig.wall_ms());
                    }
                } else {
                    self.skip("Now Playing not reachable");
                }
            }
            Action::Pause(ms) => {
                if self.goto(rig, App::NowPlaying)? {
                    let s0 = self.status(rig);
                    player::tap(rig, Key::Ok)?;
                    self.wait(rig, *ms)?;
                    player::tap(rig, Key::Ok)?;
                    self.oracles.disrupt(rig.wall_ms(), 0);
                    rig.sleep(1500);
                    let s1 = self.status(rig);
                    if let (Some(a), Some(b)) = (&s0, &s1)
                        && a.playing
                        && a.q_len == b.q_len
                        && a.q_pos == b.q_pos
                        && !rig.is_sim()
                    {
                        let t = rig.wall_ms();
                        let detail = format!("not playing 1.5 s after pause/resume on `{}`", a.title);
                        self.functional(Kind::Pause, !b.playing, detail, t);
                    }
                } else {
                    self.skip("Now Playing not reachable");
                }
            }
            Action::Skip { fwd, n } => {
                if self.goto(rig, App::NowPlaying)? {
                    let k = if *fwd { Key::Next } else { Key::Prev };
                    for _ in 0..*n {
                        rig.key(k, true)?;
                        rig.sleep(40);
                        rig.key(k, false)?;
                        rig.sleep(40);
                    }
                    self.oracles.disrupt(rig.wall_ms(), 1);
                    self.wait(rig, 1000)?;
                } else {
                    self.skip("Now Playing not reachable");
                }
            }
            Action::Volume(n) => {
                let mut steps = vec![Step::Tap(Key::VolUp, 1); *n as usize];
                steps.push(Step::Wait(800));
                steps.extend(vec![Step::Tap(Key::VolDown, 1); *n as usize]);
                self.keys(rig, &steps)?;
            }
            Action::Dsp(on) => {
                player::command(rig, if *on { "dsp on" } else { "dsp off" }, None, 0)?;
                self.wait(rig, 500)?;
            }
            Action::Eq(v) => {
                let t: Vec<String> = v.iter().map(ToString::to_string).collect();
                player::command(rig, &format!("sound {}", t.join(" ")), None, 0)?;
                self.wait(rig, 500)?;
            }
            Action::NpView(n) => {
                if self.caps.session && self.goto(rig, App::NowPlaying)? {
                    self.keys(rig, &vec![Step::Tap(Key::Down, 1); *n as usize])?;
                    self.wait(rig, 1000)?;
                } else {
                    self.skip("Now Playing not reachable or no test session");
                }
            }
            Action::Listen(s) => self.wait(rig, s * 1000)?,
            Action::Idle(s) => {
                player::command(rig, "stop", None, 0)?;
                if self.radio_on {
                    let _ = self.cmd(rig, "fm off", "^fm", 3000)?;
                    self.radio_on = false;
                }
                if self.caps.screen_off {
                    let _ = self.cmd(rig, "ui screen-off", "^screen-off", 2000)?;
                }
                self.wait(rig, s * 1000)?;
                // Wake: the first key only lights the screen.
                player::tap(rig, Key::Back)?;
                rig.sleep(300);
            }
            Action::Browse { app, steps, seed } => {
                if *app == App::NowPlaying && !self.caps.session {
                    self.skip("Now Playing's views are a setting: no test session");
                } else if self.goto(rig, *app)? {
                    let s = browse_steps(*seed, *steps, self.stress, false);
                    // OK may start a file: a source change.
                    if self.keys(rig, &s)? {
                        disrupted = true;
                    }
                } else {
                    self.skip(&format!("{} not reachable", app.name()));
                }
            }
            Action::Menus { steps, seed } => {
                if self.keys(rig, &menu_steps(*seed, *steps))? {
                    disrupted = true;
                }
            }
            Action::RadioOn(k) => {
                player::command(rig, "stop", None, 0)?;
                let _ = self.cmd(rig, "fm on", "^fm", 3000)?;
                self.radio_on = true;
                self.wait(rig, 2500)?;
                if let Some(k) = k {
                    let _ = self.cmd(rig, &format!("fm tune {k}"), "^fm", 3000)?;
                    self.wait(rig, 1500)?;
                }
            }
            Action::RadioSeek(up) => {
                let _ = self.cmd(rig, if *up { "fm seek up" } else { "fm seek down" }, "^fm", 3000)?;
                self.wait(rig, 4000)?;
            }
            Action::RadioScan => {
                if self.caps.session && self.radio_on {
                    // A finished scan replaces the presets (held in RAM by
                    // the session).
                    let _ = self.cmd(rig, "fm scan", "^fm", 3000)?;
                    self.wait(rig, 30_000)?;
                } else {
                    self.skip("no test session or the radio is off");
                }
            }
            Action::RadioOff => {
                let _ = self.cmd(rig, "fm off", "^fm", 3000)?;
                self.radio_on = false;
                self.wait(rig, 1000)?;
            }
            Action::RadioKeys { steps, seed } => {
                if !self.caps.session {
                    self.skip("the station is a setting: no test session");
                } else if self.goto_radio(rig)? {
                    self.radio_on = true;
                    self.keys(rig, &browse_steps(*seed, *steps, false, true))?;
                } else {
                    self.skip("Radio not reachable");
                }
            }
            Action::Record(s) => {
                if self.recordings >= self.b.max_recordings {
                    self.skip("recording budget used up");
                } else {
                    let src = if self.radio_on { "fm" } else { "mic" };
                    let sub = REC_DIR.rsplit('/').next().unwrap_or("soak");
                    let r = self.cmd(rig, &format!("rec start {src} adpcm {s} {sub}"), "^rec", 3000)?;
                    if r.as_deref().is_some_and(|r| r.contains("starting")) {
                        self.recordings += 1;
                        self.wait(rig, u64::from(*s) * 1000 + 2000)?;
                        for _ in 0..10 {
                            if self
                                .cmd(rig, "rec status", "^rec", 2000)?
                                .is_some_and(|r| !r.starts_with("rec recording"))
                            {
                                break;
                            }
                            rig.sleep(1000);
                        }
                        let _ = self.cmd(rig, "rec stop", "^rec", 2000)?;
                    } else {
                        self.skip(&format!("rec start: {}", r.unwrap_or_else(|| "no reply".into())));
                    }
                }
            }
            Action::Set { id, value } => {
                if !self.set(rig, *id, *value)? {
                    self.skip(&format!("settings test-set {id:#06x} {value} refused"));
                }
                self.wait(rig, 500)?;
            }
            Action::Look { values, steps, seed, secs } => {
                if self.caps.session {
                    self.look(rig, false)?;
                    for (id, v) in values {
                        self.set(rig, *id, *v)?;
                    }
                    rig.sleep(1200);
                    // Keys where the player is (no navigation in the
                    // owner's look).
                    let s = browse_steps(*seed, *steps, self.stress, false);
                    if self.keys(rig, &s)? {
                        disrupted = true;
                    }
                    self.wait(rig, secs * 1000)?;
                    self.look(rig, true)?;
                    if values.iter().any(|(id, _)| *id == LIST_ROLL) {
                        self.lists_clamped = false;
                    }
                } else {
                    self.skip("no test session");
                }
            }
            Action::Clock(level) => {
                match level {
                    Some(l) => {
                        player::command(rig, "gov off", None, 0)?;
                        player::command(rig, &format!("clock {l}"), None, 0)?;
                        self.clock_pinned = true;
                    }
                    None => {
                        player::command(rig, "gov on", None, 0)?;
                        self.clock_pinned = false;
                    }
                }
                self.wait(rig, 500)?;
            }
            Action::Handover(s) => {
                // Read-only: the computer cannot write the card (rule 5).
                rig.switch("usb msc ro")?;
                self.oracles.disrupt(rig.wall_ms(), *s + 10);
                self.wait(rig, s * 1000)?;
                rig.switch("usb dev")?;
                self.oracles.disrupt(rig.wall_ms(), 10);
                self.wait(rig, 4000)?;
            }
            Action::Personality(s) => {
                rig.switch("usb kbd")?;
                self.wait(rig, s * 1000)?;
                rig.switch("usb dev")?;
                self.oracles.disrupt(rig.wall_ms(), 5);
                self.wait(rig, 2000)?;
            }
        }
        if disrupted {
            self.oracles.disrupt(rig.wall_ms().max(t0), a.seconds());
        }
        // The counters are read at the action's end: the next read covers
        // the next action only, so a finding names the action it happened in.
        self.boundary(rig, BOUNDARY_MS)?;
        *self.done.entry(self.kind).or_default() += 1;
        Ok(())
    }

    /// Read the oracles now unless the last read is younger than `min_ms`
    /// (or a finding or a stop is already in): the counters are cumulative
    /// over the time since the last read, so a read at each boundary keeps
    /// what one read covers inside one action (soak seed 1, 2026-11-25: two
    /// underruns read in `radio-keys` over the 28 s since the last sample in
    /// `browse pictures` could have been either action's).
    fn boundary(&mut self, rig: &mut dyn Rig, min_ms: u64) -> Result<()> {
        let last = self.next_sample.saturating_sub(self.b.sample_every_s * 1000);
        if self.findings.is_empty() && self.stopped.is_none() && rig.wall_ms() >= last + min_ms {
            self.sample(rig)?;
        }
        Ok(())
    }

    fn goto_radio(&mut self, rig: &mut dyn Rig) -> Result<bool> {
        if player::top(rig)?.name == "Radio" {
            return Ok(true);
        }
        let Some(k) = self.home(rig)? else { return Ok(false) };
        let n = self.homes.len();
        let (key, times) = if (2 + n - k) % n <= (k + n - 2) % n {
            (Key::Next, (2 + n - k) % n)
        } else {
            (Key::Prev, (k + n - 2) % n)
        };
        for _ in 0..times {
            player::tap(rig, key)?;
        }
        // Opening Radio starts the radio, which stops the music (a source
        // change; the firmware stops the DAC at once, no underrun counted):
        // read what came before, then allow for the change from here.
        self.boundary(rig, 0)?;
        player::tap(rig, Key::Ok)?;
        self.oracles.disrupt(rig.wall_ms(), 0);
        rig.sleep(1100);
        Ok(player::expect_screen(rig, "Radio", 3000).is_ok())
    }

    /// The position moved the way the key asked (same track only).
    fn check_seek(&mut self, a: Option<&Status>, b: Option<&Status>, fwd: Option<bool>, el: u64, t: u64) {
        let (Some(a), Some(b)) = (a, b) else { return };
        let (Some(p0), Some(p1)) = (a.pos_ms, b.pos_ms) else { return };
        if !a.playing || !b.playing || a.title != b.title || a.q_pos != b.q_pos || a.q_len != b.q_len {
            return;
        }
        let bad = match fwd {
            Some(true) => p1 < p0 + el + 1000,
            Some(false) => p1 + 1000 > p0 + el && p1 > 1500,
            None => p1 > el + 3000,
        };
        let what = match fwd {
            Some(true) => "forward",
            Some(false) => "back",
            None => "to the start",
        };
        let detail = format!("seek {what}: position {p0} → {p1} ms in {el} ms on `{}`", a.title);
        self.functional(Kind::Seek, bad, detail, t);
    }

    /// Seek and pause checks: a note per failure, a finding after
    /// `FUNCTIONAL_RUN` failures in a row (a slow seek is not a bug).
    fn functional(&mut self, kind: Kind, bad: bool, detail: String, t: u64) {
        let run = if kind == Kind::Seek { &mut self.seek_fails } else { &mut self.pause_fails };
        if !bad {
            *run = 0;
            return;
        }
        *run += 1;
        if *run >= FUNCTIONAL_RUN {
            *run = 0;
            self.findings.push(Finding { kind, detail, t_ms: t, action: self.action_no });
        } else if self.notes.len() < 50 {
            self.notes.push(format!("{} (#{})", detail, self.action_no));
        }
    }
}

fn take_sample(rig: &mut dyn Rig, ctx: &mut Ctx) -> Result<Sample> {
    let t_ms = rig.wall_ms();
    let fault = rig.fault()?.map(|f| {
        let kind = f.kind().map_or_else(|| format!("kind {:#x}", f.kind), |k| k.name().to_string());
        (f.count, format!("{kind} pc={:#010x} lr={:#010x} {}", f.pc, f.lr, f.text_str()))
    });
    let t_h0 = rig.wall_ms();
    let h = rig.health()?;
    let t_h1 = rig.wall_ms();
    let c = rig.counters()?;
    let st = ctx.status(rig);
    let screen = player::top(rig).map(|t| t.name).unwrap_or_default();
    Ok(Sample {
        t_ms,
        uptime_ms: h.uptime_ms,
        t_up_ms: t_h0 + (t_h1 - t_h0) / 2,
        up_rtt_ms: u32::try_from(t_h1 - t_h0).unwrap_or(u32::MAX),
        stack_free: h.stack_free,
        fault,
        c,
        st,
        screen,
        action: ctx.action_no,
        kind: ctx.kind,
    })
}

// -------------------------------------------------------------- session

pub struct Config {
    pub scenario: Scenario,
    pub seed: u64,
    pub minutes: f64,
    /// Stop after this many actions (tests; the player runs by time).
    pub max_actions: Option<usize>,
    pub minimise: bool,
    pub max_replays: u32,
    pub regress_dir: PathBuf,
    pub folders: Vec<String>,
}

#[derive(Default)]
pub struct Outcome {
    pub history: Vec<Action>,
    pub finding: Option<Finding>,
    pub minimised: Option<Vec<Action>>,
    pub saved: Option<PathBuf>,
}

/// The start of every run and replay: a fresh boot, the test look, a test
/// session (settings in RAM only).
pub fn prologue(rig: &mut dyn Rig) -> Result<Caps> {
    rig.reboot()?;
    if !rig.is_sim() {
        let re = regex::Regex::new("^slot |^not started|^unknown")?;
        let r = player::command(rig, "boot status", Some(&re), 3000).unwrap_or_default();
        if let Some(why) = fallback_start(&r) {
            bail!(
                "the player started by stage 2's fallback ({why}): the firmware under test failed to start; \
                 read the fault record (`cargo xtask dev results`) before soaking"
            );
        }
    }
    let mut caps = Caps::default();
    player::test_look(rig, true)?;
    let re = regex::Regex::new("^test-session|^commands:|^card:")?;
    let r = player::command(rig, "settings test-session on", Some(&re), 3000).unwrap_or_default();
    caps.session = r.starts_with("test-session on");
    let re = regex::Regex::new("^screen-off$")?;
    caps.screen_off = rig.is_sim() || {
        // Asked once with the screen on: the first key wakes it again.
        let ok = player::command(rig, "ui screen-off", Some(&re), 1500).is_ok();
        if ok {
            rig.sleep(1200);
            player::tap(rig, Key::Back)?;
            rig.sleep(300);
        }
        ok
    };
    let re = regex::Regex::new("underruns=")?;
    caps.pos = player::command(rig, "status", Some(&re), 2000).is_ok_and(|r| r.contains(" pos="));
    caps.soak_words = rig.counters()?.soak_words;
    Ok(caps)
}

fn new_ctx(rig: &mut dyn Rig, b: &Budget, caps: Caps, homes: &[Vec<u16>], stress: bool) -> Result<Ctx> {
    let up = rig.health()?.uptime_ms;
    let fault = rig.fault()?.map(|f| f.count);
    let mut c = Ctx::new(b.clone(), caps, homes.to_vec(), up, fault);
    c.stress = stress;
    c.next_sample = rig.wall_ms();
    Ok(c)
}

/// Run actions with the oracles; stops at the first finding.
fn drive(rig: &mut dyn Rig, ctx: &mut Ctx, a: &Action) -> Result<()> {
    match ctx.run(rig, a) {
        Ok(()) => Ok(()),
        Err(e) if e.is::<Locked>() => Err(e),
        Err(e) => {
            log(&format!("   channel lost: {e:#}; waiting for the player"));
            let before = ctx.last_usb;
            let up_before = ctx.last_uptime;
            come_back(rig, ctx)?;
            if let Some(u) = &ctx.last_usb {
                log(&format!("   {}", usb_line(u)));
            }
            let rebooted = ctx.last_uptime < up_before;
            lost_evidence(rig, rebooted);
            if let Some(text) = self_recovery(before.as_ref(), ctx.last_usb.as_ref(), rebooted) {
                log(&format!("   FOUND (usb-recovery; the run goes on): {text}"));
                let t = rig.wall_ms();
                ctx.recoveries.push(Finding {
                    kind: Kind::UsbRecovery,
                    detail: format!("{text}; the request: {e:#}"),
                    t_ms: t,
                    action: ctx.action_no,
                });
                return Ok(());
            }
            if ctx.findings.is_empty() {
                let t = rig.wall_ms();
                ctx.findings.push(Finding {
                    kind: Kind::Reset,
                    detail: format!("channel lost: {e:#}"),
                    t_ms: t,
                    action: ctx.action_no,
                });
            }
            Ok(())
        }
    }
}

/// `boot status` says the running slot was started because another failed
/// (2026-11-25: a build that faulted at every start was soaked as the old
/// build in the other slot without anyone noticing).
fn fallback_start(reply: &str) -> Option<&str> {
    reply.contains(" failed").then_some(reply.trim())
}

/// Where `main` keeps the previous start's last interrupt sample.
const PREV_SAMPLE_EXC: u32 = 0x0013_7FDC;
const PREV_SAMPLE_PC: u32 = 0x0013_7FE0;

/// The function holding `pc` in the firmware ELF, demangled, with the
/// offset.
fn symbol_at(elf: &std::path::Path, pc: u32) -> Option<String> {
    use object::{Object, ObjectSymbol, SymbolKind};
    let data = std::fs::read(elf).ok()?;
    let file = object::File::parse(&*data).ok()?;
    let pc = u64::from(pc & !1);
    file.symbols()
        .filter(|s| s.kind() == SymbolKind::Text && s.address() <= pc && pc < s.address() + s.size().max(1))
        .max_by_key(|s| s.address())
        .and_then(|s| {
            Some(format!("{:#} +{:#x}", rustc_demangle::demangle(s.name().ok()?), pc - s.address()))
        })
}

/// After a channel loss: what the player shows and its last log lines
/// (before anything else changes them), and a screenshot under
/// `target/orpheus/soak/`. Best effort: nothing here fails the run.
fn lost_evidence(rig: &mut dyn Rig, rebooted: bool) {
    if let Ok(t) = player::top(rig) {
        log(&format!(
            "   screen: {} (depth {}, dialog {}, screen on {})",
            t.name, t.depth, t.dialog, t.screen_on
        ));
    }
    if !rig.is_sim()
        && let Ok(fb) = rig.frame()
        && let Ok(png) = super::mask::png_frame(&fb)
    {
        let path = out_dir().join("soak").join(format!("lost-{}.png", stamp()));
        if std::fs::create_dir_all(path.parent().unwrap_or(&path)).is_ok()
            && std::fs::write(&path, png).is_ok()
        {
            log(&format!("   screenshot: {}", path.display()));
        }
    }
    if rebooted
        && let (Ok(Some(exc)), Ok(Some(pc))) = (rig.peek_word(PREV_SAMPLE_EXC), rig.peek_word(PREV_SAMPLE_PC))
    {
        // The previous start's last interrupt taken from handler mode
        // (`rt::SAMPLE_*`, copied at start): after a watchdog reset, where
        // the RT executor was spinning.
        let at = symbol_at(&root().join("target/orpheus/firmware.elf"), pc).unwrap_or_else(|| "?".into());
        log(&format!("   last interrupt before the reset: exception {exc}, PC {pc:#010x} in {at}"));
    }
    if let Ok(lines) = rig.log_lines() {
        let tail = lines.len().saturating_sub(8);
        let shown: Vec<&String> = if rebooted {
            // The previous start's last lines (the ring survives resets).
            let at = lines.iter().rposition(|l| l.as_bytes() == orpheus_proto::logring::BOOT_MARK);
            at.map_or_else(Vec::new, |at| lines[at.saturating_sub(10)..at].iter().collect())
        } else {
            lines[tail..].iter().collect()
        };
        for l in shown {
            log(&format!("   log: {l}"));
        }
    }
}

/// Reconnect and take a sample. A player whose USB recovers by itself
/// (USB-W1: soft detach, controller reset) re-enumerates a few seconds
/// after the failure, and the old device may still be listed at first: a
/// few bounded tries, not a blind retry loop (CLAUDE.md rule 7).
fn come_back(rig: &mut dyn Rig, ctx: &mut Ctx) -> Result<()> {
    const TRIES: u32 = 4;
    let mut n = 1;
    loop {
        rig.reconnect()?;
        match ctx.sample(rig) {
            Ok(()) => return Ok(()),
            Err(e) if n < TRIES => {
                n += 1;
                log(&format!("   not answering yet ({e:#}); trying again in 3 s"));
                rig.sleep(3000);
            }
            Err(e) => return Err(e),
        }
    }
}

/// Does `actions` (from a fresh boot) give a finding of `kind`?
pub fn reproduces(
    rig: &mut dyn Rig,
    actions: &[Action],
    kind: Kind,
    b: &Budget,
    homes: &[Vec<u16>],
    stress: bool,
) -> Result<bool> {
    let caps = prologue(rig)?;
    let mut ctx = new_ctx(rig, b, caps, homes, stress)?;
    ctx.keep = false;
    ctx.sample(rig)?;
    for (i, a) in actions.iter().enumerate() {
        ctx.action_no = i;
        drive(rig, &mut ctx, a)?;
        if !ctx.findings.is_empty() {
            break;
        }
    }
    if ctx.findings.is_empty() {
        ctx.wait(rig, 3000)?;
        ctx.sample(rig)?;
    }
    Ok(ctx.findings.iter().any(|f| same_kind(f.kind, kind)))
}

/// The shortest reproducing tail (doubling), then ddmin, at most `budget`
/// replays. `None`: it did not reproduce.
pub fn minimise(
    rig: &mut dyn Rig,
    history: &[Action],
    kind: Kind,
    b: &Budget,
    homes: &[Vec<u16>],
    stress: bool,
    budget: &mut u32,
) -> Result<Option<Vec<Action>>> {
    let mut tail = 2usize;
    let start = loop {
        let from = history.len().saturating_sub(tail);
        if *budget == 0 {
            return Ok(None);
        }
        *budget -= 1;
        log(&format!("   replay: last {} actions", history.len() - from));
        if reproduces(rig, &history[from..], kind, b, homes, stress)? {
            break from;
        }
        if from == 0 {
            return Ok(None);
        }
        tail *= 2;
    };
    let mut test = |s: &[Action]| -> Result<bool> {
        log(&format!("   ddmin: {} actions", s.len()));
        reproduces(rig, s, kind, b, homes, stress)
    };
    Ok(Some(fuzz::ddmin(history[start..].to_vec(), &mut test, budget)?))
}

/// A sequence as a `.soak` file.
pub fn soak_text(header: &[String], actions: &[Action]) -> String {
    let mut t: String = header.iter().map(|h| format!("# {h}\n")).collect();
    for a in actions {
        t.push_str(&a.text());
        t.push('\n');
    }
    t
}

pub fn read_soak(path: &Path) -> Result<(Option<Scenario>, Vec<Action>)> {
    let t = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut sc = None;
    let mut v = Vec::new();
    for (i, l) in t.lines().enumerate() {
        let l = l.trim();
        if let Some(h) = l.strip_prefix('#') {
            if let Some(s) = h.split_whitespace().find_map(|w| w.strip_prefix("scenario=")) {
                sc = Scenario::parse(s);
            }
            continue;
        }
        if l.is_empty() {
            continue;
        }
        v.push(Action::parse(l).with_context(|| format!("{}:{}", path.display(), i + 1))?);
    }
    Ok((sc, v))
}

/// One soak: generate, run, sample, check; on a finding replay and
/// minimise. The caller does the prologue, the after-run checks and the
/// final reboot.
pub fn session(rig: &mut dyn Rig, ctx: &mut Ctx, c: &Config, homes: &[Vec<u16>]) -> Result<Outcome> {
    let mut plan = Plan::new(c.scenario, c.seed, c.folders.clone());
    let mut out = Outcome::default();
    let t0 = rig.wall_ms();
    let end = t0 + (c.minutes * 60_000.0) as u64;
    ctx.sample(rig)?;
    loop {
        if rig.wall_ms() >= end || c.max_actions.is_some_and(|m| out.history.len() >= m) {
            break;
        }
        let a = plan.next();
        ctx.action_no = out.history.len();
        if out.history.len() < 400 || out.history.len().is_multiple_of(25) {
            log(&format!(
                "[{:>6.1} min] #{} {}",
                (rig.wall_ms() - t0) as f64 / 60_000.0,
                ctx.action_no,
                a.text()
            ));
        }
        out.history.push(a.clone());
        let text = a.text();
        super::prof::hook(rig.is_sim(), super::prof::Phase::Before(ctx.action_no, &text));
        drive(rig, ctx, &a)?;
        super::prof::hook(rig.is_sim(), super::prof::Phase::After(ctx.action_no, &text));
        if ctx.findings.is_empty() && rig.wall_ms() >= ctx.next_sample {
            ctx.sample(rig)?;
        }
        if !ctx.findings.is_empty() || ctx.stopped.is_some() {
            break;
        }
    }
    if ctx.findings.is_empty() {
        ctx.sample(rig)?;
    }
    // A profile ends here, before any replay reboots the player (T-8).
    super::prof::hook(rig.is_sim(), super::prof::Phase::End);
    if let Some(f) = ctx.findings.first().cloned() {
        let history = out.history.clone();
        let mut header = vec![
            format!(
                "soak scenario={} seed={} oracle={}: {}",
                c.scenario.name(),
                c.seed,
                f.kind.name(),
                f.detail
            ),
            format!("{} actions before the finding", history.len()),
            "replay: cargo xtask dev soak --replay <this file> [--sim]".into(),
        ];
        let mut actions = history.clone();
        if c.minimise && !f.kind.after_run() {
            let mut budget = c.max_replays;
            match minimise(rig, &history, f.kind, &ctx.b, homes, c.scenario == Scenario::Stress, &mut budget)?
            {
                Some(m) => {
                    header.push(format!(
                        "minimised to {} actions ({} replays)",
                        m.len(),
                        c.max_replays - budget
                    ));
                    out.minimised = Some(m.clone());
                    actions = m;
                }
                None => header.push("did not reproduce on replay: the whole session is kept".into()),
            }
        }
        std::fs::create_dir_all(&c.regress_dir)?;
        let path = c.regress_dir.join(format!(
            "{}-{}-seed{}-{}.soak",
            c.scenario.name(),
            f.kind.name(),
            c.seed,
            actions.len()
        ));
        std::fs::write(&path, soak_text(&header, &actions))?;
        log(&format!("   saved {}", path.display()));
        out.saved = Some(path);
        out.finding = Some(f);
    }
    Ok(out)
}

// ----------------------------------------------------------- discovery

/// The firmware's test command length (`audio::CMD_LEN`).
const CMD_LEN: usize = 120;

const AUDIO: [&str; 9] = ["mp3", "flac", "ogg", "oga", "opus", "wav", "spx", "m4a", "aac"];

/// Folders with at least two audio files: the fixtures, then the card's
/// (breadth first from `/`, at most `max_ls` listings, `max` folders).
pub fn discover(rig: &mut dyn Rig, max: usize, max_ls: usize) -> Result<Vec<String>> {
    let re = regex::Regex::new(r"entries\)$|no such directory|card: in use")?;
    let mut out = Vec::new();
    let mut queue: std::collections::VecDeque<(String, u32)> = std::collections::VecDeque::new();
    for f in [FIXTURES.to_string(), format!("{}/music", super::suite::FIXTURES)] {
        queue.push_back((f, 9));
    }
    queue.push_back(("/".into(), 0));
    let mut listed = 0;
    while let Some((dir, depth)) = queue.pop_front() {
        if listed >= max_ls || out.len() >= max {
            break;
        }
        listed += 1;
        let t = rig.wall_ms();
        let got = player::command(rig, &format!("ls {dir}"), Some(&re), 15_000);
        let took = rig.wall_ms().saturating_sub(t);
        if took > 3_000 || got.is_err() {
            // Slow or failed listings are where a start-up hang would show.
            log(&format!("   ls {dir}: {} after {took} ms", if got.is_ok() { "ok" } else { "no answer" }));
        }
        let Ok(r) = got else { continue };
        if !r.ends_with("entries)") {
            continue;
        }
        let names: Vec<&str> = r.lines().filter(|l| !l.starts_with('(') && !l.is_empty()).collect();
        let audio = names
            .iter()
            .filter(|n| {
                n.rsplit_once('.').is_some_and(|(_, e)| AUDIO.contains(&e.to_ascii_lowercase().as_str()))
            })
            .count();
        // `playdir PATH` must fit the firmware's 120-byte command.
        if audio >= 2 && "playdir ".len() + dir.len() <= CMD_LEN {
            out.push(dir.clone());
        }
        for n in names.iter().filter_map(|n| n.strip_suffix('/')) {
            let hidden = n.starts_with('.') || n.eq_ignore_ascii_case("System Volume Information");
            let p = if dir == "/" { format!("/{n}") } else { format!("{dir}/{n}") };
            if "playdir ".len() + p.len() > CMD_LEN {
                continue;
            }
            if depth < 4 && !hidden {
                queue.push_back((p, depth + 1));
            } else if depth == 9 && !n.starts_with('.') {
                // The soak's own fixture sets (gapless parts per codec).
                queue.push_front((p, 9));
            }
        }
    }
    Ok(out)
}

/// `--setup`: 44.1 and 48 kHz tones (sample-rate changes between tracks),
/// and the gapless corpus if `cargo xtask test codecs --gen` made it.
pub fn fixtures() -> Vec<(String, Vec<u8>)> {
    let mut v = vec![
        (format!("{FIXTURES}/tones/01 Tone 440.wav"), super::suite::wav_tone(440.0, 20.0)),
        (format!("{FIXTURES}/tones/02 Tone 660 48k.wav"), wav_tone_at(660.0, 12.0, 48_000)),
        (format!("{FIXTURES}/tones/03 Tone 880.wav"), super::suite::wav_tone(880.0, 8.0)),
    ];
    let dir = root().join("tests/corpus/gapless");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|r| r.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_file()).collect())
        .unwrap_or_default();
    files.sort();
    for f in files {
        let (Some(name), Some(data)) = (f.file_name().and_then(|n| n.to_str()), std::fs::read(&f).ok())
        else {
            continue;
        };
        let codec = name.split('_').next().unwrap_or("x");
        v.push((format!("{FIXTURES}/gapless-{codec}/{name}"), data));
    }
    v
}

fn wav_tone_at(hz: f32, secs: f32, rate: u32) -> Vec<u8> {
    let mut w = super::suite::wav_tone(hz * 44_100.0 / rate as f32, secs * rate as f32 / 44_100.0);
    w[24..28].copy_from_slice(&rate.to_le_bytes());
    w[28..32].copy_from_slice(&(rate * 2).to_le_bytes());
    w
}

// --------------------------------------------------------------- report

fn csv_header() -> String {
    let mut h = String::from("t_s,uptime_s,action,kind,screen,playing,rate,pos_ms,q,underruns");
    for u in UB {
        let _ = write!(h, ",ub_{u}");
    }
    h.push_str(",loads_per_s,evictions_per_s,loads,evictions,code_faults,ro_faults,idle_pct,clock_level,clock_switches,stack_free,head_min,card_errors,load_errors,battery_mv,ring_halves");
    h
}

pub fn csv(samples: &[Sample], derived: &[[f64; 4]]) -> String {
    let mut t = csv_header();
    t.push('\n');
    for (i, s) in samples.iter().enumerate() {
        let d = derived.get(i).copied().unwrap_or_default();
        let st = s.st.clone().unwrap_or_default();
        let _ = write!(
            t,
            "{:.1},{:.1},{},{},{},{},{},{},{}/{},{}",
            s.t_ms as f64 / 1000.0,
            f64::from(s.uptime_ms) / 1000.0,
            s.action,
            s.kind,
            s.screen,
            u8::from(st.playing),
            st.rate,
            st.pos_ms.map(|p| p.to_string()).unwrap_or_default(),
            st.q_pos,
            st.q_len,
            st.underruns
        );
        for u in st.ub {
            let _ = write!(t, ",{u}");
        }
        let _ = writeln!(
            t,
            ",{:.0},{:.0},{},{},{},{},{:.1},{},{},{},{},{},{},{},{:.0}",
            d[0],
            d[1],
            s.c.loads,
            s.c.evictions,
            s.c.code_faults,
            s.c.ro_faults,
            d[2],
            s.c.clock_level,
            s.c.clock_switches,
            s.stack_free,
            s.c.head_min.map(|h| h.to_string()).unwrap_or_default(),
            s.c.card_errors,
            s.c.load_errors,
            s.c.battery_mv,
            d[3]
        );
    }
    t
}

/// A line chart as inline SVG (no scripts, no CDN).
fn svg_chart(title: &str, xs: &[f64], ys: &[Option<f64>], marks: &[f64], limit: Option<f64>) -> String {
    let (w, h, l, r, top, bot) = (760.0, 150.0, 56.0, 12.0, 22.0, 22.0);
    let pts: Vec<(f64, f64)> = xs.iter().zip(ys).filter_map(|(x, y)| y.map(|y| (*x, y))).collect();
    let mut s = format!(
        "<svg viewBox=\"0 0 {w} {h}\" width=\"{w}\" height=\"{h}\" role=\"img\" aria-label=\"{title}\"><text x=\"{l}\" y=\"15\" class=\"t\">{title}</text>"
    );
    if pts.is_empty() {
        s.push_str("<text x=\"60\" y=\"80\" class=\"a\">no data</text></svg>");
        return s;
    }
    let x0 = xs.first().copied().unwrap_or(0.0);
    let x1 = xs.last().copied().unwrap_or(1.0).max(x0 + 1.0);
    let mut y0 = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let mut y1 = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    if let Some(lim) = limit {
        y0 = y0.min(lim);
        y1 = y1.max(lim);
    }
    if (y1 - y0).abs() < 1e-9 {
        y1 = y0 + 1.0;
    }
    let px = |x: f64| l + (x - x0) / (x1 - x0) * (w - l - r);
    let py = |y: f64| top + (1.0 - (y - y0) / (y1 - y0)) * (h - top - bot);
    let _ = write!(
        s,
        "<line x1=\"{l}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" class=\"g\" /><text x=\"{}\" y=\"{}\" class=\"a\" text-anchor=\"end\">{y1:.0}</text><text x=\"{}\" y=\"{}\" class=\"a\" text-anchor=\"end\">{y0:.0}</text><text x=\"{l}\" y=\"{}\" class=\"a\">{:.0} min</text><text x=\"{}\" y=\"{}\" class=\"a\" text-anchor=\"end\">{:.0} min</text>",
        h - bot,
        w - r,
        h - bot,
        l - 4.0,
        top + 8.0,
        l - 4.0,
        h - bot,
        h - 6.0,
        x0,
        w - r,
        h - 6.0,
        x1
    );
    if let Some(lim) = limit {
        let _ = write!(
            s,
            "<line x1=\"{l}\" y1=\"{0:.1}\" x2=\"{1}\" y2=\"{0:.1}\" class=\"lim\" />",
            py(lim),
            w - r
        );
    }
    for m in marks {
        let _ = write!(
            s,
            "<line x1=\"{0:.1}\" y1=\"{top}\" x2=\"{0:.1}\" y2=\"{1}\" class=\"bad\" />",
            px(*m),
            h - bot
        );
    }
    let poly: Vec<String> = pts.iter().map(|(x, y)| format!("{:.1},{:.1}", px(*x), py(*y))).collect();
    let _ = write!(s, "<polyline points=\"{}\" class=\"p\" /></svg>", poly.join(" "));
    s
}

pub struct Report<'a> {
    pub scenario: Scenario,
    pub seed: u64,
    pub player: &'a str,
    pub minutes: f64,
    pub ctx: &'a Ctx,
    pub findings: &'a [Finding],
    pub after: &'a [String],
    pub actions: usize,
    pub folders: &'a [String],
}

impl Report<'_> {
    pub fn pass(&self) -> bool {
        self.findings.is_empty()
    }

    pub fn text(&self) -> String {
        let st = &self.ctx.oracles.stats;
        let b = &self.ctx.b;
        let mut t = String::new();
        let _ = writeln!(
            t,
            "Orpheus soak `{}` seed {} ({}): {}, {:.1} min, {} actions, {} samples",
            self.scenario.name(),
            self.seed,
            self.player,
            if self.pass() { "PASS" } else { "FAIL" },
            self.minutes,
            self.actions,
            st.samples
        );
        for f in self.findings {
            let _ = writeln!(
                t,
                "FAIL {} at {:.1} min (action #{}): {}",
                f.kind.name(),
                f.t_ms as f64 / 60_000.0,
                f.action,
                f.detail
            );
        }
        let _ = writeln!(t, "\noracle                      measured            limit");
        let row = |t: &mut String, k: &str, m: String, l: String| {
            let _ = writeln!(t, "{k:<27} {m:<19} {l}");
        };
        row(
            &mut t,
            "stack free min (B)",
            st.stack_free_min.map_or("-".into(), |v| v.to_string()),
            format!("≥ {}", b.stack_free_min),
        );
        row(
            &mut t,
            "line loads/s mean",
            format!("{:.0}", st.loads_per_s_mean()),
            format!("≤ {:.0}", b.loads_per_s_mean_max),
        );
        row(
            &mut t,
            "line loads/s max",
            format!("{:.0}", st.loads_per_s_max),
            format!("≤ {:.0} for < {} samples", b.loads_per_s_max, b.thrash_samples),
        );
        row(
            &mut t,
            "evictions/s max",
            format!("{:.0}", st.evictions_per_s_max),
            format!("≤ {:.0}", b.evictions_per_s_max),
        );
        row(
            &mut t,
            "underruns (in disruptions)",
            format!("{} ({})", st.underruns_total, st.underruns_in_disruptions),
            format!("0 outside, ≤ {} per window", b.underruns_per_disruption_max),
        );
        row(
            &mut t,
            "DAC headroom min (frames)",
            st.head_min.map_or("-".into(), |v| v.to_string()),
            format!("≥ {}", b.head_min_frames),
        );
        row(&mut t, "card I/O errors", st.card_errors.to_string(), format!("≤ {}", b.card_errors_max));
        row(
            &mut t,
            "DAC vs host clock",
            st.drift.map_or("-".into(), |(s, p)| format!("{p:+.0} ppm / {s:.0} s")),
            format!("±{:.0} ppm (≥ {} s)", b.drift_ppm_max, b.drift_min_window_s),
        );
        row(
            &mut t,
            "uptime vs host clock",
            st.clock_ppm.map_or("-".into(), |p| format!("{p:+.0} ppm")),
            format!("±{:.0} ppm", b.clock_ppm_max),
        );
        row(&mut t, "CPU idle mean", format!("{:.0} %", st.idle_pct_mean()), "-".into());
        row(
            &mut t,
            "battery min (mV)",
            st.battery_mv_min.map_or("-".into(), |v| v.to_string()),
            format!("--battery stops at {}", b.battery_min_mv),
        );
        let _ = writeln!(t, "\nactions run: {}", fmt_counts(&self.ctx.done));
        if !self.ctx.skipped.is_empty() {
            let _ = writeln!(t, "skipped: {}", fmt_counts(&self.ctx.skipped));
        }
        let _ = writeln!(
            t,
            "firmware: test session {}, screen-off {}, status pos {}, VER-12 stats words {}",
            yes(self.ctx.caps.session),
            yes(self.ctx.caps.screen_off),
            yes(self.ctx.caps.pos),
            yes(self.ctx.caps.soak_words)
        );
        let _ = writeln!(t, "folders: {}", self.folders.join(" | "));
        for n in self.after.iter().chain(&self.ctx.notes) {
            let _ = writeln!(t, "note: {n}");
        }
        for f in &self.ctx.recoveries {
            let _ = writeln!(
                t,
                "usb-recovery at {:.0} s (action #{}): {}",
                f.t_ms as f64 / 1000.0,
                f.action,
                f.detail
            );
        }
        t
    }

    pub fn write(&self, dir: &Path, history: &[Action]) -> Result<String> {
        std::fs::create_dir_all(dir)?;
        let text = self.text();
        std::fs::write(dir.join("summary.txt"), &text)?;
        let data = csv(&self.ctx.samples, &self.ctx.oracles.derived);
        std::fs::write(dir.join("samples.csv"), &data)?;
        std::fs::write(
            dir.join("actions.soak"),
            soak_text(
                &[format!("soak scenario={} seed={} (whole session)", self.scenario.name(), self.seed)],
                history,
            ),
        )?;
        let st = &self.ctx.oracles.stats;
        let json = serde_json::json!({
            "scenario": self.scenario.name(),
            "seed": self.seed,
            "player": self.player,
            "pass": self.pass(),
            "minutes": self.minutes,
            "actions": self.actions,
            "samples": st.samples,
            "findings": self.findings.iter().map(|f| serde_json::json!({
                "kind": f.kind.name(), "detail": f.detail, "t_s": f.t_ms as f64 / 1000.0, "action": f.action,
            })).collect::<Vec<_>>(),
            "stack_free_min": st.stack_free_min,
            "loads_per_s_mean": st.loads_per_s_mean(),
            "loads_per_s_max": st.loads_per_s_max,
            "evictions_per_s_max": st.evictions_per_s_max,
            "underruns_total": st.underruns_total,
            "underruns_in_disruptions": st.underruns_in_disruptions,
            "head_min": st.head_min,
            "card_errors": st.card_errors,
            "drift": st.drift.map(|(s, p)| serde_json::json!({"seconds": s, "ppm": p})),
            "clock_ppm": st.clock_ppm,
            "idle_pct_mean": st.idle_pct_mean(),
            "battery_mv_min": st.battery_mv_min,
            "done": self.ctx.done,
            "skipped": self.ctx.skipped,
            "notes": self.after.iter().chain(&self.ctx.notes).collect::<Vec<_>>(),
            "usb_recoveries": self.ctx.recoveries.iter().map(|f| serde_json::json!({
                "detail": f.detail, "t_s": f.t_ms as f64 / 1000.0, "action": f.action,
            })).collect::<Vec<_>>(),
        });
        std::fs::write(dir.join("summary.json"), serde_json::to_string_pretty(&json)?)?;
        std::fs::write(dir.join("index.html"), self.html(&text))?;
        Ok(text)
    }

    fn html(&self, text: &str) -> String {
        let s = &self.ctx.samples;
        let d = &self.ctx.oracles.derived;
        let xs: Vec<f64> = s.iter().map(|x| x.t_ms as f64 / 60_000.0).collect();
        let marks: Vec<f64> = self.findings.iter().map(|f| f.t_ms as f64 / 60_000.0).collect();
        let b = &self.ctx.b;
        let col = |i: usize| -> Vec<Option<f64>> {
            (0..s.len()).map(|k| (k > 0).then(|| d.get(k).map_or(0.0, |v| v[i]))).collect()
        };
        let mut under = 0u64;
        let mut prev: Option<&Status> = None;
        let cum: Vec<Option<f64>> = s
            .iter()
            .map(|x| {
                if let (Some(a), Some(b)) = (prev, x.st.as_ref()) {
                    under += if a.title == b.title && b.underruns >= a.underruns {
                        b.underruns - a.underruns
                    } else {
                        b.underruns
                    };
                }
                prev = x.st.as_ref();
                Some(under as f64)
            })
            .collect();
        let charts = [
            svg_chart("pager line loads/s", &xs, &col(0), &marks, Some(b.loads_per_s_max)),
            svg_chart("pager evictions/s", &xs, &col(1), &marks, Some(b.evictions_per_s_max)),
            svg_chart(
                "stack free (B)",
                &xs,
                &s.iter().map(|x| Some(f64::from(x.stack_free))).collect::<Vec<_>>(),
                &marks,
                Some(f64::from(b.stack_free_min)),
            ),
            svg_chart("underruns (cumulative)", &xs, &cum, &marks, None),
            svg_chart(
                "DAC headroom low-water (frames)",
                &xs,
                &s.iter().map(|x| x.c.head_min.map(f64::from)).collect::<Vec<_>>(),
                &marks,
                None,
            ),
            svg_chart("PCM ring fill (halves)", &xs, &col(3), &marks, None),
            svg_chart("CPU idle (%)", &xs, &col(2), &marks, None),
            svg_chart(
                "clock level",
                &xs,
                &s.iter().map(|x| Some(f64::from(x.c.clock_level))).collect::<Vec<_>>(),
                &marks,
                None,
            ),
            svg_chart(
                "battery (mV)",
                &xs,
                &s.iter()
                    .map(|x| (x.c.battery_mv != 0).then_some(f64::from(x.c.battery_mv)))
                    .collect::<Vec<_>>(),
                &marks,
                None,
            ),
        ];
        let mut h = String::from(
            "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\"><title>Orpheus soak</title>\
             <style>:root{--bg:#1d1f23;--fg:#dde;--dim:#99a;--line:#6cf;--bad:#f66;--grid:#445}\
             @media (prefers-color-scheme: light){:root{--bg:#fff;--fg:#222;--dim:#667;--line:#0569c2;--bad:#c22;--grid:#ccd}}\
             body{font:14px system-ui;background:var(--bg);color:var(--fg);margin:16px;max-width:800px}\
             pre{white-space:pre-wrap}svg{max-width:100%;height:auto;display:block;margin:8px 0}\
             .t{fill:var(--fg);font-size:13px}.a{fill:var(--dim);font-size:11px}.g{stroke:var(--grid)}\
             .p{fill:none;stroke:var(--line);stroke-width:1.5}.lim{stroke:var(--dim);stroke-dasharray:4 3}.bad{stroke:var(--bad)}</style>",
        );
        let _ = write!(
            h,
            "<h1>Soak {} seed {}: {}</h1><pre>{}</pre>",
            self.scenario.name(),
            self.seed,
            if self.pass() { "pass" } else { "FAIL" },
            esc(text)
        );
        for c in charts {
            h.push_str(&c);
        }
        h.push_str("<p>Dashed: budget. Red lines: findings. Data: samples.csv; actions: actions.soak.</p>");
        h
    }
}

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

fn fmt_counts(m: &BTreeMap<&'static str, u32>) -> String {
    m.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn stamp() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (y, mo, d, h, mi, s) = super::civil(now);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

pub const HISTORY_HEADER: &str = "date,commit,scenario,seed,minutes,actions,samples,pass,findings,loads_per_s_mean,\
                                  loads_per_s_max,evictions_per_s_max,stack_free_min,underruns_total,\
                                  underruns_in_disruptions,head_min,card_errors,drift_ppm,clock_ppm,idle_pct_mean";

/// `metrics/soak/history.csv` (VER-10 history): one row per player run;
/// returns the previous row of the same scenario, if any.
fn history_row(r: &Report<'_>, stamp: &str) -> Result<Option<String>> {
    let dir = root().join("metrics/soak");
    std::fs::create_dir_all(&dir)?;
    let p = dir.join("history.csv");
    let old = std::fs::read_to_string(&p).unwrap_or_default();
    let prev = old.lines().rfind(|l| l.split(',').nth(2) == Some(r.scenario.name())).map(str::to_string);
    let st = &r.ctx.oracles.stats;
    let row = format!(
        "{stamp},{},{},{},{:.1},{},{},{},{},{:.0},{:.0},{:.0},{},{},{},{},{},{},{},{:.1}",
        crate::util::git_commit(),
        r.scenario.name(),
        r.seed,
        r.minutes,
        r.actions,
        st.samples,
        u8::from(r.pass()),
        r.findings.iter().map(|f| f.kind.name()).collect::<Vec<_>>().join("+"),
        st.loads_per_s_mean(),
        st.loads_per_s_max,
        st.evictions_per_s_max,
        st.stack_free_min.map_or(String::new(), |v| v.to_string()),
        st.underruns_total,
        st.underruns_in_disruptions,
        st.head_min.map_or(String::new(), |v| v.to_string()),
        st.card_errors,
        st.drift.map_or(String::new(), |(_, p)| format!("{p:.0}")),
        st.clock_ppm.map_or(String::new(), |p| format!("{p:.0}")),
        st.idle_pct_mean()
    );
    let mut text = if old.is_empty() { format!("{HISTORY_HEADER}\n") } else { old };
    text.push_str(&row);
    text.push('\n');
    std::fs::write(&p, text)?;
    Ok(prev)
}

// ------------------------------------------------------------- command

pub const HELP: &str = "\
cargo xtask dev soak <scenario> [--hours H | --minutes M] [--seed N] [--sim] [--report DIR]
                     [--setup] [--battery] [--music DIR]... [--no-minimise] [--max-replays R]
cargo xtask dev soak --replay FILE [--sim]
cargo xtask dev soak --list
  scenarios: music, radio, browse-while-playing, settings-churn, mixed, stress";

/// An error after the prologue: reboot first (CLAUDE.md rule 4), unless the
/// player does not answer (rule 7: then the user is asked).
fn rebooted(rig: &mut dyn Rig, e: anyhow::Error) -> anyhow::Error {
    if rig.health().is_ok() {
        let _ = fuzz::reset_ui(rig);
        let _ = player::command(rig, "stop", None, 0);
        if let Err(r) = rig.reboot() {
            return e.context(format!("and the reboot failed: {r:#}"));
        }
    }
    e
}

fn settings_status(rig: &mut dyn Rig) -> Option<String> {
    let re = regex::Regex::new("^test-session").ok()?;
    player::command(rig, "settings test-session", Some(&re), 3000).ok().map(|r| {
        r.split_whitespace()
            .filter(|w| w.starts_with("store=") || w.starts_with("keys=") || w.starts_with("crc="))
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// The player's state files in `/.orpheus/` (queue, resume record), as
/// `ls` lists them; `None` without a listing (the card busy or absent).
fn state_files(rig: &mut dyn Rig) -> Option<Vec<String>> {
    let re = regex::Regex::new(r"entries\)$|no such directory|card: in use").ok()?;
    // The card mounts and the state loads after a boot: a few tries.
    for _ in 0..15 {
        if let Ok(r) = player::command(rig, "ls /.orpheus", Some(&re), 15_000)
            && r.ends_with("entries)")
        {
            let mut v: Vec<String> =
                r.lines().filter(|l| *l == "queue.bin" || *l == "player.bin").map(str::to_string).collect();
            v.sort();
            return Some(v);
        }
        rig.sleep(1000);
    }
    None
}

/// Everything after the actions: clean up, the card, the settings store
/// and the queue across the final reboot (CLAUDE.md rule 4).
fn epilogue(
    rig: &mut dyn Rig,
    ctx: &mut Ctx,
    before: Option<&Vec<String>>,
    store0: Option<&String>,
    battery: bool,
) -> Vec<Finding> {
    let mut out = Vec::new();
    let t = rig.wall_ms();
    let action = ctx.action_no;
    let mut f = |kind: Kind, detail: String| out.push(Finding { kind, detail, t_ms: t, action });
    let _ = ctx.exec.release_all(rig);
    let _ = fuzz::reset_ui(rig);
    let _ = player::command(rig, "rec stop", None, 0);
    if ctx.radio_on {
        let _ = player::command(rig, "fm off", None, 0);
    }
    let _ = player::command(rig, "stop", None, 0);
    if ctx.clock_pinned {
        let _ = player::command(rig, "gov on", None, 0);
    }
    rig.sleep(1500);
    let q0 = state_files(rig);
    match (before, fuzz::root_listing(rig)) {
        (Some(a), Ok(Some(b))) if *a != b => {
            let new: Vec<&String> = b.iter().filter(|x| !a.contains(x)).collect();
            let gone: Vec<&String> = a.iter().filter(|x| !b.contains(x)).collect();
            f(Kind::Card, format!("card root changed: new {new:?}, gone {gone:?}"));
        }
        (Some(_), Ok(Some(_))) => {}
        _ => ctx.notes.push("card root listing not compared (no listing)".into()),
    }
    if battery {
        let _ = player::command(rig, "drain stop", None, 0);
        let _ = player::command(rig, "charge resume", None, 0);
    }
    let _ = player::test_look(rig, false);
    if let Err(e) = rig.reboot() {
        f(Kind::Reset, format!("the player did not come back after the final reboot: {e:#}"));
        return out;
    }
    // The settings store as the boot read it, before and after.
    match (store0, settings_status(rig)) {
        (Some(a), Some(b)) if a != &b => {
            f(Kind::Settings, format!("settings store {a} before, {b} after the run"));
        }
        (Some(_), Some(b)) if b.starts_with("store=none") => {
            f(Kind::Settings, "the settings store did not mount".into());
        }
        (Some(_), Some(_)) => {}
        _ => ctx.notes.push("settings store not compared (firmware without `settings test-session`)".into()),
    }
    // The queue and the resume record are still there after the boot
    // (their content is not read back: no test command reads them, and
    // the restored queue is published only once something plays).
    if let Some(a) = q0.filter(|_| !rig.is_sim()) {
        match state_files(rig) {
            Some(b) if b == a => {}
            b => f(Kind::Queue, format!("player state files {a:?} before the reboot, {b:?} after")),
        }
    }
    out
}

pub fn cmd(args: &mut Args) -> Result<String> {
    if args.flag("list") {
        let mut t = String::new();
        for s in Scenario::ALL {
            let w = weights(s);
            let total: u32 = w.iter().map(|x| x.1).sum();
            let _ = writeln!(
                t,
                "{:<22} {:>5.0} min  {}",
                s.name(),
                s.default_minutes(),
                w.iter().map(|(p, n)| format!("{p:?} {n}")).collect::<Vec<_>>().join(", ")
            );
            let _ = total;
        }
        print!("{t}");
        return Ok(format!("{} scenarios", Scenario::ALL.len()));
    }
    let b = Budget::load()?;
    let sim = args.flag("sim");
    let homes = super::suite::home_frames("")?;
    let mut usb;
    let mut simr;
    let rig: &mut dyn Rig = if sim {
        simr = SoakSim::default();
        &mut simr
    } else {
        usb = UsbPlayer::open()?;
        &mut usb
    };
    let _ = rig.wall_ms();
    if let Some(file) = args.value("replay") {
        let (sc, actions) = read_soak(Path::new(&file))?;
        let stress = sc == Some(Scenario::Stress);
        let ran = |rig: &mut dyn Rig| -> Result<Ctx> {
            let caps = prologue(rig)?;
            // `dev prof --during "soak --replay …"` (T-8): no-ops otherwise.
            super::prof::hook(rig.is_sim(), super::prof::Phase::Start);
            let mut ctx = new_ctx(rig, &b, caps, &homes, stress)?;
            ctx.sample(rig)?;
            for (i, a) in actions.iter().enumerate() {
                ctx.action_no = i;
                let text = a.text();
                log(&format!("#{i} {text}"));
                super::prof::hook(rig.is_sim(), super::prof::Phase::Before(i, &text));
                drive(rig, &mut ctx, a)?;
                super::prof::hook(rig.is_sim(), super::prof::Phase::After(i, &text));
                if !ctx.findings.is_empty() {
                    break;
                }
            }
            if ctx.findings.is_empty() {
                ctx.wait(rig, 3000)?;
                ctx.sample(rig)?;
            }
            Ok(ctx)
        };
        let r = ran(rig);
        super::prof::hook(rig.is_sim(), super::prof::Phase::End);
        let ctx = r.map_err(|e| rebooted(rig, e))?;
        let _ = fuzz::reset_ui(rig);
        let _ = player::command(rig, "stop", None, 0);
        rig.reboot()?;
        return match ctx.findings.first() {
            Some(f) => bail!("{file}: {} ({})", f.kind.name(), f.detail),
            None => Ok(format!("{file}: {} actions, no finding", actions.len())),
        };
    }
    let hours = args.value("hours");
    let mins = args.value("minutes");
    let seed = args.value("seed").map(|s| num(&s)).transpose()?.map_or(1, u64::from);
    let battery = args.flag("battery");
    let setup = args.flag("setup");
    let no_minimise = args.flag("no-minimise");
    let max_replays = args.value("max-replays").map(|s| num(&s)).transpose()?.unwrap_or(8);
    let report = args.value("report");
    let mut folders = Vec::new();
    while let Some(m) = args.value("music") {
        folders.push(m);
    }
    let name = args.next_positional().ok_or_else(|| anyhow!("{HELP}"))?;
    let scenario = Scenario::parse(&name).ok_or_else(|| anyhow!("unknown scenario `{name}`\n{HELP}"))?;
    let minutes = match (hours, mins) {
        (Some(h), _) => h.parse::<f64>().context("--hours")? * 60.0,
        (None, Some(m)) => m.parse().context("--minutes")?,
        (None, None) if sim => 10.0,
        (None, None) => scenario.default_minutes(),
    };
    let report_dir = report.map_or_else(
        || out_dir().join("soak").join(format!("{}-{}-seed{seed}", scenario.name(), stamp())),
        PathBuf::from,
    );
    if setup && !sim {
        for (remote, data) in fixtures() {
            if !remote.starts_with(&format!("{FIXTURES}/")) {
                bail!("fixture {remote} outside {FIXTURES}");
            }
            log(&format!("   upload {remote} ({} bytes)", data.len()));
            rig.put(&remote, &data)?;
        }
    }
    let c = Config {
        scenario,
        seed,
        minutes,
        max_actions: None,
        minimise: !no_minimise,
        max_replays,
        regress_dir: root().join("tests/regress/soak"),
        folders: Vec::new(),
    };
    log(&format!(
        "soak {}: seed {seed}, {minutes:.0} min, oracles every {} s",
        scenario.name(),
        b.sample_every_s
    ));
    // A fresh boot, then the card and the settings store as they are.
    let caps = prologue(rig).map_err(|e| rebooted(rig, e))?;
    super::prof::hook(rig.is_sim(), super::prof::Phase::Start);
    if !caps.session && !sim {
        let e = anyhow!(
            "the firmware has no `settings test-session` (VER-12): stage a build with the soak hooks first"
        );
        return Err(rebooted(rig, e));
    }
    let store0 = settings_status(rig);
    player::command(rig, "stop", None, 0).map_err(|e| rebooted(rig, e))?;
    rig.sleep(500);
    let before = fuzz::root_listing(rig).map_err(|e| rebooted(rig, e))?;
    if folders.is_empty() {
        folders = discover(rig, 16, 40).map_err(|e| rebooted(rig, e))?;
    }
    log(&format!("   {} folders: {}", folders.len(), folders.join(" | ")));
    if battery {
        // On the battery with the cable in: charging paused, the logger
        // recording and keeping auto-off away (PWR-5).
        player::command(rig, "charge pause", None, 0).map_err(|e| rebooted(rig, e))?;
        player::command(rig, "drain start soak stay", None, 0).map_err(|e| rebooted(rig, e))?;
    }
    let mut ctx =
        new_ctx(rig, &b, caps, &homes, scenario == Scenario::Stress).map_err(|e| rebooted(rig, e))?;
    if battery {
        ctx.battery_stop = Some(b.battery_min_mv);
    }
    let c = Config { folders: folders.clone(), ..c };
    let r = session(rig, &mut ctx, &c, &homes);
    let mut findings = ctx.findings.clone();
    if r.is_ok() {
        findings.extend(ctx.oracles.finish(rig.wall_ms(), ctx.action_no));
    }
    let mut after = Vec::new();
    if let Ok(o) = &r
        && let Some(p) = &o.saved
    {
        after.push(format!("finding saved to {}", p.display()));
    }
    let ran_min = rig.wall_ms() as f64 / 60_000.0;
    // A player that did not come back is left alone (CLAUDE.md rule 7:
    // stop and ask the user, no blind retries).
    if let Err(e) = &r
        && rig.health().is_err()
    {
        bail!("{e:#}\nthe player does not answer: stop and ask the user (CLAUDE.md rule 7)");
    }
    let tail = epilogue(rig, &mut ctx, before.as_ref(), store0.as_ref(), battery);
    findings.extend(tail);
    let o = r?;
    let rep = Report {
        scenario,
        seed,
        player: if sim { "simulator" } else { "player" },
        minutes: ran_min,
        ctx: &ctx,
        findings: &findings,
        after: &after,
        actions: o.history.len(),
        folders: &folders,
    };
    let text = rep.write(&report_dir, &o.history)?;
    if !sim {
        let st = stamp();
        let m = root().join("metrics/soak");
        std::fs::create_dir_all(&m)?;
        std::fs::write(
            m.join(format!("{}-{st}-seed{seed}.csv", scenario.name())),
            csv(&ctx.samples, &ctx.oracles.derived),
        )?;
        if let Some(prev) = history_row(&rep, &st)? {
            log(&format!("   previous {} run: {prev}", scenario.name()));
        }
    }
    print!("{text}");
    if !rep.pass() {
        bail!("{} finding(s); report in {}", findings.len(), report_dir.display());
    }
    Ok(format!("soak {} passed; report in {}", scenario.name(), report_dir.display()))
}

#[cfg(test)]
mod tests {
    /// Regression (2026-11-25): a soak ran on the fallback slot's old
    /// build because the build under test faulted at every start.
    #[test]
    fn regress_soak_refuses_a_fallback_start() {
        assert!(super::fallback_start("slot B, start 1, after A failed").is_some());
        assert!(super::fallback_start("slot A, start 1").is_none());
        assert!(super::fallback_start("not started by stage 2").is_none());
    }

    use super::*;

    fn budget() -> Budget {
        Budget::load().unwrap()
    }

    #[test]
    fn budgets_have_a_soak_section() {
        let b = budget();
        assert_eq!(b.stack_free_min, 2048);
        assert!(b.loads_per_s_mean_max >= 3200.0, "measured ≈ 3,200 loads/s during 48 kHz MP3");
        assert!(b.loads_per_s_max > b.loads_per_s_mean_max);
    }

    /// Home positions follow the grid (Games before Settings since GAME-2:
    /// Settings moved from 10 to 11; Books since BOOK-1/2: 12); `browse
    /// games` parses.
    #[test]
    fn browse_targets_on_home() {
        assert_eq!(App::Settings.home(), (12, "Settings"));
        assert_eq!(App::Games.home(), (10, "Games"));
        assert_eq!(App::Pictures.home().0, 5);
        let a = Action::parse("browse games 20 3").unwrap();
        assert_eq!(a, Action::Browse { app: App::Games, steps: 20, seed: 3 });
        assert_eq!(a.text(), "browse games 20 3");
    }

    #[test]
    fn scenarios_parse_and_have_weights() {
        for s in Scenario::ALL {
            assert_eq!(Scenario::parse(s.name()), Some(s));
            assert!(weights(s).iter().all(|w| w.1 > 0));
        }
        assert_eq!(Scenario::parse("browse"), Some(Scenario::Browse));
    }

    fn folders() -> Vec<String> {
        vec!["/Music/Sigur Rós/Takk…".into(), "/.orpheus/test/suite/music".into()]
    }

    #[test]
    fn generation_is_deterministic_per_seed() {
        for s in Scenario::ALL {
            let mut a = Plan::new(s, 7, folders());
            let mut b = Plan::new(s, 7, folders());
            let mut c = Plan::new(s, 8, folders());
            let xa: Vec<Action> = (0..500).map(|_| a.next()).collect();
            let xb: Vec<Action> = (0..500).map(|_| b.next()).collect();
            let xc: Vec<Action> = (0..500).map(|_| c.next()).collect();
            assert_eq!(xa, xb, "{}", s.name());
            assert_ne!(xa, xc, "{}", s.name());
            // Every action survives its text form (`.soak` files).
            for x in &xa {
                assert_eq!(&Action::parse(&x.text()).unwrap(), x, "{}", x.text());
            }
            // The steps inside an action are deterministic too.
            assert_eq!(browse_steps(5, 40, false, false), browse_steps(5, 40, false, false));
        }
    }

    #[test]
    fn generated_actions_are_device_safe() {
        for s in Scenario::ALL {
            let mut p = Plan::new(s, 3, folders());
            for _ in 0..3000 {
                match p.next() {
                    Action::Set { id, value } => {
                        assert!(session_settable(id), "{id:#06x}");
                        assert!(valid_values(id).contains(&value), "{id:#06x}={value}");
                    }
                    Action::Look { values, .. } => {
                        for (id, v) in values {
                            assert!(session_settable(id) && valid_values(id).contains(&v), "{id:#06x}={v}");
                        }
                    }
                    Action::Eq(v) => {
                        assert!(v[..6].iter().all(|g| (-6..=6).contains(g)) && (0..=3).contains(&v[6]));
                    }
                    Action::RadioOn(Some(k)) => assert!((87_500..=108_000).contains(&k)),
                    Action::Volume(n) => assert!(n <= 3),
                    _ => {}
                }
            }
            for st in browse_steps(11, 400, s == Scenario::Stress, false)
                .iter()
                .chain(&browse_steps(12, 400, false, true))
            {
                assert!(!script::step_text(st).contains("power"));
            }
        }
        // The firmware's whitelist is the same function.
        // (Compared with the whitespace folded: rustfmt may break it.)
        let fw = std::fs::read_to_string(root().join("fw/firmware/src/ui.rs")).unwrap();
        let fw: String = fw.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(fw.contains(
            "(matches!(id >> 8, 0x01 | 0x02 | 0x03 | 0x0A) && id != 0x030E) || matches!(id, 0x0D01..=0x0D03 | 0x1001..=0x100D)"
        ));
        assert!(!session_settable(0x0601) && !session_settable(0x0901) && !session_settable(0x0402));
    }

    #[test]
    fn status_and_stats_parse() {
        let r = "playing frames=1234 rate=48000 underruns=2 corrupt=0 where=3 written=90 played=88 walk=1/2 d0 pos=61000 q=3/12 shuf=0 title=Hoppípolla live join#4: head=900 open_us=- loads=1 u=0 lib_us=- art_us=- u_meta=- pf_us=- prep_us=- prep_loads=- dec: n=5 us=6 max_us=7 loads=8 ub: join=0 fill=1 serve=0 pf=0 prep=0 lib=0 art=0 dec=1";
        let s = parse_status(r).unwrap();
        assert!(s.playing);
        assert_eq!(
            (s.rate, s.underruns, s.played, s.pos_ms, s.q_pos, s.q_len),
            (48_000, 2, 88, Some(61_000), 3, 12)
        );
        assert_eq!(s.title, "Hoppípolla live");
        assert_eq!(s.ub, [0, 1, 0, 0, 0, 0, 0, 1]);
        assert!(parse_status("card: in use by USB").is_none());
        let mut w = vec![0u32; 24];
        w[4] = 100;
        w[8] = 5000;
        w[21] = i32::MAX as u32;
        w[22] = 3;
        let c = Counters::from_words(&w, 3900);
        assert_eq!(
            (c.loads, c.uptime_ms, c.head_min, c.card_errors, c.soak_words),
            (100, 5000, None, 3, true)
        );
        assert!(!Counters::from_words(&w[..21], 0).soak_words);
        w[21] = (-12i32) as u32;
        assert_eq!(Counters::from_words(&w, 0).head_min, Some(-12));
        assert_eq!(Counters::from_words(&w, 0).usb, None);
    }

    #[test]
    fn usb_counters_and_self_recovery() {
        use orpheus_proto::devchan::usb as u;
        let mut w = vec![0u32; u::WORDS];
        w[u::W_SETUPS] = 120;
        w[u::W_RESETS] = 2;
        w[u::W_ABORTS] = 1;
        w[u::W_REINITS] = 0;
        w[u::W_STATE] = 1;
        let before = Counters::from_words(&w, 0).usb.unwrap();
        assert_eq!((before.setups, before.resets, before.aborts), (120, 2, 1));
        w[u::W_REINITS] = 1;
        w[u::W_SETUPS] = 130;
        w[u::W_LAST_ERROR] = u32::from(u::ERR_RECOVER_SETUP) | 300 << 8;
        let after = Counters::from_words(&w, 0).usb.unwrap();
        let line = usb_line(&after);
        assert!(line.contains("re-inits 1") && line.contains("last error recover-setup at 300 s"), "{line}");
        let r = self_recovery(Some(&before), Some(&after), false).unwrap();
        assert!(r.contains("1 re-init,") && r.contains("recover-setup"), "{r}");
        // A reboot, no new re-init, or old firmware: not a self-recovery.
        assert!(self_recovery(Some(&before), Some(&after), true).is_none());
        assert!(self_recovery(Some(&after), Some(&after), false).is_none());
        assert!(self_recovery(None, Some(&after), false).is_none());
        assert_eq!(Kind::UsbRecovery.name(), "usb-recovery");
    }

    /// A healthy sample stream at `t` seconds: 48 kHz playing, 3,000
    /// loads/s, stack free 5,000 B.
    fn good(t: u64) -> Sample {
        let up = 10_000 + u32::try_from(t * 1000).unwrap();
        let played = t * 48_000 / HALF_FRAMES;
        Sample {
            t_ms: t * 1000,
            uptime_ms: up,
            stack_free: 5000,
            c: Counters {
                uptime_ms: up,
                loads: u32::try_from(t * 3000).unwrap(),
                evictions: u32::try_from(t * 2900).unwrap(),
                idle_us: u32::try_from(t * 500_000).unwrap(),
                head_min: Some(1500),
                ..Counters::default()
            },
            st: Some(Status {
                playing: true,
                rate: 48_000,
                played,
                written: played + 1,
                title: "a".into(),
                q_pos: 1,
                q_len: 9,
                ..Status::default()
            }),
            ..Sample::default()
        }
    }

    fn run(o: &mut Oracles, v: &[Sample]) -> Vec<Finding> {
        v.iter().flat_map(|s| o.check(s, 0)).collect()
    }

    /// Regression (2026-11-25): a sample stamped before a request the
    /// starved player answered 1.6 s late read as +1,613 ppm. The uptime
    /// is stamped at the request's middle and slow replies are left out.
    #[test]
    fn regress_late_reply_is_not_clock_drift() {
        let mut o = Oracles::new(budget(), 10_000, None);
        let mut v: Vec<Sample> = (0..100).map(|i| good(i * 10)).collect();
        // A starved reply: the player answered 1.6 s after the stamp.
        let mut late = good(1000);
        late.uptime_ms += 1600;
        late.t_up_ms = 1000 * 1000 + 800;
        late.up_rtt_ms = 1600;
        v.push(late);
        let f = run(&mut o, &v);
        assert!(!f.iter().any(|f| f.kind == Kind::Clock), "{f:?}");
        assert!(o.stats.clock_ppm.is_some_and(|p| p.abs() < 50.0));
    }

    #[test]
    fn oracles_pass_a_healthy_run() {
        let mut o = Oracles::new(budget(), 10_000, None);
        let v: Vec<Sample> = (0..400).map(|i| good(i * 10)).collect();
        assert!(run(&mut o, &v).is_empty());
        assert!(o.finish(4_000_000, 0).is_empty());
        let st = &o.stats;
        assert!((st.loads_per_s_mean() - 3000.0).abs() < 1.0);
        let (span, ppm) = st.drift.unwrap();
        assert!(span > 3900.0 && ppm.abs() < 50.0, "{span} {ppm}");
        assert!((st.idle_pct_mean() - 50.0).abs() < 0.1);
    }

    #[test]
    fn oracles_trip_on_bad_data() {
        let b = budget();
        let trip = |f: &dyn Fn(u64, &mut Sample)| -> Vec<Kind> {
            let mut o = Oracles::new(budget(), 10_000, None);
            let v: Vec<Sample> = (0..100)
                .map(|i| {
                    let mut s = good(i * 10);
                    f(i, &mut s);
                    s
                })
                .collect();
            let mut k: Vec<Kind> = run(&mut o, &v).into_iter().map(|f| f.kind).collect();
            k.extend(o.finish(1_000_000, 0).into_iter().map(|f| f.kind));
            k
        };
        assert_eq!(
            trip(&|i, s| if i == 50 {
                s.fault = Some((1, "hard fault".into()));
            }),
            [Kind::Fault]
        );
        // A reset: the uptime starts again (and the clock comparison too).
        let k = trip(&|i, s| {
            if i >= 50 {
                s.uptime_ms = u32::try_from((i - 50) * 10_000 + 100).unwrap();
                s.c.uptime_ms = s.uptime_ms;
            }
        });
        assert_eq!(k, [Kind::Reset]);
        assert_eq!(
            trip(&|i, s| if i == 60 {
                s.stack_free = b.stack_free_min - 1;
            }),
            [Kind::Stack]
        );
        let u = trip(&|i, s| {
            if i >= 70 {
                let st = s.st.as_mut().unwrap();
                st.underruns = 3;
                st.ub[1] = 3;
            }
        });
        assert_eq!(u, [Kind::Underrun]);
        // A load storm for long enough is thrash; one spike is not.
        let lps = b.loads_per_s_max as u64 + 1000;
        let k = trip(&|i, s| {
            if i >= 40 {
                s.c.loads = u32::try_from(400 * 3000 + (i - 40) * 10 * lps).unwrap();
            }
        });
        assert!(k.contains(&Kind::Thrash), "{k:?}");
        assert!(
            trip(&|i, s| if i >= 40 {
                s.c.loads += 10 * (lps as u32);
            })
            .is_empty()
        );
        assert_eq!(
            trip(&|i, s| if i >= 30 {
                s.c.card_errors = 1;
            }),
            [Kind::CardIo]
        );
        assert_eq!(
            trip(&|i, s| if i == 30 {
                s.c.head_min = Some(-5);
            }),
            [Kind::Ring]
        );
        // A DAC 3,000 ppm fast over 1,000 s.
        let k = trip(&|i, s| {
            let st = s.st.as_mut().unwrap();
            st.played = i * 10 * 48_144 / HALF_FRAMES;
        });
        assert_eq!(k, [Kind::Drift]);
        // The device clock 1,000 ppm slow.
        let k = trip(&|i, s| {
            s.uptime_ms = 10_000 + u32::try_from(i * 9990).unwrap();
            s.c.uptime_ms = s.uptime_ms;
        });
        assert!(k.contains(&Kind::Clock), "{k:?}");
    }

    #[test]
    fn underruns_inside_a_seek_window_are_allowed_up_to_the_budget() {
        let b = budget();
        let mut o = Oracles::new(b.clone(), 10_000, None);
        let v: Vec<Sample> = (0..10).map(|i| good(i * 10)).collect();
        assert!(run(&mut o, &v).is_empty());
        o.disrupt(95_000, 0);
        let mut s = good(100);
        s.st.as_mut().unwrap().underruns = b.underruns_per_disruption_max;
        assert!(o.check(&s, 1).is_empty());
        assert_eq!(o.stats.underruns_in_disruptions, b.underruns_per_disruption_max);
        let mut s2 = good(100 + b.seek_window_s * 10);
        s2.st.as_mut().unwrap().underruns = b.underruns_per_disruption_max + 1;
        assert_eq!(o.check(&s2, 2).len(), 1);
        // A new track starts its counter from 0.
        let mut s3 = good(300);
        s3.st.as_mut().unwrap().title = "b".into();
        assert!(o.check(&s3, 3).is_empty());
    }

    /// VER-12 finding, 2026-11-25 (`tests/regress/soak/mixed-ring-seed1-7.soak`):
    /// the headroom low-water read in `radio-keys` covered the 76 s since
    /// the last sample in `browse pictures`; the finding did not say so and
    /// read as a dip inside the radio action. It names that span now. A dip
    /// read inside a disruption window is allowed, outside one it is not
    /// (the radio's hand-over stops the DAC first, so a dip while music
    /// still plays is steady playback).
    #[test]
    fn regress_ring_finding_names_the_span_it_covers() {
        let mut o = Oracles::new(budget(), 10_000, None);
        for i in 0..5 {
            assert!(o.check(&good(i * 10), 5).is_empty());
        }
        let mut s = good(116);
        s.t_ms += 200;
        s.c.head_min = Some(-13);
        let f = o.check(&s, 6);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, Kind::Ring);
        assert!(f[0].detail.ends_with("; over the 76.2 s since the sample in action #5"), "{}", f[0].detail);
        // In the same action the span is still named.
        let mut s = good(126);
        s.c.head_min = Some(-1);
        let f = o.check(&s, 6);
        assert!(f[0].detail.ends_with("; over the 9.8 s since the last sample"), "{}", f[0].detail);
        // Inside a disruption window: allowed.
        o.disrupt(127_000, 0);
        let mut s = good(130);
        s.c.head_min = Some(-200);
        assert!(o.check(&s, 7).is_empty());
        assert_eq!(o.stats.head_min, Some(-200));
    }

    #[test]
    fn minimiser_reduces_to_the_two_actions() {
        // Fails when `dsp off` and later `eq` both occur.
        let has = |s: &[Action]| {
            let i = s.iter().position(|a| *a == Action::Dsp(false));
            i.is_some_and(|i| s[i..].iter().any(|a| matches!(a, Action::Eq(_))))
        };
        let mut p = Plan::new(Scenario::Music, 21, folders());
        let mut input: Vec<Action> =
            (0..60).map(|_| p.next()).filter(|a| !has(std::slice::from_ref(a))).collect();
        input.retain(|a| !matches!(a, Action::Dsp(false) | Action::Eq(_)));
        input.insert(17, Action::Dsp(false));
        input.insert(41, Action::Eq([1, 2, 3, 4, 5, 6, 1]));
        let mut budget = 500;
        let m = fuzz::ddmin(input, &mut |s| Ok(has(s)), &mut budget).unwrap();
        assert_eq!(m, [Action::Dsp(false), Action::Eq([1, 2, 3, 4, 5, 6, 1])]);
    }

    fn sim_rig() -> (SoakSim, Vec<Vec<u16>>) {
        (SoakSim::default(), crate::dev::suite::home_frames("").unwrap())
    }

    /// Regression: the soak's way to Now Playing opened Music › Shuffle all
    /// (VER-12, 2026-11-25, third soak; on the owner's library Shuffle all
    /// then reset the player): with UI-8's list roll-around on, the default,
    /// its held Up wrapped past Music's first row. The session turns
    /// roll-around off before it navigates there (`Ctx::clamp_lists`).
    #[test]
    fn regress_soak_now_playing_does_not_open_shuffle_all() {
        let b = budget();
        let (mut rig, homes) = sim_rig();
        let caps = prologue(&mut rig).unwrap();
        let mut ctx = new_ctx(&mut rig, &b, caps, &homes, false).unwrap();
        let roll = orpheus_apps::settings::SettingId(LIST_ROLL);
        assert_eq!(rig.p.sim.sys.settings.get(roll), 1, "roll-around on, as on the player");
        ctx.run(&mut rig, &Action::PlayDir(folders()[0].clone())).unwrap();
        let before = ctx.status(&mut rig).expect("status");
        assert!(before.q_len > 0, "{before:?}");
        assert!(ctx.goto(&mut rig, App::NowPlaying).unwrap());
        let after = ctx.status(&mut rig).expect("status");
        assert_eq!((after.q_len, &after.title), (before.q_len, &before.title), "the queue was replaced");
        assert_eq!(rig.p.sim.sys.settings.get(roll), 0);
    }

    /// Soak seed 1, 2026-11-25 (`tests/regress/soak/mixed-underrun-seed1-4.soak`):
    /// two underruns were read in `radio-keys` over the 28 s since the last
    /// sample in `browse pictures`, so the run could not tell which action
    /// had them (the radio's start stops the music cleanly; the navigation
    /// before it and the end of `browse pictures` both played music). Every
    /// action is now read at its end, and the radio's entry just before it
    /// stops the music, after which its disruption window opens.
    #[test]
    fn regress_underruns_are_read_in_the_action_they_happen_in() {
        let b = budget();
        let (mut rig, homes) = sim_rig();
        let caps = prologue(&mut rig).unwrap();
        let mut ctx = new_ctx(&mut rig, &b, caps, &homes, false).unwrap();
        ctx.run(&mut rig, &Action::PlayDir(folders()[0].clone())).unwrap();
        // Past the play's disruption window.
        ctx.action_no = 1;
        ctx.run(&mut rig, &Action::Listen(b.seek_window_s + 15)).unwrap();
        assert!(ctx.findings.is_empty(), "{:?}", ctx.findings);
        // Underruns from now on (each status read adds one), in an action
        // too short for a sample of its own.
        rig.planted = Some(Planted { after: vec![], ..Planted::default() });
        ctx.action_no = 2;
        ctx.run(&mut rig, &Action::Listen(3)).unwrap();
        let f = ctx.findings.first().expect("read at the action's end");
        assert_eq!((f.kind, f.action), (Kind::Underrun, 2), "{f:?}");
        assert!(f.detail.contains("since the sample in action #1"), "{}", f.detail);

        // The radio's entry: read just before the music stops, the window
        // opened right after.
        let (mut rig, homes) = sim_rig();
        let caps = prologue(&mut rig).unwrap();
        let mut ctx = new_ctx(&mut rig, &b, caps, &homes, false).unwrap();
        ctx.run(&mut rig, &Action::PlayDir(folders()[0].clone())).unwrap();
        ctx.run(&mut rig, &Action::Listen(b.seek_window_s + 15)).unwrap();
        let (n, t) = (ctx.oracles.stats.samples, rig.wall_ms());
        assert!(t > ctx.oracles.disrupt_until, "outside any window");
        assert!(ctx.goto_radio(&mut rig).unwrap());
        assert_eq!(ctx.oracles.stats.samples, n + 1, "read before the radio starts");
        assert!(ctx.oracles.disrupt_until >= t + b.seek_window_s * 1000, "window from the entry");
        assert!(ctx.findings.is_empty(), "{:?}", ctx.findings);
    }

    /// The tool against the simulator: every scenario runs for a few
    /// minutes of the simulator's time without a finding, navigates where
    /// it means to, and never asks for anything harmful.
    #[test]
    fn scenarios_run_clean_on_the_simulator() {
        let b = budget();
        for s in Scenario::ALL {
            let (mut rig, homes) = sim_rig();
            let caps = prologue(&mut rig).unwrap();
            assert!(caps.session && caps.screen_off && caps.pos && caps.soak_words);
            let mut ctx = new_ctx(&mut rig, &b, caps, &homes, s == Scenario::Stress).unwrap();
            let c = Config {
                scenario: s,
                seed: 2,
                minutes: 6.0,
                max_actions: Some(40),
                minimise: false,
                max_replays: 0,
                regress_dir: std::env::temp_dir(),
                folders: folders(),
            };
            let o = session(&mut rig, &mut ctx, &c, &homes).unwrap();
            assert!(ctx.findings.is_empty(), "{}: {:?}", s.name(), ctx.findings);
            assert!(!o.history.is_empty() && ctx.samples.len() >= 2, "{}", s.name());
            assert!(ctx.oracles.stats.samples >= 2);
            assert_eq!(ctx.nav_fail, 0, "{}: Home's focus not found", s.name());
            let done: u32 = ctx.done.values().sum();
            let skipped: u32 = ctx.skipped.values().sum();
            assert!(skipped * 3 <= done, "{}: {skipped} of {done} skipped: {:?}", s.name(), ctx.skipped);
            for l in &rig.p.sim.sys.log {
                assert!(
                    !matches!(
                        l,
                        orpheus_sim::apps::Logged::Power(_)
                            | orpheus_sim::apps::Logged::Storage(_)
                            | orpheus_sim::apps::Logged::FactoryReset
                    ),
                    "{}: {l:?}",
                    s.name()
                );
            }
            assert!(!rig.p.sim.shell.gate.locked);
            for t in &rig.sent {
                assert!(!t.starts_with("bt ") && !t.starts_with("usb msc ") || t == "usb msc ro", "{t}");
            }
            // The report and its plot are written.
            let dir = std::env::temp_dir().join(format!("orpheus-soak-{}-{}", s.name(), std::process::id()));
            let rep = Report {
                scenario: s,
                seed: 2,
                player: "simulator",
                minutes: 6.0,
                ctx: &ctx,
                findings: &[],
                after: &[],
                actions: o.history.len(),
                folders: &c.folders,
            };
            let t = rep.write(&dir, &o.history).unwrap();
            assert!(t.contains("PASS"), "{t}");
            let html = std::fs::read_to_string(dir.join("index.html")).unwrap();
            assert!(html.contains("<polyline") && !html.contains("http"));
            let csv = std::fs::read_to_string(dir.join("samples.csv")).unwrap();
            assert_eq!(csv.lines().count(), ctx.samples.len() + 1);
            let (_, back) = read_soak(&dir.join("actions.soak")).unwrap();
            assert_eq!(back, o.history);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// A bug planted in the simulator (underruns once a folder plays) is
    /// found by a session, replayed, minimised and saved.
    #[test]
    fn a_planted_bug_is_found_and_saved() {
        let b = budget();
        let (mut rig, homes) = sim_rig();
        let caps = prologue(&mut rig).unwrap();
        rig.planted = Some(Planted { after: vec!["playdir"], ..Planted::default() });
        let mut ctx = new_ctx(&mut rig, &b, caps, &homes, false).unwrap();
        let dir = std::env::temp_dir().join(format!("orpheus-soak-regress-{}", std::process::id()));
        let c = Config {
            scenario: Scenario::Music,
            seed: 5,
            minutes: 120.0,
            max_actions: Some(60),
            minimise: true,
            max_replays: 12,
            regress_dir: dir.clone(),
            folders: folders(),
        };
        let o = session(&mut rig, &mut ctx, &c, &homes).unwrap();
        let f = o.finding.expect("the planted bug is found");
        assert_eq!(f.kind, Kind::Underrun, "{f:?}");
        let m = o.minimised.expect("minimised");
        assert!(m.len() <= 2 && matches!(m[0], Action::PlayDir(_)), "{m:?}");
        let (_, back) = read_soak(o.saved.as_ref().unwrap()).unwrap();
        assert_eq!(back, m);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The minimiser on the simulator: underruns after `dsp off` and a
    /// later `sound …`; the sequence shrinks to those two actions.
    #[test]
    fn minimiser_reduces_on_the_simulator() {
        let b = budget();
        let (mut rig, homes) = sim_rig();
        rig.planted = Some(Planted { after: vec!["dsp off", "sound "], ..Planted::default() });
        let mut history = Vec::new();
        for i in 0..20u32 {
            history.push(match i % 4 {
                0 => Action::Volume(1),
                1 => Action::Dsp(true),
                2 => Action::Listen(2),
                _ => Action::Set { id: 0x0103, value: (i % 5) as i16 },
            });
        }
        history.insert(5, Action::Dsp(false));
        history.insert(15, Action::Eq([1, 0, -1, 2, 0, 3, 1]));
        assert!(reproduces(&mut rig, &history, Kind::Underrun, &b, &homes, false).unwrap());
        let mut budget = 80;
        let m =
            minimise(&mut rig, &history, Kind::Underrun, &b, &homes, false, &mut budget).unwrap().unwrap();
        assert_eq!(m, [Action::Dsp(false), Action::Eq([1, 0, -1, 2, 0, 3, 1])]);
    }

    #[test]
    fn regressions_replay_in_the_simulator() {
        // Every committed soak finding runs in the simulator too.
        let b = budget();
        let dir = root().join("tests/regress/soak");
        for e in
            std::fs::read_dir(&dir).map(|r| r.filter_map(Result::ok).collect()).unwrap_or_else(|_| Vec::new())
        {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "soak") {
                let (sc, actions) = read_soak(&p).unwrap();
                let (mut rig, homes) = sim_rig();
                let r = reproduces(&mut rig, &actions, Kind::Fault, &b, &homes, sc == Some(Scenario::Stress));
                assert!(r.is_ok(), "{}: {r:?}", p.display());
            }
        }
    }

    #[test]
    fn fixtures_stay_under_the_soak_folder() {
        for (p, d) in fixtures() {
            assert!(p.starts_with("/.orpheus/test/soak/"), "{p}");
            assert!(!d.is_empty());
        }
        let w = wav_tone_at(660.0, 0.5, 48_000);
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 48_000);
        assert!(REC_DIR.starts_with("/.orpheus/"));
    }
}
