//! VER-13 (07 §5.12): the setting → effect matrix over every screen and
//! service context of the simulator.
//!
//! For each setting of `tests/settings/effects.toml` the same journey to
//! each context runs with the setting at a value under test and at its
//! default (`set:KEY=…` first, then the journey). Screen contexts (the
//! manifest's `@screens` group) compare the frame and the backlight's
//! duty (`Sim::backlight`); service contexts
//! (`@services`) compare what the simulated services did (the queue, the
//! tuner's register writes, the recorded file, the keyboard reports, …).
//! The observation must differ in exactly the contexts the manifest
//! declares for the setting (`contexts`, groups expanded) and be the same
//! in every other one (the "absent elsewhere" half). Every non-default
//! value is tried in the declared contexts (each must differ from the
//! default somewhere, unless the setting's effect is outside the
//! simulator); the leak check elsewhere uses up to three of them (the
//! first, the middle and the last). A declared context whose `cover`
//! names a dedicated test (an effect that needs a second setting, time or
//! a file) or that is `uncovered` need not show in the fixed journey.
//!
//! `context_scripts` and `service_scripts` must name exactly the
//! manifest's `@screens` and `@services` (a test checks it), so the
//! manifest and this file cannot drift apart.
//!
//! Settings with `gap` (no effect yet) are held to the same: they may show
//! their value where `contexts` says (their Settings page) and nowhere
//! else.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use orpheus_apps::settings::{self, Kind, SettingId};
use orpheus_sim::AppSim;

// ------------------------------------------------------------ contexts

const PLAY: &str = "set:queue=takk set:track=1 set:pos=47000";
const STOPWATCH: &str = "next*7 ok wait:100 expect-screen:Stopwatch";
const BOOKS: &str = "set:books=on goto:Books wait:50 expect-screen:Books";
const SEQ: &str = "set:seq=on set:seqdemo=1 goto:Sequencer wait:50 expect-screen:Sequencer";

/// Screen context → journey from a fresh simulator at Home.
fn context_scripts() -> Vec<(&'static str, String)> {
    let mut v: Vec<(&'static str, String)> = vec![
        ("Home", "wait:10".into()),
        ("Home.Menu", "menu".into()),
        ("Home.Playing", format!("{PLAY} wait:100")),
        ("QuickPanel", "hold:menu expect-screen:QuickPanel".into()),
        ("Volume", "set:volume=29 vol+ vol+".into()),
        ("PowerMenu", "hold:power expect-screen:PowerMenu".into()),
        ("Lock", "set:playing=1 power down expect-screen:Lock".into()),
        ("Charging", "set:charge=charging set:battery=64 goto:Charging expect-screen:Charging".into()),
        ("UsbConnect", "set:usb=host wait:100".into()),
        ("Music", "ok wait:100 expect-screen:Music".into()),
        ("Folders", "ok down*8 ok wait:100 expect-screen:Folders".into()),
        ("MusicLibrary", "set:library=ready ok down ok wait:100 expect-screen:MusicLibrary".into()),
        ("NowPlaying", format!("{PLAY} goto:NowPlaying wait:100")),
        ("NowPlaying.Menu", format!("{PLAY} goto:NowPlaying wait:100 menu")),
        ("Equaliser", "goto:Equaliser wait:50".into()),
        ("Files", "next ok wait:100 expect-screen:Files".into()),
        ("Files.Big", "next ok down*2 ok ok wait:600".into()),
        ("Files.Menu", "next ok wait:100 menu".into()),
        ("TextViewer", "next ok ok wait:100 ok wait:200 expect-screen:TextViewer".into()),
        ("Pictures", "next*5 ok wait:100 down*3 ok wait:100".into()),
        ("PictureViewer", "next*5 ok wait:100 down*3 ok wait:100 ok wait:100 expect-screen:PictureViewer".into()),
        ("Radio", "next*2 ok wait:1100 expect-screen:Radio".into()),
        (
            "RadioPresets",
            "set:presets=demo set:radio=94700 next*2 ok wait:1100 menu down ok wait:100 expect-screen:RadioPresets"
                .into(),
        ),
        ("Bluetooth", "next*3 ok wait:100 expect-screen:Bluetooth".into()),
        ("Recorder", "next*4 ok wait:200 expect-screen:Recorder".into()),
        ("Recorder.Recording", "set:recsignal=tone next*4 ok wait:200 ok wait:1500".into()),
        ("Steps", "set:steps=4321 next*6 ok expect-screen:Steps".into()),
        ("Steps.Week", "set:week=3000/9000/12000/500/7000/0/4321 next*6 ok next".into()),
        ("Stopwatch", STOPWATCH.into()),
        ("Timer", format!("{STOPWATCH} next")),
        ("Alarms", format!("{STOPWATCH} prev")),
        ("Alert", "set:0x0d32=1 set:clock=06:59:58 set:clockrun=1 wait:3000 expect-screen:Alert".into()),
        ("Authenticator", "set:vault=unlocked goto:Authenticator wait:100".into()),
        ("UsbDrive", "next*9 ok expect-screen:UsbDrive".into()),
        ("Settings", "prev ok expect-screen:Settings".into()),
        ("About", "prev ok down*11 ok ok expect-screen:Info".into()),
        ("HomeApps", "prev ok down*2 ok down*11 ok expect-screen:HomeApps".into()),
        ("Games", "set:gameseed=7 prev*3 ok wait:300 expect-screen:Games".into()),
        ("Blocks", "set:gameseed=7 goto:Blocks wait:100 ok wait:1500".into()),
        ("Game4096", "set:gameseed=7 goto:Game4096 wait:100 right down".into()),
        ("Solitaire", "set:gameseed=7 goto:Solitaire wait:100 up ok next ok".into()),
        ("Salvo", "set:gameseed=7 goto:Salvo wait:100 up right press:ok release:ok wait:1000".into()),
        ("Bricks", "set:gameseed=7 goto:Bricks wait:100 press:prev wait:200 release:prev ok wait:800".into()),
        ("GamePause", "set:gameseed=7 goto:Blocks wait:100 ok wait:1500 back expect-screen:GamePause".into()),
        ("Books", BOOKS.into()),
        ("Reader", format!("{BOOKS} down ok wait:50 expect-screen:Reader")),
        // Scrolled to its end: every reading setting's row shows.
        ("BookMenu", format!("{BOOKS} down ok wait:50 expect-screen:Reader ok wait:50 expect-screen:BookMenu down*8")),
        ("Sequencer", SEQ.into()),
        ("SeqGrid", format!("{SEQ} down ok wait:50 expect-screen:SeqGrid")),
    ];
    for (g, name) in [
        "Settings.Sound",
        "Settings.Playback",
        "Settings.Display",
        "Settings.Power",
        "Settings.DateTime",
        "Settings.Bluetooth",
        "Settings.Radio",
        "Settings.Recording",
        "Settings.Usb",
        "Settings.Language",
        "Settings.Storage",
        "Settings.System",
        "Settings.Accessibility",
    ]
    .into_iter()
    .enumerate()
    {
        v.push((name, format!("prev ok down*{g} ok expect-screen:SettingsGroup")));
    }
    v
}

/// Service context → (journey, what to read of the services after it).
type Probe = fn(&mut AppSim) -> String;

