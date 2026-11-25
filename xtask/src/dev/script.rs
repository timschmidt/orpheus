//! The simulator's script language on the player (VER-7, 07 §5.7).
//!
//! Journeys and fuzz sequences are written in the language of
//! `orpheus_sim::Sim::run_script`, so one journey runs unchanged in the
//! simulator and on the player. On the player only what keys and the
//! developer channel can do is allowed:
//!
//! | Command | Player |
//! |---|---|
//! | `ok`, `down*3` | taps: key down, 60 ms, key up, 40 ms (as the simulator) |
//! | `hold:menu`, `hold:back:1200` | key down, 700 ms (or the given ms), key up, 40 ms |
//! | `press:next`, `release:next` | key edges |
//! | `wait:300` | sleep |
//! | `shot:name` | screenshot (once the frame is stable), compared with the simulator's frame |
//! | `expect-screen:Name` | `ui top` until the top screen is `Name` (2 s) |
//!
//! `set:`, `goto:`, `theme:`, `accent:`, `lang:` and `expect-typed:` change
//! the simulator's fake services: they belong in `sim:` lines of a journey.
//! The Power key is refused everywhere (CLAUDE.md rule 4: an automated test
//! never powers the player off).

use anyhow::{Context, Result, anyhow, bail};
use orpheus_ui::Key;

/// One device-executable command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Tap(Key, u32),
    Hold(Key, u64),
    Press(Key),
    Release(Key),
    Wait(u64),
    Shot(String),
    Expect(String),
}

/// Default hold in the simulator (`hold:k` without a time; Power excepted).
pub const HOLD_MS: u64 = 700;
/// Tap timing of the simulator (`Sim::tap`).
pub const TAP_DOWN_MS: u64 = 60;
pub const TAP_UP_MS: u64 = 40;

fn key(name: &str) -> Result<Key> {
    let k = Key::from_name(name).ok_or_else(|| anyhow!("unknown key `{name}`"))?;
    if k == Key::Power {
        bail!("the Power key is never pressed by an automated test (CLAUDE.md rule 4)");
    }
    Ok(k)
}

/// Commands that only the simulator understands.
const SIM_ONLY: [&str; 6] = ["set", "goto", "theme", "accent", "lang", "expect-typed"];

/// Parse one script command (as `Sim::command` reads it).
pub fn parse_step(tok: &str) -> Result<Step> {
    if let Some((op, arg)) = tok.split_once(':') {
        return Ok(match op {
            "hold" => match arg.split_once(':') {
                Some((k, ms)) => {
                    Step::Hold(key(k)?, ms.parse().with_context(|| format!("hold time in `{tok}`"))?)
                }
                None => Step::Hold(key(arg)?, HOLD_MS),
            },
            "press" => Step::Press(key(arg)?),
            "release" => Step::Release(key(arg)?),
            "wait" => Step::Wait(arg.parse().with_context(|| format!("wait time in `{tok}`"))?),
            "shot" if !arg.is_empty() => Step::Shot(arg.to_string()),
            "expect-screen" if !arg.is_empty() => Step::Expect(arg.to_string()),
            o if SIM_ONLY.contains(&o) => {
                bail!("`{tok}` changes the simulator's services: put it in a `sim:` line")
            }
            _ => bail!("unknown command `{tok}`"),
        });
    }
    let (name, n) = match tok.split_once('*') {
        Some((k, n)) => (k, n.parse::<u32>().with_context(|| format!("count in `{tok}`"))?),
        None => (tok, 1),
    };
    Ok(Step::Tap(key(name)?, n))
}

/// Split like `Sim::run_script`: whitespace and commas.
pub fn tokens(script: &str) -> impl Iterator<Item = &str> {
    script.split(|c: char| c.is_whitespace() || c == ',').filter(|s| !s.is_empty())
}

/// The script text of a step (what the simulator runs).
pub fn step_text(s: &Step) -> String {
    match s {
        Step::Tap(k, 1) => k.name().to_string(),
        Step::Tap(k, n) => format!("{}*{n}", k.name()),
        Step::Hold(k, ms) => format!("hold:{}:{ms}", k.name()),
        Step::Press(k) => format!("press:{}", k.name()),
        Step::Release(k) => format!("release:{}", k.name()),
        Step::Wait(ms) => format!("wait:{ms}"),
        Step::Shot(n) => format!("shot:{n}"),
        Step::Expect(n) => format!("expect-screen:{n}"),
    }
}