fn service_scripts() -> Vec<(&'static str, &'static str, Probe)> {
    vec![
        ("playback", "set:queue=takk wait:500", |s| {
            // The queue's order, a track's end in the middle of the queue
            // (the next track's place 500 ms on: Gapless), then the end of
            // the last track.
            let st = s.sys.media.state();
            let order: Vec<String> = (0..st.queue_len)
                .filter_map(|i| s.sys.media.queue_entry(i).map(|e| e.title.to_string()))
                .collect();
            run(s, &format!("set:pos={} wait:1000", st.duration_ms.saturating_sub(500)));
            let mid = s.sys.media.state();
            let last = st.queue_len.saturating_sub(1);
            run(s, &format!("set:track={last} wait:100"));
            let d = s.sys.media.state().duration_ms;
            run(s, &format!("set:pos={} wait:4000", d.saturating_sub(1000)));
            let end = s.sys.media.state();
            format!(
                "{order:?} {} {} {:?} {} {}",
                mid.queue_pos, mid.position_ms, end.source, end.queue_pos, end.position_ms
            )
        }),
        ("radio-tuner", "set:radio=94700 next*2 ok wait:1100 hold:next wait:1500", |s| {
            format!("{:?} {:?}", s.sys.radio.radio.info(), s.sys.radio.chip.writes)
        }),
        (
            "recording",
            "set:recsignal=tone next*4 ok wait:200 ok wait:2000 ok hold:ok next ok wait:200",
            |s| {
                let Some((path, _)) = s.sys.rec.last_file.clone() else { return "no file".into() };
                let b = orpheus_sim::games::read(&mut s.sys.media, &path).unwrap_or_default();
                format!("{path} {} {:016x}", b.len(), fnv(b.iter().map(|&x| u16::from(x))))
            },
        ),
        (
            "usb-keyboard",
            "set:vault=unlocked set:usb=host goto:Authenticator wait:100 ok wait:50 ok wait:100",
            |s| format!("{:?}", s.sys.auth.host),
        ),
        ("usb-host", "set:usb=host wait:200 ok wait:200", |s| {
            let usb: Vec<String> = s
                .sys
                .log
                .iter()
                .filter(|l| matches!(l, orpheus_sim::apps::Logged::Usb(_)))
                .map(|l| format!("{l:?}"))
                .collect();
            // VER-13: and what the computer is offered (the developer
            // interface, a write-protected card).
            format!("{usb:?} {:?}", s.sys.usb_offer())
        }),
        ("bt-module", "wait:3000", |s| format!("{:?} {}", s.sys.bt.phase, s.sys.bt.reconnect)),
        ("steps-counter", "set:walk=on wait:20000", |s| format!("{}", s.sys.steps.history.today())),
        ("alarms", "set:clock=06:59:58 set:clockrun=1 wait:3000", |s| s.screen().to_string()),
        ("sleep", "set:queue=takk wait:200", |s| format!("{:?}", s.sys.sleep_left)),
        ("volume", "set:volume=29 vol+ vol+", |s| format!("{}", s.sys.volume)),
        // Keys on Home and in a list: the clicks the sound output got.
        ("key-click", "next prev ok wait:100 down up back wait:100", |s| format!("{}", s.sys.clicks)),
        // A11Y-1: the screen reader on Home (the journey turns it on): two
        // moves and where am I; what it said and the audio's hash.
        ("speech", SPEECH, |s| {
            let a = s.a11y.as_mut().expect("the apps have a reader");
            a.speech.drain(20_000);
            let log: Vec<String> = a.speech.log.iter().map(orpheus_sim::speech::Said::line).collect();
            format!("{log:?} {:016x}", a.speech.audio_hash())
        }),
        // TTS-3: the screen reader speaking over music on Home (the
        // journey plays the album and turns the reader on): what was said,
        // the audio's hash, the music lowered under it (samples, lowest
        // gain) or paused for it (the player's pauses and resumes), and
        // the player at the end.
        ("speech-music", SPEECH_MUSIC, |s| {
            let a = s.a11y.as_mut().expect("the apps have a reader");
            a.speech.drain(20_000);
            let log: Vec<String> = a.speech.log.iter().map(orpheus_sim::speech::Said::line).collect();
            let (ducked, low) = (a.speech.ducked, a.speech.duck_min);
            let hash = a.speech.audio_hash();
            run(s, "wait:1000");
            let st = s.sys.media.state();
            format!(
                "{log:?} {hash:016x} ducked={ducked} low={low} pauses={} {:?}",
                s.sys.speech_pauses, st.source
            )
        }),
        // SEQ-2: the Sequencer — a new project (its kit, length, swing),
        // then the demo beat played 2.5 s from the grid (the notes only:
        // `seqdry`) and two keys pressed while it plays (with Key click
        // on): the steps played, the metronome's clicks, the count-in
        // left, the first note's frame, the key clicks.
        ("seq-play", SEQ_PLAY, |s| {
            let st = &s.sys.seq.studio;
            let clicks_metro =
                s.sys.seq.notes.iter().filter(|h| h.track == orpheus_seq::player::CLICK).count();
            let first =
                s.sys.seq.notes.iter().find(|h| h.track != orpheus_seq::player::CLICK).map(|h| h.frame);
            format!(
                "{} | {} {} {:?} {} | {}",
                s.sys.seq.noted.clone().unwrap_or_default(),
                st.seq.steps(),
                clicks_metro,
                first,
                st.seq.counting_in(),
                s.sys.clicks
            )
        }),
    ]
}

/// The `speech` journey (see `service_scripts`).
const SPEECH: &str = "set:0x1004=1 wait:1500 next wait:1500 down wait:1500 chord:back+ok:700 wait:3000";

/// The `speech-music` journey (see `service_scripts`): an album playing,
/// the reader on over it, two moves on Home.
const SPEECH_MUSIC: &str = "set:queue=takk wait:300 set:0x1004=1 wait:1500 next wait:1500 down wait:1500";

/// The `seq-play` journey (see `service_scripts`).
const SEQ_PLAY: &str = "set:0x0108=1 set:seq=on set:seqdry=on set:seqdemo=1 goto:Sequencer wait:50 ok wait:50 \
     expect-screen:SeqGrid set:seqnote=1 back wait:50 down ok wait:50 expect-screen:SeqGrid menu ok wait:2500 down up \
     wait:50";

fn run(s: &mut AppSim, script: &str) {
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
}

fn fnv(it: impl Iterator<Item = u16>) -> u64 {
    it.fold(0xcbf2_9ce4_8422_2325, |h, p| (h ^ u64::from(p)).wrapping_mul(0x0100_0000_01b3))
}

/// A context: its name, journey and what is compared (the frame, or a
/// service's state).
#[derive(Clone)]
struct Ctx {
    name: &'static str,
    script: String,
    probe: Option<Probe>,
}

/// Every context of @screens and @services.
fn all_contexts() -> Vec<Ctx> {
    let mut v: Vec<Ctx> =
        context_scripts().into_iter().map(|(name, script)| Ctx { name, script, probe: None }).collect();
    v.extend(service_scripts().into_iter().map(|(name, script, probe)| Ctx {
        name,
        script: script.to_string(),
        probe: Some(probe),
    }));
    v
}

// ------------------------------------------------------------ manifest

fn manifest() -> &'static toml::Table {
    static M: OnceLock<toml::Table> = OnceLock::new();
    M.get_or_init(|| {
        let text = include_str!("../../tests/settings/effects.toml");
        text.parse::<toml::Table>().unwrap_or_else(|e| panic!("effects.toml: {e}"))
    })
}

fn group(name: &str) -> Vec<String> {
    manifest()["groups"][name]
        .as_array()
        .unwrap_or_else(|| panic!("[groups] {name}"))
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

fn entry(id: u16) -> &'static toml::Table {
    let key = format!("0x{id:04X}");
    manifest()["setting"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(toml::Value::as_table)
        .find(|t| t.get("id").and_then(toml::Value::as_str) == Some(&key))
        .unwrap_or_else(|| panic!("{key}: no manifest entry"))
}

/// Names of a list of contexts and groups, groups expanded.
fn expand(list: Option<&toml::Value>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for c in list.and_then(toml::Value::as_array).into_iter().flatten() {
        let c = c.as_str().unwrap();
        match c.strip_prefix('@') {
            Some(g) => out.extend(group(g)),
            None => {
                out.insert(c.to_string());
            }
        }
    }
    out
}

/// The screen and service contexts the entry declares (groups expanded).
fn declared(id: u16) -> BTreeSet<String> {
    let mine: BTreeSet<String> = group("screens").into_iter().chain(group("services")).collect();
    let mut out = expand(entry(id).get("contexts"));
    out.retain(|c| mine.contains(c));
    out
}

/// Declared contexts the matrix's own journeys need not show: the entry's
/// `cover` names a dedicated test for them (an effect that needs more than
/// opening the screen: a sort, a held key, a time passing).
fn shown_elsewhere(id: u16) -> BTreeSet<String> {
    let me = format!("sim/tests/settings_matrix.rs::matrix::s{id:04x}");
    let mut out = BTreeSet::new();
    if let Some(cover) = entry(id).get("cover").and_then(toml::Value::as_table) {
        for (ctx, by) in cover {
            let by: Vec<&str> = by.as_array().into_iter().flatten().filter_map(toml::Value::as_str).collect();
            if !ctx.starts_with('@') && by.iter().any(|t| *t != me) {
                out.insert(ctx.clone());
            }
        }
    }
    // Declared contexts without a test yet (reviewed exclusions).
    if let Some(u) = entry(id).get("uncovered").and_then(toml::Value::as_table) {
        out.extend(u.keys().cloned());
    }
    out
}

// ------------------------------------------------------------ the matrix

/// What the context shows after `prefix` and its journey (the clock at
/// 21:05 so that the 12/24-hour clock shows). A journey that no longer
/// ends where it should (another app on Home, another file in a sorted
/// folder, no alarm ringing) is an effect too: it observes as such.
fn observe(prefix: &str, c: &Ctx) -> String {
    let mut s = AppSim::new();
    if let Err(e) = s.run_script(&format!("set:clock=21:05:00 {prefix} {}", c.script)) {
        return format!("journey ended elsewhere: {e:#}");
    }
    match c.probe {
        Some(p) => p(&mut s),
        None => {
            // What the panel shows (turned when the display is rotated),
            // and how bright: the backlight's duty (Brightness, the alert
            // flash, the panel dark).
            s.draw();
            format!("{:016x} bl={}", fnv(s.panel().into_iter()), s.backlight())
        }
    }
}

/// The values under test of a setting other than its default.
fn values(id: u16) -> Vec<i16> {
    let d = settings::def(SettingId(id)).unwrap_or_else(|| panic!("{id:#06x} not in the tree"));
    let all: Vec<i16> = match d.kind {
        Kind::Toggle => vec![0, 1],
        Kind::Choice(c) => (0..i16::try_from(c.len()).unwrap()).collect(),
        Kind::Range { min, max, step, .. } => {
            let mid = min + ((max - min) / 2 / step.max(1)) * step.max(1);
            vec![min, mid, max]
        }
        Kind::Language => vec![0, settings::lang_code("ru"), settings::lang_code("ja")],
        // Games off Home (the last cell before Settings: the other apps
        // keep their places, so the other journeys still find them).
        Kind::HomeApps => vec![1 << orpheus_apps::App::Games as u8],
        Kind::Action(_) | Kind::Info(_) => vec![],
    };
    all.into_iter().filter(|v| *v != d.default).collect()
}

/// Which contexts differ from the default for each value.
fn differences(id: u16, values: &[i16], contexts: &[Ctx]) -> BTreeMap<i16, BTreeSet<String>> {
    let d = settings::def(SettingId(id)).unwrap().default;
    let set = |v: i16| format!("set:0x{id:04x}={v}");
    let mut out: BTreeMap<i16, BTreeSet<String>> = values.iter().map(|v| (*v, BTreeSet::new())).collect();
    for c in contexts {
        let base = observe(&set(d), c);
        for &v in values {
            if observe(&set(v), c) != base {
                out.entry(v).or_default().insert(c.name.to_string());
            }
        }
    }
    out
}

/// One row of the matrix: present where declared, absent elsewhere.
fn row(id: u16) {
    let all = all_contexts();
    let vals = values(id);
    if std::env::var_os("ORPHEUS_MATRIX_DUMP").is_some() {
        let d = differences(id, &vals, &all);
        let union: BTreeSet<&String> = d.values().flatten().collect();
        let mut line = format!("MATRIX 0x{id:04X} {union:?}");
        for (v, c) in &d {
            line.push_str(&format!("\nMATRIX 0x{id:04X} = {v}: {c:?}"));
        }
        eprintln!("{line}");
        return;
    }
    let e = entry(id);
    // A setting without an effect (`gap`) may still show its value (its
    // Settings page): it must change nothing else.
    let gap = e.contains_key("gap");
    let want = declared(id);
    let elsewhere = shown_elsewhere(id);
    let mut fails = Vec::new();
    // Every value in the declared contexts.
    let own: Vec<Ctx> = all.iter().filter(|c| want.contains(c.name)).cloned().collect();
    let d = differences(id, &vals, &own);
    let mut shown = BTreeSet::new();
    let off_matrix = expand(e.get("contexts")).iter().any(|c| !want.contains(c)) || !elsewhere.is_empty();
    for (v, c) in &d {
        // A value that changes none of the matrix's contexts is tested
        // only where its effect is (audio, a dedicated journey …).
        if c.is_empty() && !gap && !off_matrix {
            fails.push(format!("value {v} changes no declared context"));
        }
        shown.extend(c.iter().cloned());
    }
    for c in want.difference(&shown) {
        if !elsewhere.contains(c) {
            fails.push(format!("declared context `{c}` shows no effect for any value"));
        }
    }
    // Up to three values elsewhere.
    let pick: Vec<i16> = match vals.len() {
        0..=3 => vals.clone(),
        n => vec![vals[0], vals[n / 2], vals[n - 1]],
    };
    let rest: Vec<Ctx> = all.iter().filter(|c| !want.contains(c.name)).cloned().collect();
    for (v, c) in differences(id, &pick, &rest) {
        for x in c {
            fails.push(format!("value {v} leaks into `{x}`"));
        }
    }
    assert!(fails.is_empty(), "0x{id:04X} ({}):\n  {}", e["name"], fails.join("\n  "));
}

#[test]
fn contexts_are_the_manifest_screens_and_services() {
    let names: BTreeSet<String> = context_scripts().iter().map(|(n, _)| (*n).to_string()).collect();
    assert_eq!(
        names,
        group("screens").into_iter().collect(),
        "context_scripts() and [groups] screens differ"
    );
    let names: BTreeSet<String> = service_scripts().iter().map(|(n, ..)| (*n).to_string()).collect();
    assert_eq!(
        names,
        group("services").into_iter().collect(),
        "service_scripts() and [groups] services differ"
    );
    // Each journey runs, ends where it says, and observes the same twice
    // (the simulator is deterministic).
    for c in all_contexts() {
        let a = observe("", &c);
        assert!(!a.starts_with("journey ended elsewhere"), "{}: {a}", c.name);
        assert_eq!(a, observe("", &c), "{}", c.name);
    }
}

/// Effects the matrix's fixed journeys cannot show: they need a second
/// setting (pairwise), time to pass, or a file to look into.
mod effects {
    use super::{AppSim, PLAY, fnv, run};
    use orpheus_apps::services::ToolsService;
    use orpheus_apps::tools::Alert;

    fn sim(script: &str) -> AppSim {
        let mut s = AppSim::new();
        run(&mut s, script);
        s
    }

    fn frame(script: &str) -> u64 {
        let mut s = sim(script);
        s.draw();
        fnv(s.fb.iter().copied())
    }

    /// 2026-11-25 (VER-13): the simulator queued a folder in order with
    /// Settings › Playback › Shuffle on (the firmware ORs the setting into
    /// `PlayDir`/`PlayLib`): the shuffle setting showed no effect on the
    /// queue in the simulator.
    #[test]
    fn regress_sim_queue_follows_the_shuffle_setting() {
        let order = |shuffle: i16| {
            let s = sim(&format!("set:0x0201={shuffle} set:queue=takk wait:200"));
            let n = s.sys.media.state().queue_len;
            (0..n).map(|i| s.sys.media.queue_entry(i).unwrap().title.to_string()).collect::<Vec<_>>()
        };
        let (plain, shuffled) = (order(0), order(1));
        assert_ne!(plain, shuffled, "the queue is shuffled");
        let (mut a, mut b) = (plain.clone(), shuffled);
        a.sort();
        b.sort();
        assert_eq!(a, b, "the same tracks");
    }

    /// The focused row's label and the list's length (screen reader).
    fn rows(s: &mut AppSim) -> Vec<String> {
        let focus = |s: &mut AppSim| {
            let mut d = orpheus_ui::a11y::Desc::new();
            s.shell.describe(&mut s.sys, &mut d);
            d.focus.map(|f| (f.label.as_str().to_string(), f.count)).unwrap_or_default()
        };
        let (first, count) = focus(s);
        let mut out = vec![first];
        for _ in 1..count {
            run(s, "down");
            out.push(focus(s).0);
        }
        out
    }

    /// Regression (DOC-2 finding, 2026-11-27): Settings › Bluetooth ›
    /// Output (0x0604, Headphones / Bluetooth) was offered in Settings and
    /// the quick panel but nothing read it: the YM03 cannot route (the
    /// Bluetooth module takes its audio after the headphone amplifier,
    /// U-BT-2). It is hidden: neither list offers it, and a stored value
    /// changes no frame.
    #[test]
    fn regress_output_setting_offered_without_effect() {
        assert!(orpheus_apps::settings::hidden(orpheus_apps::settings::id::OUTPUT));
        let mut q = sim("hold:menu expect-screen:QuickPanel");
        let quick = rows(&mut q);
        assert!(quick.iter().any(|r| r == "Bluetooth"), "{quick:?}");
        assert!(!quick.iter().any(|r| r == "Output"), "{quick:?}");
        let mut g = sim("goto:Settings wait:50");
        let groups = rows(&mut g);
        let bt = groups.iter().position(|r| r == "Bluetooth").expect("a Bluetooth group");
        let mut b = sim(&format!("goto:Settings wait:50 down*{bt} ok wait:50"));
        let items = rows(&mut b);
        assert!(!items.is_empty() && !items.iter().any(|r| r == "Output"), "{items:?}");
        for script in
            ["hold:menu expect-screen:QuickPanel", &format!("goto:Settings wait:50 down*{bt} ok wait:50")]
        {
            assert_eq!(
                frame(&format!("set:0x0604=0 {script}")),
                frame(&format!("set:0x0604=1 {script}")),
                "{script}"
            );
        }
    }

    /// Settings › Playback › Seek step (0x0205): Next held on Now Playing
    /// seeks by the setting's step first (2, 5, 10 or 30 s), then faster;
    /// the place shows on Now Playing and in the player.
    #[test]
    fn seek_step_sets_the_first_steps_of_a_held_key() {
        let held = |v: i16| {
            let mut s = sim(&format!("set:0x0205={v} {PLAY} goto:NowPlaying wait:100 hold:next:550 wait:50"));
            let at = s.sys.media.state().position_ms;
            s.draw();
            (at, fnv(s.fb.iter().copied()))
        };
        let (base, base_frame) = held(1);
        // The steps the hold made (the same for every value), from 2 s.
        let n = (i64::from(base) - i64::from(held(0).0)) / 3000;
        assert!(n >= 1, "{n} steps");
        for (v, step) in [(0i16, 2000u32), (2, 10_000), (3, 30_000)] {
            let (at, f) = held(v);
            assert_ne!(f, base_frame, "value {v}: Now Playing shows another place");
            // The same steps and time played: they differ by the steps.
            assert_eq!(i64::from(at) - i64::from(base), n * (i64::from(step) - 5000), "value {v}");
        }
        assert!(base > 52_000, "5 s (default) from 47 s: {base} ms");
    }

    /// Settings › Playback › Resume at start-up (0x0204): a start-up with a
    /// resume point opens it paused at its place (05 §6.1: Home shows the
    /// paused track, Music offers Now Playing); off, Music offers
    /// "Resume ‹title›" and nothing is loaded.
    #[test]
    fn resume_at_start_opens_the_saved_place_paused() {
        let on = sim("set:resume=1 wait:100");
        let st = on.sys.media.state();
        assert_eq!((st.source, st.position_ms), (orpheus_ui::event::Source::Paused, 83_000));
        assert!(on.sys.media.resume_title().is_none());
        let off = sim("set:0x0204=0 set:resume=1 wait:100");
        assert_eq!(off.sys.media.state().queue_len, 0, "off: nothing loaded");
        assert!(off.sys.media.resume_title().is_some(), "off: Music offers Resume");
        let music = |v: i16| frame(&format!("set:0x0204={v} set:resume=1 ok wait:100 expect-screen:Music"));
        assert_ne!(music(1), music(0), "Music: Now Playing or Resume");
        let home = |v: i16| frame(&format!("set:0x0204={v} set:resume=1 wait:100"));
        assert_ne!(home(1), home(0), "Home: the paused track in the strip");
        // Play continues at the saved place either way.
        let play = |v: i16| sim(&format!("set:0x0204={v} set:resume=1 ok ok wait:100")).sys.media.state();
        assert!(play(1).position_ms >= 83_000 && play(0).position_ms >= 83_000);
    }

    /// Settings › Playback › Gapless (0x0203): off, a track that ends is
    /// followed by the next after a short silence (the firmware stops the
    /// DAC and opens the next file fresh); on, at once.
    #[test]
    fn gapless_off_leaves_a_silence_between_tracks() {
        let after = |v: i16| {
            let s = sim(&format!("set:0x0203={v} set:queue=takk wait:200"));
            let d = s.sys.media.state().duration_ms;
            let mut s = s;
            run(&mut s, &format!("set:pos={} wait:1000", d - 500));
            let st = s.sys.media.state();
            (st.queue_pos, st.position_ms, s.sys.media.last_change)
        };
        let (on, off) = (after(1), after(0));
        assert_eq!((on.0, on.2, off.0, off.2), (1, Some(true), 1, Some(false)));
        assert_eq!(on.1 - off.1, orpheus_sim::media::GAP_MS, "{on:?} {off:?}");
    }

    /// Settings › Playback › Auto-bookmark long files (0x0207): a podcast
    /// (over 20 min) left part-way continues there when played again from
    /// its start; off, it starts at 0. A song (under 20 min) never does.
    #[test]
    fn auto_bookmark_continues_a_long_file() {
        let again = |v: i16, queue: &str| {
            let s = sim(&format!(
                "set:0x0207={v} set:queue={queue} set:pos=600000 wait:1000 set:queue=takk wait:500                  set:queue={queue} wait:100"
            ));
            s.sys.media.state().position_ms
        };
        assert!((601_000..601_300).contains(&again(1, "podcast")), "{}", again(1, "podcast"));
        assert!(again(0, "podcast") < 200, "off: from the start");
        assert!(again(1, "kino") < 200, "a song: from the start");
        let marks = sim("set:queue=podcast set:pos=600000 wait:31000 ok").sys.media.auto_marks().len();
        assert_eq!(marks, 1, "noted while playing");
    }

    /// Settings › Sound › Key click (0x0108): every key press clicks, not
    /// while the microphone is live (the recorder open, its meters on).
    #[test]
    fn key_click_on_every_press_not_with_the_microphone() {
        let clicks = |script: &str| sim(script).sys.clicks;
        assert_eq!(clicks("set:0x0108=0 next prev ok back"), 0);
        assert_eq!(clicks("set:0x0108=1 next prev ok back"), 4);
        assert_eq!(clicks("set:0x0108=1 hold:menu"), 1, "a held key clicks once");
        let rec = clicks("set:0x0108=1 next*4 ok wait:200 expect-screen:Recorder down up");
        assert_eq!(rec, 5, "no click once the recorder listens");
    }

    /// Key click = Presses and holds (0x0108 = 2, A11Y-2 "key-press
    /// confirmation sound"): a held key clicks again when it reaches its
    /// hold delay (with any Hold delay 0x1002), a tap clicks once; never
    /// while the microphone is live.
    #[test]
    fn key_click_confirms_holds() {
        let s = |script: &str| {
            let s = sim(script);
            (s.sys.clicks, s.sys.hold_clicks)
        };
        assert_eq!(s("set:0x0108=2 next prev"), (2, 0), "taps click once");
        assert_eq!(s("set:0x0108=2 hold:back"), (2, 1), "a hold clicks twice");
        assert_eq!(s("set:0x0108=1 hold:back"), (1, 0), "Presses: once");
        assert_eq!(s("set:0x0108=2 set:0x1002=4 hold:back:1000"), (1, 0), "not held long enough");
        assert_eq!(s("set:0x0108=2 set:0x1002=4 hold:back:2100"), (2, 1), "held past 2 s");
        let rec = s("set:0x0108=2 next*4 ok wait:200 expect-screen:Recorder hold:up");
        assert_eq!(rec, (5, 0), "no click once the recorder listens");
    }

    /// Settings › Accessibility › Hold delay (0x1002): a key held shorter
    /// than the delay is a tap (Menu on Home opens Home's menu, not the
    /// quick panel), held past it a hold; Power's menu needs at least the
    /// delay (1.5 s, or 2 s at the longest).
    #[test]
    fn hold_delay_moves_every_hold() {
        let top = |script: &str| sim(script).screen().to_string();
        for (v, ms) in [(0i16, 500u64), (1, 750), (2, 1000), (3, 1500), (4, 2000)] {
            assert_eq!(top(&format!("set:0x1002={v} hold:menu:{}", ms - 60)), "Home", "{v}: a tap");
            assert_eq!(top(&format!("set:0x1002={v} hold:menu:{}", ms + 60)), "QuickPanel", "{v}: a hold");
        }
        assert_eq!(top("set:0x1002=4 hold:power:1600"), "Home", "2 s: Power's menu waits");
        assert_eq!(top("set:0x1002=4 hold:power:2100"), "PowerMenu");
        assert_eq!(top("set:0x1002=2 hold:power:1600"), "PowerMenu", "1 s: Power at 1.5 s as before");
    }

    /// Settings › Accessibility › Key repeat (0x1003): Down held for 3 s in
    /// Settings › Display (13 rows) moves the focus row by row at 8 → 30
    /// per second (it reaches the end), 4 → 10 (slow) or 2 per second
    /// (slower).
    #[test]
    fn key_repeat_slows_a_held_key() {
        let moved = |v: i16| {
            let mut s =
                sim(&format!("set:0x030C=0 set:0x1003={v} prev ok down*2 ok expect-screen:SettingsGroup"));
            run(&mut s, "press:down wait:2000 release:down");
            s.draw();
            fnv(s.fb.iter().copied())
        };
        let (normal, slow, slower) = (moved(0), moved(1), moved(2));
        assert!(normal != slow && slow != slower && normal != slower);
        // Repeats counted on the mapper itself: orpheus-ui input.rs
        // `accessible_hold_and_repeat`.
    }

    /// Animations off is reduced motion too (A11Y-2, 05 §7.14): the alarm
    /// alert's icon and the paused recorder's dot stop blinking; with
    /// Animations on they change between two frames half a second apart.
    #[test]
    fn reduced_motion_stops_blinks_and_spinners() {
        // The pixels of `(x, y, w, h)` (the blinking or spinning mark only:
        // the recorder's meters move with the sound).
        let area = |s: &AppSim, (x, y, w, h): (usize, usize, usize, usize)| {
            fnv((y..y + h).flat_map(|r| (x..x + w).map(move |c| (r, c))).map(|(r, c)| s.fb[r * 128 + c]))
        };
        let pair = |v: i16, script: &str, gap: u64, r| {
            let mut s = sim(&format!("set:0x0308={v} {script}"));
            s.draw();
            let a = area(&s, r);
            run(&mut s, &format!("wait:{gap}"));
            s.draw();
            (a, area(&s, r))
        };
        let alert = "set:0x0d32=1 set:clock=06:59:58 set:clockrun=1 wait:3000 expect-screen:Alert";
        let paused = "set:recsignal=tone next*4 ok wait:200 ok wait:1000 ok wait:300 expect-screen:Recorder";
        // The spinner (radio, Bluetooth, update): orpheus-ui misc.rs
        // `spinner_stands_still_with_reduced_motion`.
        for (what, script, gap, r) in
            [("alert", alert, 500, (52, 22, 24, 24)), ("recorder", paused, 500, (4, 18, 8, 8))]
        {
            let (a, b) = pair(1, script, gap, r);
            assert_ne!(a, b, "{what}: moves with Animations on");
            let (a, b) = pair(0, script, gap, r);
            assert_eq!(a, b, "{what}: still with Animations off");
        }
    }

    /// Now Playing's names line (UI-NP, owner 2026-11-28): "Artist –
    /// Title" over the whole width above the progress bar scrolls when it
    /// is wider than the screen (frames 1.5 s apart differ) with Animations
    /// (0x0308) on and Power mode (0x0401) Auto; Animations off or Battery
    /// saver keep it at rest, cut with an ellipsis, as it is before the
    /// marquee starts.
    #[test]
    fn now_playing_names_scroll_unless_reduced_motion() {
        // Panel rows 71–89: the names line (content y 57–75); the rest of
        // the screen is not part of this.
        let line = |s: &AppSim| fnv((71..90).flat_map(|r| (0..128).map(move |c| s.fb[r * 128 + c])));
        let pair = |set: &str| {
            let mut s = sim(&format!("{set} {PLAY} goto:NowPlaying wait:100 expect-screen:NowPlaying"));
            s.draw();
            let a = line(&s);
            run(&mut s, "wait:1500");
            s.draw();
            (a, line(&s))
        };
        let (rest, moved) = pair("set:0x0308=1 set:0x0401=0");
        assert_ne!(rest, moved, "Animations on: the long names line scrolls");
        for set in ["set:0x0308=0 set:0x0401=0", "set:0x0308=1 set:0x0401=1"] {
            let (a, b) = pair(set);
            assert_eq!((a, b), (rest, rest), "{set}: the line stays at rest");
        }
    }

    /// Settings › Accessibility › Text size (0x1001): lists show fewer,
    /// taller rows and a key pages by what shows (Settings › Display,
    /// Next pages by 5 rows normally, 4 at Large, 3 at Extra large), and
    /// the focus stays on the panel.
    #[test]
    fn text_size_pages_by_the_rows_shown() {
        let focus = |v: i16| {
            let s = sim(&format!(
                "set:0x030C=0 set:0x1001={v} prev ok down*2 ok expect-screen:SettingsGroup next"
            ));
            orpheus_ui::nav::Screen::<orpheus_sim::apps::HostServices>::save(s.shell.top()).focus
        };
        assert_eq!((focus(0), focus(1), focus(2)), (6, 5, 4));
    }

    /// Settings › Sound › Fade on pause (0x0109): the player's `status`
    /// reports the last pause's fade (60 ms, or 0 when it stopped at
    /// once); the fade itself is the chain's (orpheus-dsp
    /// `pause_fades_out_and_in_over_60_ms`) and the browser build's.
    #[test]
    fn fade_on_pause_is_reported() {
        let fade = |v: i16| sim(&format!("set:0x0109={v} {PLAY} set:pause=1 wait:100")).sys.media.last_fade;
        assert_eq!((fade(1), fade(0)), (Some(60), Some(0)));
    }

    /// Equaliser › Custom 1/2 bands (0x0D10–0x0D19): the equaliser screen
    /// draws a band's gain when its curve is the preset, and nothing
    /// changes while another preset is chosen (pairwise with 0x0101).
    #[test]
    fn custom_bands_show_with_their_preset() {
        for band in 0x0D10..=0x0D19u16 {
            let preset = if band < 0x0D15 { 9 } else { 10 };
            let eq =
                |p: i16, g: i16| frame(&format!("set:0x0101={p} set:{band:#06x}={g} goto:Equaliser wait:50"));
            assert_ne!(eq(preset, 6), eq(preset, 0), "{band:#06x} with its preset");
            assert_ne!(eq(preset, -12), eq(preset, 0), "{band:#06x} at -12 dB");
            assert_eq!(eq(0, 6), eq(0, 0), "{band:#06x} with Flat");
            let other = if preset == 9 { 10 } else { 9 };
            assert_eq!(eq(other, 6), eq(other, 0), "{band:#06x} with the other custom curve");
        }
    }

    /// Recording › Input gain (0x080A) with Gain Manual (0x0804 = 0): Up
    /// on the recorder steps from it (the toast shows +12 clamped, or +1),
    /// and it is passed on (`rec_settings().gain_db`, which the firmware
    /// puts into the ADC's digital gain; the simulator's fake ADC has none,
    /// so the sound itself is player-only). With Automatic gain Up only
    /// says so, whatever the value.
    #[test]
    fn input_gain_in_manual_mode() {
        let ui = |mode: i16, g: i16| {
            frame(&format!(
                "set:0x0804={mode} set:0x080a={g} next*4 ok wait:200 expect-screen:Recorder up wait:50"
            ))
        };
        assert_ne!(ui(0, 12), ui(0, 0), "manual: shown");
        assert_ne!(ui(0, -12), ui(0, 0), "manual: shown");
        assert_eq!(ui(1, 12), ui(1, 0), "automatic: not shown");
        for g in [-12i16, 0, 7, 12] {
            let s = sim(&format!("set:0x080a={g}"));
            assert_eq!(orpheus_apps::services::rec_settings(&s.sys).gain_db, i8::try_from(g).unwrap());
        }
    }

    /// Recording › Auto-split (0x0807): the session is configured with
    /// the split time of each value (orpheus-rec's
    /// `split_ends_the_file_at_the_split_time` shows the split itself; 15
    /// minutes of recording do not fit the simulated card).
    #[test]
    fn auto_split_minutes() {
        for (v, min) in [(0i16, 0u32), (1, 15), (2, 30), (3, 60)] {
            let s = sim(&format!("set:0x0807={v}"));
            assert_eq!(orpheus_apps::services::rec_settings(&s.sys).split_min, min, "value {v}");
        }
    }

    /// The games' "score table variant" settings: after a game finished at
    /// the reference settings, the Games list and the game's pause menu
    /// show that variant's best result; with the setting at another value
    /// they show the other variant's (none yet). (game, finish, settings)
    /// for the games whose finish the simulator can script: Blocks (a
    /// stack topped out), Solitaire (an almost won game completing
    /// itself), Salvo (one round, player 2 hit). Salvo's players 3 and 4
    /// play only with 3–4 players, and 4096's and Bricks' finished games
    /// are not scripted yet (`tests/settings/effects.toml`, uncovered).
    fn finished_games() -> Vec<(&'static str, String, Vec<u16>)> {
        let blocks = "set:gameseed=7 set:blocksrows=16 goto:Blocks ok hold:down hold:down hold:down hold:down \
                      hold:down hold:down hold:down wait:300";
        let sol = "set:solnear=4 goto:Solitaire wait:1500";
        let salvo = "set:gameseed=7 set:0x0e66=0 goto:Salvo wait:100 set:salvohealth=1.0 menu down*6 ok \
                     set:salvoaim=0.90.150 press:ok release:ok wait:6000 ok wait:100";
        vec![
            ("Blocks", blocks.into(), vec![0x0E01, 0x0E03, 0x0E06, 0x0E0A]),
            ("Solitaire", sol.into(), vec![0x0E41, 0x0E42, 0x0E43, 0x0E47]),
            (
                "Salvo",
                salvo.into(),
                vec![0x0E61, 0x0E62, 0x0E63, 0x0E6C, 0x0E6D, 0x0E6E, 0x0E70, 0x0E71, 0x0E74, 0x0E75],
            ),
        ]
    }

    /// The value the finish script left (Salvo's rounds are not part of
    /// the variant) and another one of another variant (player 1 Expert:
    /// the hardest computer is part of Salvo's variant, Novice is not
    /// harder than the others' Easy).
    fn reference_and_other(s: &AppSim, key: u16) -> (i16, i16) {
        let d = orpheus_apps::settings::def(orpheus_apps::settings::SettingId(key)).unwrap();
        let r = s.sys.settings.get(d.id);
        (r, if key == 0x0E62 { 4 } else { d.step(r, 1) })
    }

    #[test]
    fn score_table_variant_in_games_and_pause() {
        let mut fails = Vec::new();
        for (game, finish, keys) in finished_games() {
            let base = sim(&finish);
            for key in keys {
                let (r, v) = reference_and_other(&base, key);
                let list = |x: i16| frame(&format!("{finish} set:{key:#06x}={x} goto:Games wait:300"));
                if list(r) == list(v) {
                    fails.push(format!("{game} {key:#06x}={v}: the Games list is the same"));
                }
                let pause = |x: i16| {
                    frame(&format!("{finish} set:{key:#06x}={x} goto:{game} wait:100 back wait:100"))
                };
                if pause(r) == pause(v) {
                    fails.push(format!("{game} {key:#06x}={v}: the pause menu is the same"));
                }
            }
        }
        // The control: settings outside the variant (Blocks' ghost piece
        // and previews, Solitaire's timer) leave both as they are.
        for (game, finish, _) in finished_games().into_iter().filter(|g| g.0 != "Salvo") {
            for key in [0x0E04u16, 0x0E05, 0x0E46] {
                let base = sim(&finish);
                let (r, v) = reference_and_other(&base, key);
                let list = |x: i16| frame(&format!("{finish} set:{key:#06x}={x} goto:Games wait:300"));
                if list(r) != list(v) {
                    fails.push(format!("{game} {key:#06x}={v}: changes the Games list"));
                }
            }
        }
        assert!(fails.is_empty(), "{}", fails.join("\n"));
    }

    /// Alarms 1–4: time (0x0D30 + 3k), days (+1) and on (+2) together:
    /// the alarm rings at its time on its days when on, and not at
    /// another time, on another day or when off.
    #[test]
    fn alarms_ring_at_their_time_on_their_days() {
        for k in 0..4u16 {
            let (t, d, on) = (0x0D30 + 3 * k, 0x0D31 + 3 * k, 0x0D32 + 3 * k);
            let ring = |time: i16, days: i16, enabled: i16| {
                let s = sim(&format!(
                    "set:{t:#06x}={time} set:{d:#06x}={days} set:{on:#06x}={enabled} set:clock=09:59:58 \
                     set:clockrun=1 wait:3000"
                ));
                s.sys.tools().ringing()
            };
            let kk = u8::try_from(k).unwrap();
            assert_eq!(ring(600, 0x7F, 1), Some(Alert::Alarm(kk)), "alarm {k} at 10:00");
            assert_eq!(ring(601, 0x7F, 1), None, "alarm {k} at 10:01");
            assert_eq!(ring(600, 0, 0), None, "alarm {k} once, off");
            assert_eq!(ring(600, 0x7F, 0), None, "alarm {k} off");
            // Today (the fake clock's Tuesday) not among the days.
            let today = sim("wait:1").sys.clock.weekday;
            assert_eq!(ring(600, 0x7F & !(1 << today), 1), None, "alarm {k}, not today");
        }
    }

    /// The key gate after `script` with the screen timers running as on the
    /// player (`set:timers=run`): (screen on, keys locked).
    fn gate(script: &str) -> (bool, bool) {
        let s = sim(&format!("set:timers=run {script}"));
        (s.shell.gate.screen_on, s.shell.gate.locked)
    }

    /// Display › Backlight timeout (0x0302): the screen goes off that long
    /// after the last key (5 s … 120 s), never with Always; a key restarts
    /// it. With the timers held (the test look) nothing happens.
    #[test]
    fn backlight_timeout_turns_the_screen_off() {
        // Lock delay Never: only the screen goes off.
        for (v, s) in [(0i16, 5u64), (1, 10), (2, 15), (3, 30), (4, 60), (5, 120)] {
            let ms = s * 1000;
            let set = format!("set:0x0302={v} set:0x0303=4");
            assert_eq!(gate(&format!("{set} wait:{}", ms - 500)), (true, false), "value {v} before");
            assert_eq!(gate(&format!("{set} wait:{}", ms + 100)), (false, false), "value {v} after");
            assert_eq!(
                gate(&format!("{set} wait:{} down wait:{}", ms - 500, ms - 500)),
                (true, false),
                "a key"
            );
        }
        assert_eq!(gate("set:0x0302=6 wait:200000"), (true, false), "Always");
        let held = sim("set:0x0302=0 set:0x0303=0 wait:20000");
        assert!(held.shell.gate.screen_on && !held.shell.gate.locked, "held under the test look");
        // The first key after it only wakes the screen.
        let mut s = sim("set:timers=run set:0x0302=0 set:0x0303=4 wait:5100");
        run(&mut s, "down");
        assert!(s.shell.gate.screen_on);
        assert_eq!(s.screen(), "Home", "the waking key does nothing else");
    }

    /// Display › Brightness (0x030D, U-PWM-3): the lit panel's backlight
    /// is the stock duty of the level (default 6 → 8; 1 → 3, 11 → 15),
    /// on every screen; dark after Power whatever the level; an alert
    /// flashes 15 and 3 every 500 ms whatever the level (never 16).
    #[test]
    fn brightness_lights_the_panel_at_its_level() {
        use orpheus_power::backlight::LEVEL_DUTY;
        assert_eq!(sim("wait:10").backlight(), 8, "the stock default");
        for lvl in 1..=11i16 {
            let want = LEVEL_DUTY[lvl as usize];
            assert_eq!(sim(&format!("set:0x030d={lvl} wait:10")).backlight(), want, "Home, level {lvl}");
            let s = sim(&format!("set:0x030d={lvl} prev ok wait:100 expect-screen:Settings"));
            assert_eq!(s.backlight(), want, "Settings, level {lvl}");
            assert_eq!(sim(&format!("set:0x030d={lvl} power")).backlight(), 0, "dark, level {lvl}");
        }
        let alert = "set:0x0d32=1 set:clock=06:59:58 set:clockrun=1 wait:3000 expect-screen:Alert";
        for lvl in [1i16, 6, 11] {
            let mut s = sim(&format!("set:0x030d={lvl} {alert}"));
            let mut seen = std::collections::BTreeSet::new();
            for _ in 0..8 {
                run(&mut s, "wait:250");
                seen.insert(s.backlight());
            }
            assert_eq!(seen.into_iter().collect::<Vec<_>>(), [3, 15], "alert at level {lvl}");
        }
        // The slider applies each step at once (live), as the stock one.
        let s = sim("prev ok down*2 ok ok expect-screen:Slider up");
        assert_eq!(s.backlight(), LEVEL_DUTY[7]);
    }

    /// Display › Lock delay (0x0303): after the backlight timeout (5 s
    /// here) the keys lock at once, or 5, 30, 60 s later; Never: not by the
    /// timer. A key before then only wakes the screen and keeps them free.
    #[test]
    fn lock_delay_locks_after_the_screen_went_off() {
        for (v, s) in [(0i16, 0u64), (1, 5), (2, 30), (3, 60)] {
            let at = 5000 + s * 1000;
            let set = format!("set:0x0302=0 set:0x0303={v}");
            if s > 0 {
                assert_eq!(gate(&format!("{set} wait:{}", at - 500)), (false, false), "value {v} before");
            }
            assert_eq!(gate(&format!("{set} wait:{}", at + 100)), (false, true), "value {v} after");
        }
        assert_eq!(gate("set:0x0302=0 set:0x0303=4 wait:120000"), (false, false), "Never");
        assert_eq!(
            gate("set:0x0302=0 set:0x0303=1 wait:6000 down wait:3000"),
            (true, false),
            "woken in time"
        );
    }

    /// Display › Screen saver (0x0307): Clock shows a dim clock on the
    /// background when the backlight timeout turns the screen off (the
    /// panel stays lit), which moves at the next minute; Off leaves the
    /// panel dark (the frame as it was). A key brings the screen back.
    #[test]
    fn screen_saver_shows_a_moving_clock() {
        let frame_of = |s: &mut AppSim| {
            s.draw();
            fnv(s.panel().into_iter())
        };
        let mut home = sim("wait:100");
        let home_frame = frame_of(&mut home);
        let script = |v: i16| {
            format!(
                "set:timers=run set:clock=10:00:30 set:clockrun=1 set:0x0302=0 set:0x0303=4 set:0x0307={v}"
            )
        };
        let mut off = sim(&format!("{} wait:6000", script(0)));
        assert!(!off.shell.gate.screen_on && !off.shell.gate.saver);
        let mut on = sim(&format!("{} wait:6000", script(1)));
        assert!(!on.shell.gate.screen_on && on.shell.gate.saver);
        let saver = frame_of(&mut on);
        assert_ne!(saver, frame_of(&mut off), "the saver is drawn");
        assert_ne!(saver, home_frame);
        run(&mut on, "wait:30000");
        assert_ne!(frame_of(&mut on), saver, "the clock moved at 10:01");
        run(&mut on, "down");
        run(&mut off, "wait:30000 down");
        assert!(on.shell.gate.screen_on);
        assert_eq!(frame_of(&mut on), frame_of(&mut off), "Home is back, as after a dark screen");
        // Power turns the screen off dark, never into the saver.
        let s = sim(&format!("{} power", script(1)));
        assert!(!s.shell.gate.screen_on && !s.shell.gate.saver);
    }

    /// Power › Keys when locked (0x0404): with music playing and the keys
    /// locked (Power), Vol+ works with Volume and Volume and transport,
    /// not with None (the lock notice instead); Next changes the track
    /// only with Volume and transport. The screen under the lock never
    /// sees the keys.
    #[test]
    fn keys_when_locked_reach_volume_and_transport() {
        for (v, vol, transport) in [(0i16, false, false), (1, true, false), (2, true, true)] {
            let s = sim(&format!("set:0x0404={v} set:queue=takk wait:300 power wait:100 vol+ wait:100"));
            assert!(s.shell.gate.locked);
            assert_eq!(s.sys.volume != 12, vol, "value {v}: volume");
            let mut s = sim(&format!("set:0x0404={v} set:queue=takk wait:300 power wait:100"));
            let (top, pos) = (s.screen(), s.sys.media.state().queue_pos);
            run(&mut s, "next wait:300");
            assert_eq!(s.sys.media.state().queue_pos != pos, transport, "value {v}: Next");
            if transport {
                assert_eq!(s.screen(), top, "value {v}: the screen under the lock stays");
            }
            run(&mut s, "ok wait:300");
            let playing = s.sys.media.state().source == orpheus_ui::event::Source::Playing;
            assert_eq!(playing, !transport, "value {v}: OK pauses only with transport");
        }
    }

    /// Date & time › Correction (0x0506): +60 s a day puts the running
    /// clock a second ahead after 24 minutes, -60 a second behind, 0 not.
    #[test]
    fn clock_correction_moves_the_clock() {
        use orpheus_sim::Host;
        let clock = |v: i16| {
            let mut s = sim(&format!("set:0x0506={v} set:clock=12:00:00 set:clockrun=1"));
            for _ in 0..24 {
                s.sys.advance(60_000);
            }
            let c = s.sys.clock;
            (c.hour, c.minute, c.second)
        };
        assert_eq!(clock(0), (12, 24, 0));
        assert_eq!(clock(60), (12, 24, 1));
        assert_eq!(clock(-60), (12, 23, 59));
        assert_eq!(clock(30), (12, 24, 0), "half a second so far");
    }

    /// Date & time › Set from RDS (0x0507): with the radio on a station
    /// that sends clock time, the clock is set from it (in the Time zone)
    /// once two minutes agree; off, it is left alone.
    #[test]
    fn set_from_rds_sets_the_clock() {
        // 2026-11-25 08:30:00 UTC on air; the player's clock at 12:41.
        let clock = |set: &str| {
            let s = sim(&format!(
                "{set} set:rdsclock=1795595400 set:radio=94700 next*2 ok wait:1100 expect-screen:Radio wait:62000"
            ));
            let c = s.sys.clock;
            (c.day, c.hour, c.minute)
        };
        assert_eq!(clock("set:0x0507=0").1, 12, "off: the clock keeps its time");
        assert_eq!(clock("set:0x0507=1"), (25, 8, 31));
        assert_eq!(clock("set:0x0507=1 set:0x0505=60"), (25, 9, 31));
    }

    /// USB › Read-only card (0x0902): File transfer gives the computer the
    /// card write-protected; the USB question says so.
    #[test]
    fn read_only_card_for_the_computer() {
        for v in [0i16, 1] {
            let s = sim(&format!("set:0x0902={v} set:usb=host wait:200 down ok wait:200"));
            let o = s.sys.usb_offer();
            assert_eq!(o.personality, Some(orpheus_apps::services::UsbMode::FileTransfer));
            assert_eq!(o.card_read_only, v == 1, "value {v}");
        }
        // Charge only: no card, nothing write-protected.
        let s = sim("set:0x0902=1 set:usb=host wait:200 ok wait:200");
        assert!(!s.sys.usb_offer().card_read_only);
    }
}

/// One test per setting (the tool resolves each by name).
mod matrix {
    use super::row;

    #[test]
    fn s0101() {
        row(0x0101);
    }
    #[test]
    fn s0102() {
        row(0x0102);
    }
    #[test]
    fn s0103() {
        row(0x0103);
    }
    #[test]
    fn s0104() {
        row(0x0104);
    }
    #[test]
    fn s0105() {
        row(0x0105);
    }
    #[test]
    fn s0106() {
        row(0x0106);
    }
    #[test]
    fn s0107() {
        row(0x0107);
    }
    #[test]
    fn s0108() {
        row(0x0108);
    }
    #[test]
    fn s0109() {
        row(0x0109);
    }
    #[test]
    fn s0201() {
        row(0x0201);
    }
    #[test]
    fn s0202() {
        row(0x0202);
    }
    #[test]
    fn s0203() {
        row(0x0203);
    }
    #[test]
    fn s0204() {
        row(0x0204);
    }
    #[test]
    fn s0205() {
        row(0x0205);
    }
    #[test]
    fn s0206() {
        row(0x0206);
    }
    #[test]
    fn s0207() {
        row(0x0207);
    }
    #[test]
    fn s0208() {
        row(0x0208);
    }
    #[test]
    fn s030d() {
        row(0x030D);
    }
    #[test]
    fn s0302() {
        row(0x0302);
    }
    #[test]
    fn s0303() {
        row(0x0303);
    }
    #[test]
    fn s0304() {
        row(0x0304);
    }
    #[test]
    fn s0305() {
        row(0x0305);
    }
    #[test]
    fn s0306() {
        row(0x0306);
    }
    #[test]
    fn s0307() {
        row(0x0307);
    }
    #[test]
    fn s0308() {
        row(0x0308);
    }
    #[test]
    fn s0309() {
        row(0x0309);
    }
    #[test]
    fn s030a() {
        row(0x030A);
    }
    #[test]
    fn s030e() {
        row(0x030E);
    }
    #[test]
    fn s030c() {
        row(0x030C);
    }
    #[test]
    fn s0401() {
        row(0x0401);
    }
    #[test]
    fn s0402() {
        row(0x0402);
    }
    #[test]
    fn s0403() {
        row(0x0403);
    }
    #[test]
    fn s0404() {
        row(0x0404);
    }
    #[test]
    fn s0503() {
        row(0x0503);
    }
    #[test]
    fn s0504() {
        row(0x0504);
    }
    #[test]
    fn s0505() {
        row(0x0505);
    }
    #[test]
    fn s0506() {
        row(0x0506);
    }
    #[test]
    fn s0507() {
        row(0x0507);
    }
    #[test]
    fn s0601() {
        row(0x0601);
    }
    #[test]
    fn s0603() {
        row(0x0603);
    }
    #[test]
    fn s0604() {
        row(0x0604);
    }
    #[test]
    fn s0701() {
        row(0x0701);
    }
    #[test]
    fn s0702() {
        row(0x0702);
    }
    #[test]
    fn s0703() {
        row(0x0703);
    }
    #[test]
    fn s0801() {
        row(0x0801);
    }
    #[test]
    fn s0802() {
        row(0x0802);
    }
    #[test]
    fn s0803() {
        row(0x0803);
    }
    #[test]
    fn s0804() {
        row(0x0804);
    }
    #[test]
    fn s0806() {
        row(0x0806);
    }
    #[test]
    fn s0807() {
        row(0x0807);
    }
    #[test]
    fn s0808() {
        row(0x0808);
    }
    #[test]
    fn s0809() {
        row(0x0809);
    }
    #[test]
    fn s080a() {
        row(0x080A);
    }
    #[test]
    fn s0901() {
        row(0x0901);
    }
    #[test]
    fn s0902() {
        row(0x0902);
    }
    #[test]
    fn s0903() {
        row(0x0903);
    }
    #[test]
    fn s0904() {
        row(0x0904);
    }
    #[test]
    fn s0905() {
        row(0x0905);
    }
    #[test]
    fn s0906() {
        row(0x0906);
    }
    #[test]
    fn s0a01() {
        row(0x0A01);
    }
    #[test]
    fn s0d01() {
        row(0x0D01);
    }
    #[test]
    fn s0d02() {
        row(0x0D02);
    }
    #[test]
    fn s0d03() {
        row(0x0D03);
    }
    #[test]
    fn s0d10() {
        row(0x0D10);
    }
    #[test]
    fn s0d11() {
        row(0x0D11);
    }
    #[test]
    fn s0d12() {
        row(0x0D12);
    }
    #[test]
    fn s0d13() {
        row(0x0D13);
    }
    #[test]
    fn s0d14() {
        row(0x0D14);
    }
    #[test]
    fn s0d15() {
        row(0x0D15);
    }
    #[test]
    fn s0d16() {
        row(0x0D16);
    }
    #[test]
    fn s0d17() {
        row(0x0D17);
    }
    #[test]
    fn s0d18() {
        row(0x0D18);
    }
    #[test]
    fn s0d19() {
        row(0x0D19);
    }
    #[test]
    fn s0d20() {
        row(0x0D20);
    }
    #[test]
    fn s0d21() {
        row(0x0D21);
    }
    #[test]
    fn s0d22() {
        row(0x0D22);
    }
    #[test]
    fn s0d23() {
        row(0x0D23);
    }
    #[test]
    fn s0d24() {
        row(0x0D24);
    }
    #[test]
    fn s0d25() {
        row(0x0D25);
    }
    #[test]
    fn s0d30() {
        row(0x0D30);
    }
    #[test]
    fn s0d31() {
        row(0x0D31);
    }
    #[test]
    fn s0d32() {
        row(0x0D32);
    }
    #[test]
    fn s0d33() {
        row(0x0D33);
    }
    #[test]
    fn s0d34() {
        row(0x0D34);
    }
    #[test]
    fn s0d35() {
        row(0x0D35);
    }
    #[test]
    fn s0d36() {
        row(0x0D36);
    }
    #[test]
    fn s0d37() {
        row(0x0D37);
    }
    #[test]
    fn s0d38() {
        row(0x0D38);
    }
    #[test]
    fn s0d39() {
        row(0x0D39);
    }
    #[test]
    fn s0d3a() {
        row(0x0D3A);
    }
    #[test]
    fn s0d3b() {
        row(0x0D3B);
    }
    #[test]
    fn s0d40() {
        row(0x0D40);
    }
    #[test]
    fn s0d41() {
        row(0x0D41);
    }
    #[test]
    fn s0d42() {
        row(0x0D42);
    }
    #[test]
    fn s0d43() {
        row(0x0D43);
    }
    #[test]
    fn s0d44() {
        row(0x0D44);
    }
    #[test]
    fn s0d50() {
        row(0x0D50);
    }
    #[test]
    fn s0e01() {
        row(0x0E01);
    }
    #[test]
    fn s0e02() {
        row(0x0E02);
    }
    #[test]
    fn s0e03() {
        row(0x0E03);
    }
    #[test]
    fn s0e04() {
        row(0x0E04);
    }
    #[test]
    fn s0e05() {
        row(0x0E05);
    }
    #[test]
    fn s0e06() {
        row(0x0E06);
    }
    #[test]
    fn s0e07() {
        row(0x0E07);
    }
    #[test]
    fn s0e08() {
        row(0x0E08);
    }
    #[test]
    fn s0e09() {
        row(0x0E09);
    }
    #[test]
    fn s0e0a() {
        row(0x0E0A);
    }
    #[test]
    fn s0e21() {
        row(0x0E21);
    }
    #[test]
    fn s0e22() {
        row(0x0E22);
    }
    #[test]
    fn s0e23() {
        row(0x0E23);
    }
    #[test]
    fn s0e24() {
        row(0x0E24);
    }
    #[test]
    fn s0e25() {
        row(0x0E25);
    }
    #[test]
    fn s0e41() {
        row(0x0E41);
    }
    #[test]
    fn s0e42() {
        row(0x0E42);
    }
    #[test]
    fn s0e43() {
        row(0x0E43);
    }
    #[test]
    fn s0e44() {
        row(0x0E44);
    }
    #[test]
    fn s0e45() {
        row(0x0E45);
    }
    #[test]
    fn s0e46() {
        row(0x0E46);
    }
    #[test]
    fn s0e47() {
        row(0x0E47);
    }
    #[test]
    fn s0e61() {
        row(0x0E61);
    }
    #[test]
    fn s0e62() {
        row(0x0E62);
    }
    #[test]
    fn s0e63() {
        row(0x0E63);
    }
    #[test]
    fn s0e64() {
        row(0x0E64);
    }
    #[test]
    fn s0e65() {
        row(0x0E65);
    }
    #[test]
    fn s0e66() {
        row(0x0E66);
    }
    #[test]
    fn s0e67() {
        row(0x0E67);
    }
    #[test]
    fn s0e68() {
        row(0x0E68);
    }
    #[test]
    fn s0e69() {
        row(0x0E69);
    }
    #[test]
    fn s0e6a() {
        row(0x0E6A);
    }
    #[test]
    fn s0e6b() {
        row(0x0E6B);
    }
    #[test]
    fn s0e6c() {
        row(0x0E6C);
    }
    #[test]
    fn s0e6d() {
        row(0x0E6D);
    }
    #[test]
    fn s0e6e() {
        row(0x0E6E);
    }
    #[test]
    fn s0e6f() {
        row(0x0E6F);
    }
    #[test]
    fn s0e70() {
        row(0x0E70);
    }
    #[test]
    fn s0e71() {
        row(0x0E71);
    }
    #[test]
    fn s0e72() {
        row(0x0E72);
    }
    #[test]
    fn s0e73() {
        row(0x0E73);
    }
    #[test]
    fn s0e74() {
        row(0x0E74);
    }
    #[test]
    fn s0e75() {
        row(0x0E75);
    }
    #[test]
    fn s0e76() {
        row(0x0E76);
    }
    #[test]
    fn s0ea1() {
        row(0x0EA1);
    }
    #[test]
    fn s0ea2() {
        row(0x0EA2);
    }
    #[test]
    fn s0ea3() {
        row(0x0EA3);
    }
    #[test]
    fn s0ea4() {
        row(0x0EA4);
    }
    #[test]
    fn s0ea5() {
        row(0x0EA5);
    }
    #[test]
    fn s0ea6() {
        row(0x0EA6);
    }
    #[test]
    fn s0ea7() {
        row(0x0EA7);
    }
    #[test]
    fn s0f01() {
        row(0x0F01);
    }
    #[test]
    fn s0f02() {
        row(0x0F02);
    }
    #[test]
    fn s0f03() {
        row(0x0F03);
    }
    #[test]
    fn s0f04() {
        row(0x0F04);
    }
    #[test]
    fn s0f05() {
        row(0x0F05);
    }
    #[test]
    fn s0f06() {
        row(0x0F06);
    }
    #[test]
    fn s0f07() {
        row(0x0F07);
    }
    #[test]
    fn s1001() {
        row(0x1001);
    }
    #[test]
    fn s1002() {
        row(0x1002);
    }
    #[test]
    fn s1003() {
        row(0x1003);
    }
    #[test]
    fn s1004() {
        row(0x1004);
    }
    #[test]
    fn s1005() {
        row(0x1005);
    }
    #[test]
    fn s1006() {
        row(0x1006);
    }
    #[test]
    fn s1007() {
        row(0x1007);
    }
    #[test]
    fn s1008() {
        row(0x1008);
    }
    #[test]
    fn s1009() {
        row(0x1009);
    }
    #[test]
    fn s100a() {
        row(0x100A);
    }
    #[test]
    fn s100b() {
        row(0x100B);
    }
    #[test]
    fn s100c() {
        row(0x100C);
    }
    #[test]
    fn s100d() {
        row(0x100D);
    }
    #[test]
    fn s1101() {
        row(0x1101);
    }
    #[test]
    fn s1102() {
        row(0x1102);
    }
    #[test]
    fn s1103() {
        row(0x1103);
    }
    #[test]
    fn s1104() {
        row(0x1104);
    }
    #[test]
    fn s1105() {
        row(0x1105);
    }
    #[test]
    fn s1106() {
        row(0x1106);
    }
    #[test]
    fn s1107() {
        row(0x1107);
    }
}