/// One item of a journey.
#[derive(Clone, Debug)]
pub enum Item {
    /// Runs in the simulator and on the player (the original token).
    Step(String, Step),
    /// Simulator only (`sim:` lines): brings the fake services to the state
    /// the player is in (something playing, a locked vault …).
    Sim(String),
    /// Player only (`dev:` lines): e.g. a longer wait for the card.
    Dev(Step),
    /// Player only (`cmd:` lines): a firmware test command; its reply must
    /// match the regular expression, if one is given (`cmd: TEXT ~ REGEX`).
    Cmd { text: String, want: Option<String> },
}

/// Parse a journey: one command group per line; `#` starts a comment line.
pub fn parse_journey(text: &str) -> Result<Vec<Item>> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = |e: anyhow::Error| e.context(format!("line {}: `{line}`", i + 1));
        if let Some(rest) = line.strip_prefix("cmd:") {
            let (text, want) = match rest.split_once(" ~ ") {
                Some((t, w)) => (t.trim(), Some(w.trim().to_string())),
                None => (rest.trim(), None),
            };
            if text.is_empty() {
                return Err(at(anyhow!("empty command")));
            }
            if let Some(w) = &want {
                regex::Regex::new(w).map_err(|e| at(anyhow!("bad regex: {e}")))?;
            }
            out.push(Item::Cmd { text: text.to_string(), want });
        } else if let Some(rest) = line.strip_prefix("sim:") {
            for t in tokens(rest) {
                if t == "power" || t.contains(":power") || t.starts_with("power*") {
                    return Err(at(anyhow!("no Power key, not even in the simulator part")));
                }
                out.push(Item::Sim(t.to_string()));
            }
        } else if let Some(rest) = line.strip_prefix("dev:") {
            for t in tokens(rest) {
                let s = parse_step(t).map_err(at)?;
                if matches!(s, Step::Shot(_)) {
                    return Err(at(anyhow!("shots are compared with the simulator: not in `dev:` lines")));
                }
                out.push(Item::Dev(s));
            }
        } else {
            for t in tokens(line) {
                out.push(Item::Step(t.to_string(), parse_step(t).map_err(at)?));
            }
        }
    }
    Ok(out)
}

/// Rough time on the player (ms): taps, holds, waits, plus the developer
/// channel's round trips (a screenshot is 32 KiB in 1 KiB control reads).
pub fn estimate_ms(items: &[Item]) -> u64 {
    let step = |s: &Step| match s {
        Step::Tap(_, n) => u64::from(*n) * (TAP_DOWN_MS + TAP_UP_MS + 10),
        Step::Hold(_, ms) => ms + TAP_UP_MS + 10,
        Step::Press(_) | Step::Release(_) => 5,
        Step::Wait(ms) => *ms,
        Step::Shot(_) => 700,
        Step::Expect(_) => 60,
    };
    items
        .iter()
        .map(|i| match i {
            Item::Step(_, s) | Item::Dev(s) => step(s),
            Item::Sim(_) => 0,
            Item::Cmd { want, .. } => {
                if want.is_some() {
                    300
                } else {
                    60
                }
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_round_trip() {
        for t in [
            "ok",
            "down*3",
            "hold:menu:700",
            "press:next",
            "release:next",
            "wait:300",
            "shot:a",
            "expect-screen:Home",
        ] {
            assert_eq!(step_text(&parse_step(t).unwrap()), t);
        }
        assert_eq!(parse_step("hold:back").unwrap(), Step::Hold(Key::Back, HOLD_MS));
        assert_eq!(parse_step("vol+").unwrap(), Step::Tap(Key::VolUp, 1));
    }

    #[test]
    fn power_and_sim_only_refused() {
        for t in ["power", "hold:power", "press:power", "power*2"] {
            assert!(parse_step(t).unwrap_err().to_string().contains("rule 4"), "{t}");
        }
        assert!(parse_step("set:battery=5").unwrap_err().to_string().contains("sim:"));
        assert!(parse_step("goto:Home").is_err());
        assert!(parse_journey("sim: hold:power").is_err());
        assert!(parse_journey("dev: shot:x").is_err());
    }

    #[test]
    fn journey_lines() {
        let j = parse_journey(
            "# comment\nok down*2\nsim: set:queue=takk\ndev: wait:500\ncmd: status ~ underruns=0\ncmd: stop\n",
        )
        .unwrap();
        assert_eq!(j.len(), 6);
        assert!(matches!(&j[2], Item::Sim(s) if s == "set:queue=takk"));
        assert!(matches!(&j[3], Item::Dev(Step::Wait(500))));
        assert!(matches!(&j[4], Item::Cmd { text, want: Some(w) } if text == "status" && w == "underruns=0"));
        assert!(matches!(&j[5], Item::Cmd { want: None, .. }));
        assert!(parse_journey("cmd: status ~ (").is_err());
        assert_eq!(estimate_ms(&j), 3 * 110 + 500 + 300 + 60);
    }
}
