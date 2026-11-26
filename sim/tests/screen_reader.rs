//! A11Y-1 (05 §7.14): the screen reader in the simulator.
//!
//! * `lint`: every screen of the apps, reached by real journeys, every
//!   focus on it (Down through it) and its menu: a title, a label for every
//!   focusable element, something to say at every verbosity
//!   (`orpheus_ui::a11y::Desc::problem`). The journeys must reach every
//!   screen the apps have.
//! * `spoken`: navigation journeys with the reader on; the exact spoken
//!   sequence (texts, earcons, stops, with their times) and the hash of the
//!   audio rendered through `orpheus-tts` are goldens in
//!   `sim/golden/speech/` (`ORPHEUS_BLESS=1` rewrites them;
//!   `ORPHEUS_SPEECH_WAVS=DIR` writes each journey's audio as a WAV).
//! * `behaviour`: interrupt on a key, chords (toggle, where am I, time and
//!   battery), the lock, keys with the screen off, earcons.
//! * `effects`: the differential tests of the reader's settings (VER-13,
//!   `tests/settings/effects.toml`): the same journey at a value and at the
//!   default; what is said or heard differs as declared, the frames do
//!   not.

use std::collections::BTreeSet;
use std::path::PathBuf;

use orpheus_apps::services::SettingsService;
use orpheus_sim::AppSim;
use orpheus_sim::speech::Said;
use orpheus_ui::a11y::{Cause, Config, Desc, Earcon, Reader, SpeechService, Verbosity};

fn sim(script: &str) -> AppSim {
    let mut s = AppSim::new();
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
    s
}

fn run(s: &mut AppSim, script: &str) {
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
}

fn log(s: &AppSim) -> &[Said] {
    &s.a11y.as_ref().expect("the apps have a reader").speech.log
}

fn texts(s: &AppSim) -> Vec<String> {
    s.a11y.as_ref().unwrap().speech.texts().into_iter().map(String::from).collect()
}

fn earcons(s: &AppSim) -> Vec<Earcon> {
    log(s)
        .iter()
        .filter_map(|l| match l {
            Said::Earcon(_, e) => Some(*e),
            _ => None,
        })
        .collect()
}

fn frame(s: &mut AppSim) -> u64 {
    s.draw();
    s.fb.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, p| (h ^ u64::from(*p)).wrapping_mul(0x0100_0000_01b3))
}

fn hash(s: &AppSim) -> u64 {
    s.a11y.as_ref().unwrap().speech.audio_hash()
}

/// A speech service that keeps the text (the lint's).
#[derive(Default)]
struct Capture(String);

impl SpeechService for Capture {
    fn stop(&mut self) {}
    fn say(&mut self, text: &str) {
        self.0.push_str(text);
        self.0.push('\n');
    }
    fn earcon(&mut self, _e: Earcon) {}
}

// ------------------------------------------------------------ the lint

const PLAY: &str = "set:queue=takk set:track=1 set:pos=47000";
const BOOKS: &str = "set:books=on goto:Books wait:50 expect-screen:Books";
const SEQ: &str = "set:seq=on set:seqdemo=1 goto:Sequencer wait:50 expect-screen:Sequencer";
const GRID: &str = "set:seq=on set:seqdemo=1 goto:Sequencer wait:50 down ok wait:50 expect-screen:SeqGrid";
const DRIVE: &str = "down*2 next ok wait:100 expect-screen:UsbDrive";
const GAMES: &str = "set:games=on set:gameseed=7 prev*2 ok wait:100 expect-screen:Games";

/// (what, journey from a fresh simulator): together every screen.
fn lint_journeys() -> Vec<(&'static str, String)> {
    let mut v: Vec<(&'static str, String)> = vec![
        ("home", "wait:10".into()),
        ("boot", "goto:Boot".into()),
        ("quick panel", "hold:menu expect-screen:QuickPanel".into()),
        ("volume", "set:volume=20 vol+".into()),
        ("power menu", "hold:power expect-screen:PowerMenu".into()),
        ("goodbye", "hold:power ok expect-screen:Goodbye".into()),
        ("lock", "set:playing=1 power down expect-screen:Lock".into()),
        ("charging", "set:charge=charging set:battery=64 goto:Charging expect-screen:Charging".into()),
        ("usb connect", "set:usb=host wait:100 expect-screen:UsbConnect".into()),
        ("notice", "set:card=absent ok expect-screen:Notice".into()),
        ("music", "ok wait:100 expect-screen:Music".into()),
        ("bookmarks", "ok wait:100 up ok wait:100 expect-screen:Bookmarks".into()),
        ("folders", "ok down*8 ok wait:100 expect-screen:Folders".into()),
        ("library", "set:library=ready ok down ok wait:100 expect-screen:MusicLibrary".into()),
        ("library albums", "set:library=ready ok down ok wait:100 ok wait:100 expect-screen:MusicLibrary".into()),
        ("now playing", format!("{PLAY} goto:NowPlaying wait:100")),
        ("now playing views", format!("{PLAY} goto:NowPlaying wait:100 down wait:50")),
        ("up next", format!("{PLAY} goto:NowPlaying wait:100 down*3 wait:100")),
        ("details", format!("{PLAY} goto:NowPlaying wait:100 down*4 wait:100")),
        ("nothing playing", "goto:NowPlaying wait:100".into()),
        ("equaliser", "goto:Equaliser wait:50".into()),
        ("files", "next ok wait:100 expect-screen:Files".into()),
        ("files big", "next ok down*2 ok ok wait:600".into()),
        ("properties", "next ok wait:100 menu down*2 ok wait:100 expect-screen:Properties".into()),
        ("text", "next ok ok wait:100 ok wait:200 expect-screen:TextViewer".into()),
        ("pictures", "next*5 ok wait:100 down*3 ok wait:100 expect-screen:Pictures".into()),
        ("picture", "next*5 ok wait:100 down*3 ok wait:100 ok wait:100 expect-screen:PictureViewer".into()),
        ("radio", "next*2 ok wait:1100 expect-screen:Radio".into()),
        (
            "presets",
            "set:presets=demo set:radio=94700 next*2 ok wait:1100 menu down ok wait:100 expect-screen:RadioPresets"
                .into(),
        ),
        ("bluetooth", "set:bt=on next*3 ok wait:100 expect-screen:Bluetooth".into()),
        ("bt scan", "set:bt=on next*3 ok wait:100 down*2 ok expect-screen:BtScan".into()),
        ("bt connect", "set:bt=on next*3 ok wait:100 down*2 ok wait:500 ok expect-screen:BtConnect".into()),
        ("bt paired", "set:bt=on next*3 ok wait:100 down*3 ok expect-screen:BtPaired".into()),
        ("recorder", "next*4 ok wait:200 expect-screen:Recorder".into()),
        ("recording", "set:recsignal=tone next*4 ok wait:200 ok wait:1500".into()),
        ("recordings", "next*4 ok wait:200 menu down*8 ok wait:200 expect-screen:Recordings".into()),
        ("steps", "set:steps=4321 next*6 ok expect-screen:Steps".into()),
        ("steps week", "set:week=3000/9000/12000/500/7000/0/4321 next*6 ok next".into()),
        ("stopwatch", "next*7 ok wait:100 expect-screen:Stopwatch".into()),
        ("timer", "next*7 ok wait:100 next".into()),
        ("alarms", "next*7 ok wait:100 prev".into()),
        ("alarm days", "next*7 ok wait:100 prev menu down ok wait:100 expect-screen:AlarmDays".into()),
        ("alert", "set:0x0d32=1 set:clock=06:59:58 set:clockrun=1 wait:3000 expect-screen:Alert".into()),
        ("authenticator", "set:vault=unlocked goto:Authenticator wait:100".into()),
        ("authenticator code", "set:vault=unlocked goto:Authenticator wait:100 ok wait:50".into()),
        ("authenticator pad", "goto:Authenticator wait:100".into()),
        ("usb drive", DRIVE.into()),
        ("drive files", format!("{DRIVE} ok wait:100 expect-screen:DriveFiles")),
        ("drive options", format!("{DRIVE} ok wait:100 expect-screen:DriveFiles down ok expect-screen:DriveOptions")),
        (
            "drive status",
            format!(
                "{DRIVE} ok wait:100 expect-screen:DriveFiles down ok expect-screen:DriveOptions ok wait:400 \
                 expect-screen:DriveStatus"
            ),
        ),
        ("settings", "prev ok expect-screen:Settings".into()),
        ("choice", "prev ok down*2 ok down*3 ok expect-screen:Choice".into()),
        ("slider", "prev ok down*2 ok ok expect-screen:Slider".into()),
        ("picker", "prev ok down*4 ok ok expect-screen:Picker".into()),
        ("date picker", "prev ok down*4 ok down ok expect-screen:Picker".into()),
        ("about", "prev ok down*11 ok ok expect-screen:Info".into()),
        ("battery", "prev ok down*3 ok down*4 ok expect-screen:Info".into()),
        ("home apps", "prev ok down*2 ok down*11 ok expect-screen:HomeApps".into()),
        ("developer", "prev ok down*11 ok ok expect-screen:Info ok*5 wait:200 back up ok expect-screen:Developer".into()),
        ("update", "prev ok down*11 ok down ok wait:300 expect-screen:UpdateFirmware".into()),
        ("reset dialog", "prev ok down*11 ok down*4 ok".into()),
        ("format dialog", "prev ok down*10 ok down*4 ok".into()),
        ("speech rate", "prev ok up ok down*4 ok expect-screen:Slider".into()),
        ("games", GAMES.into()),
        ("blocks", "set:gameseed=7 goto:Blocks wait:100 ok wait:1500".into()),
        ("4096", "set:gameseed=7 goto:Game4096 wait:100 right down".into()),
        ("solitaire", "set:gameseed=7 goto:Solitaire wait:100 up ok next ok".into()),
        ("salvo", "set:gameseed=7 goto:Salvo wait:100 up right press:ok release:ok wait:1000".into()),
        ("bricks", "set:gameseed=7 goto:Bricks wait:100 press:prev wait:200 release:prev ok wait:800".into()),
        ("game pause", "set:gameseed=7 goto:Blocks wait:100 ok wait:1500 back expect-screen:GamePause".into()),
        (
            "game settings",
            "set:gameseed=7 goto:Blocks wait:100 ok wait:1500 back expect-screen:GamePause down*2 ok \
             expect-screen:GameSettings"
                .into(),
        ),
        ("game help", "set:gameseed=7 goto:Blocks wait:100 ok wait:1500 back down*3 ok expect-screen:GameHelp".into()),
        ("books", BOOKS.into()),
        ("reader", format!("{BOOKS} down ok wait:50 expect-screen:Reader")),
        ("book menu", format!("{BOOKS} down ok wait:50 expect-screen:Reader ok wait:50 expect-screen:BookMenu")),
        (
            "book contents",
            format!("{BOOKS} down*3 ok wait:50 expect-screen:Reader ok wait:50 ok wait:50 expect-screen:BookContents"),
        ),
        ("book go to", format!("{BOOKS} down ok wait:50 goto:BookGoTo")),
        ("book bookmarks", format!("{BOOKS} down ok wait:50 goto:BookBookmarks")),
        ("sequencer", SEQ.into()),
        ("seq grid", GRID.into()),
        ("seq transport", format!("{GRID} goto:SeqTransport")),
        ("seq step", format!("{GRID} goto:SeqStep")),
        ("seq track", format!("{GRID} goto:SeqTrack")),
        ("seq mixer", format!("{GRID} goto:SeqMixer")),
        ("seq sound", format!("{GRID} menu down*9 ok wait:50 expect-screen:SeqSound")),
        ("seq settings", format!("{SEQ} menu ok wait:50 expect-screen:SeqSettings")),
        ("seq song", format!("{GRID} menu down*11 ok down ok wait:50 expect-screen:SeqSong")),
        (
            "seq live",
            "set:seq=on set:seqsong=1 goto:Sequencer wait:50 down ok wait:50 expect-screen:SeqGrid menu down*11 ok ok \
             wait:50 expect-screen:SeqLive"
                .into(),
        ),
        ("seq keys", format!("{GRID} down*8 menu down*10 ok wait:50 expect-screen:SeqKeys")),
    ];
    for g in 0..13 {
        v.push(("settings group", format!("prev ok down*{g} ok expect-screen:SettingsGroup")));
    }
    v
}

/// Every screen of the apps (`AppScreen::name`).
const ALL_SCREENS: &[&str] = &[
    "Boot",
    "Home",
    "Settings",
    "SettingsGroup",
    "Choice",
    "Info",
    "HomeApps",
    "PowerMenu",
    "QuickPanel",
    "UsbConnect",
    "Music",
    "Bookmarks",
    "Properties",
    "Developer",
    "Slider",
    "Picker",
    "Lock",
    "Notice",
    "Charging",
    "Goodbye",
    "Files",
    "Folders",
    "Pictures",
    "Recordings",
    "DriveFiles",
    "TextViewer",
    "NowPlaying",
    "Equaliser",
    "PictureViewer",
    "MusicLibrary",
    "Bluetooth",
    "BtScan",
    "BtPaired",
    "BtConnect",
    "Radio",
    "RadioPresets",
    "Steps",
    "Stopwatch",
    "AlarmDays",
    "Alert",
    "Authenticator",
    "Recorder",
    "UsbDrive",
    "DriveOptions",
    "DriveStatus",
    "UpdateFirmware",
    "Games",
    "GamePause",
    "GameSettings",
    "Blocks",
    "Game4096",
    "Solitaire",
    "Salvo",
    "Bricks",
    "GameHelp",
    "Books",
    "BookMenu",
    "BookContents",
    "BookBookmarks",
    "Reader",
    "BookGoTo",
    "Sequencer",
    "SeqGrid",
    "SeqTransport",
    "SeqStep",
    "SeqTrack",
    "SeqMixer",
    "SeqSound",
    "SeqSettings",
    "SeqSong",
    "SeqKeys",
    "SeqLive",
];

/// What the reader would say on entering what `s` shows, at `v`.
fn entry_speech(d: &Desc, v: Verbosity) -> String {
    let mut r = Reader::new();
    let mut c = Capture::default();
    r.observe(d, Cause::Other, &Config { on: true, verbosity: v, ..Config::DEFAULT }, &mut c);
    c.0
}

fn describe(s: &mut AppSim) -> Desc {
    let mut d = Desc::new();
    s.shell.describe(&mut s.sys, &mut d);
    d
}

/// What the lint found: problems, and one line per focus checked (the
/// coverage report).
#[derive(Default)]
struct Lint {
    fails: Vec<String>,
    lines: Vec<String>,
}

/// The lint for what `s` shows now.
fn check(s: &mut AppSim, ctx: &str, lint: &mut Lint) -> Desc {
    let fails = &mut lint.fails;
    let d = describe(s);
    let at = format!(
        "{ctx} ({}{})",
        s.screen(),
        match d.layer {
            orpheus_ui::a11y::Layer::Screen => "",
            orpheus_ui::a11y::Layer::Menu => ", menu",
            orpheus_ui::a11y::Layer::Dialog => ", dialog",
        }
    );
    if let Some(p) = d.problem() {
        fails.push(format!("{at}: {p}: title {:?} focus {:?} summary {:?}", d.title, d.focus, d.summary));
    }
    for v in [Verbosity::Brief, Verbosity::Normal, Verbosity::Detailed] {
        let said = entry_speech(&d, v);
        if orpheus_ui::a11y::speakable_len(&said) == 0 {
            fails.push(format!("{at}: nothing said at {v:?}"));
        }
        if v == Verbosity::Normal {
            let role = d.focus.map_or("-", |f| match f.role.word() {
                "" => "item",
                w => w,
            });
            lint.lines.push(format!("{at:<40} {role:<11} {}", said.trim().replace('\n', " | ")));
        }
    }
    d
}

/// Down through every focus (until it comes back to the first, or 40),
/// checking each.
fn walk_focus(s: &mut AppSim, ctx: &str, lint: &mut Lint) -> usize {
    let first = check(s, ctx, lint);
    let Some(f0) = first.focus else { return 0 };
    let n = f0.count.clamp(1, 40);
    let mut steps = 0;
    for _ in 1..n {
        run(s, "down");
        steps += 1;
        let d = check(s, ctx, lint);
        match d.focus {
            Some(f) if f.index == f0.index && f.label == f0.label => break,
            None => break,
            _ => {}
        }
    }
    steps
}

/// A11Y-1 (05 §7.14 "a host lint test fails for any focusable item
/// without one"): every screen reached by the journeys, every focus on it
/// and every item of its menu say something; the journeys reach every
/// screen of the apps.
#[test]
fn lint() {
    let mut lint = Lint::default();
    let mut seen = BTreeSet::new();
    let mut foci = 0;
    for (what, script) in lint_journeys() {
        let mut s = AppSim::new();
        if let Err(e) = s.run_script(&script) {
            lint.fails.push(format!("{what}: journey: {e:#}"));
            continue;
        }
        let screen = s.screen();
        seen.insert(screen);
        foci += 1 + walk_focus(&mut s, what, &mut lint);
        // Its menu, item by item (a dialog stays as it is).
        if !s.shell.dialog_open() && s.screen() == screen {
            run(&mut s, "menu");
            if s.shell.menu_open() {
                foci += 1 + walk_focus(&mut s, &format!("{what} menu"), &mut lint);
            }
        }
    }
    // The coverage report: what the reader says on each focus checked.
    let out = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target"), PathBuf::from)
        .join("orpheus");
    let _ = std::fs::create_dir_all(&out);
    let _ = std::fs::write(out.join("a11y-lint.txt"), lint.lines.join("\n") + "\n");
    let missing: Vec<&&str> = ALL_SCREENS.iter().filter(|n| !seen.contains(**n)).collect();
    assert!(missing.is_empty(), "screens no lint journey reaches: {missing:?}");
    assert!(lint.fails.is_empty(), "{} problems:\n  {}", lint.fails.len(), lint.fails.join("\n  "));
    assert!(foci > 300, "{foci} foci checked");
}

// ------------------------------------------------------------ spoken goldens

/// The spoken journeys (05 §7.14, A11Y-1): Home → Music → a list → Now
/// Playing; Settings (a switch, a value, the speech rate); a dialog; the
/// lock; the chords; keys with the screen off.
fn spoken_journeys() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "music_to_now_playing",
            "set:library=ready set:0x1004=1 wait:2500 ok wait:2500 down wait:2000 ok wait:2500 down wait:1500 ok \
             wait:2000 ok wait:2500 ok wait:3000 expect-screen:NowPlaying",
        ),
        (
            "settings",
            "set:0x1004=1 wait:2500 prev wait:1500 ok wait:2500 down*2 wait:1500 ok wait:2500 down*5 wait:2000 ok \
             wait:1500 back wait:2000 up wait:2000",
        ),
        (
            "speech_rate",
            "set:0x1004=1 wait:2500 prev ok wait:2500 up wait:2000 ok wait:2500 down*4 wait:2500 ok wait:2500 \
             up*4 wait:1500 back wait:3000",
        ),
        (
            "dialog",
            "set:0x1004=1 wait:2500 prev ok down*11 ok down*4 wait:2500 ok wait:4000 prev wait:1500 back wait:2500",
        ),
        (
            "lock",
            "set:0x1004=1 set:playing=1 wait:2500 power wait:1500 down wait:2500 wait:2000 power wait:3000",
        ),
        (
            "chords",
            "chord:back+menu wait:2500 chord:back+ok:800 wait:5000 chord:back+ok:2600 wait:3000 chord:back+menu wait:2000",
        ),
        (
            "screen_off",
            "set:0x1004=1 set:timers=run set:0x0302=0 wait:2500 wait:4000 next wait:2500 ok wait:2500 back wait:2000",
        ),
        ("interrupt", "set:0x1004=1 wait:300 ok wait:200 down down down wait:2500"),
    ]
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("golden/speech")
}

/// The golden text of a journey: its script, what the reader did, the audio.
fn spoken_record(name: &str, script: &str) -> (String, Vec<i16>) {
    let mut s = sim(script);
    s.a11y.as_mut().unwrap().speech.drain(20_000);
    let sp = &s.a11y.as_ref().unwrap().speech;
    let mut out = format!("# A11Y-1 spoken journey `{name}` (sim/tests/screen_reader.rs)\n# {script}\n");
    for l in &sp.log {
        out.push_str(&l.line());
        out.push('\n');
    }
    out.push_str(&format!(
        "audio 16000 Hz mono, {} samples, fnv64 {:016x}\n",
        sp.rendered(),
        sp.audio_hash()
    ));
    (out, sp.audio.clone())
}

#[test]
fn spoken() {
    let dir = golden_dir();
    let bless = std::env::var_os("ORPHEUS_BLESS").is_some();
    let wavs = std::env::var_os("ORPHEUS_SPEECH_WAVS").map(PathBuf::from);
    let mut fails = Vec::new();
    for (name, script) in spoken_journeys() {
        let (got, audio) = spoken_record(name, script);
        if let Some(w) = &wavs {
            std::fs::create_dir_all(w).unwrap();
            std::fs::write(w.join(format!("{name}.wav")), orpheus_sim::speech::wav_bytes(&audio)).unwrap();
        }
        let path = dir.join(format!("{name}.txt"));
        if bless {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &got).unwrap();
            continue;
        }
        let want = std::fs::read_to_string(&path).unwrap_or_default();
        if want != got {
            fails.push(format!("{name}: differs from {}:\n--- want\n{want}\n--- got\n{got}", path.display()));
        }
        // The same twice (the simulator is deterministic).
        assert_eq!(spoken_record(name, script).0, got, "{name}");
    }
    assert!(fails.is_empty(), "{}", fails.join("\n"));
}

/// Every utterance of the spoken journeys converts to words the speech
/// engine says (no symbol left over that it would spell or skip).
#[test]
fn spoken_text_is_plain_words() {
    for (name, script) in spoken_journeys() {
        let s = sim(script);
        for t in texts(&s) {
            assert!(t.chars().all(|c| c.is_alphanumeric() || " ,.:%'-()?!".contains(c)), "{name}: {t:?}");
            let mut words = 0;
            let mut conv =
                orpheus_tts::Converter::new(orpheus_tts::Options::default(), orpheus_tts::Speed::NORMAL);
            conv.convert(t.as_bytes(), |_, tok| {
                words += usize::from(matches!(tok, orpheus_tts::Token::Word(_)));
            });
            assert!(words > 0, "{name}: {t:?} has no words");
        }
    }
}

// ------------------------------------------------------------ behaviour

/// A key stops the current utterance (`Stop::Now`): the speech is cut
/// within a few ms and the new focus is said.
#[test]
fn a_key_interrupts() {
    let mut s = sim("set:0x1004=1 wait:300");
    assert!(s.a11y.as_ref().unwrap().speech.busy(), "still saying Home");
    run(&mut s, "down");
    let l = log(&s);
    let stop = l.iter().position(|x| matches!(x, Said::Stop(_))).expect("stopped");
    assert!(matches!(&l[stop + 1], Said::Text(_, t) if t.contains("5 of 12")), "{l:?}");
}

/// The toggle chord (Back and Menu held 2 s) turns the reader on and off
/// from any screen, also while it is off; a shorter hold does nothing, and
/// the two keys do nothing else (no Home, no quick panel, no menu).
#[test]
fn toggle_chord() {
    let mut s = sim("next ok wait:100 expect-screen:Files");
    run(&mut s, "chord:back+menu:1500");
    assert_eq!(s.sys.setting(orpheus_apps::settings::id::READER), 0, "1.5 s is not enough");
    assert_eq!(s.screen(), "Files", "and nothing else happened");
    assert!(!s.shell.menu_open());
    run(&mut s, "chord:back+menu");
    assert_eq!(s.sys.setting(orpheus_apps::settings::id::READER), 1);
    assert_eq!(s.screen(), "Files");
    assert_eq!(texts(&s)[0], "Screen reader on");
    assert!(texts(&s)[1].starts_with("Files. "), "{:?}", texts(&s));
    assert_eq!(earcons(&s)[0], Earcon::ReaderOn);
    run(&mut s, "chord:back+menu");
    assert_eq!(s.sys.setting(orpheus_apps::settings::id::READER), 0);
    assert_eq!(texts(&s).last().map(String::as_str), Some("Screen reader off"));
    let n = texts(&s).len();
    run(&mut s, "down ok wait:500");
    assert_eq!(texts(&s).len(), n, "off: nothing more is said");
}

/// Where am I (Back and OK by default): the screen, its summary and the
/// focus with its position at Detailed verbosity; held 2 s, the time and
/// the battery.
#[test]
fn where_am_i_and_time() {
    let mut s = sim(&format!("set:0x1004=1 set:clock=21:05:00 {PLAY} goto:NowPlaying wait:3000"));
    run(&mut s, "chord:back+ok:700");
    let t = texts(&s);
    let w = t.last().unwrap();
    assert!(w.starts_with("Now Playing. "), "{w}");
    assert!(w.contains(" of "), "position and times: {w}");
    assert_eq!(s.screen(), "NowPlaying", "the keys did nothing else");
    run(&mut s, "chord:back+ok:2200");
    let t = texts(&s);
    assert!(t.last().unwrap().starts_with("21:05. Battery "), "{t:?}");
}

/// Power locks the keys: "Keys locked"; a key then says the lock notice;
/// Power again: "Unlocked" and the screen.
#[test]
fn lock_and_unlock_are_said() {
    let s = sim("set:0x1004=1 wait:2000 power wait:1500 down wait:2500 wait:2000 power wait:2000");
    let t = texts(&s);
    let i = t.iter().position(|x| x == "Keys locked").expect("locked");
    assert!(t[i + 1].starts_with("Keys locked. "), "the notice: {t:?}");
    assert!(t.iter().any(|x| x.contains("Press Power to unlock")));
    let u = t.iter().position(|x| x == "Unlocked").expect("unlocked");
    assert!(t[u + 1].starts_with("Home. "), "{t:?}");
    // The notice ending under the lock is not said.
    assert_eq!(t.iter().filter(|x| x.starts_with("Home. ")).count(), 2, "{t:?}");
}

/// The earcons: a screen entered and left, a menu, a dialog, a list
/// rolled round, a switch turned.
#[test]
fn earcons_for_roles_and_events() {
    let s = sim(
        "set:0x1004=1 wait:500 prev ok wait:500 up wait:300 down wait:300 back wait:300 menu wait:300 back",
    );
    let e = earcons(&s);
    for want in [Earcon::ReaderOn, Earcon::Enter, Earcon::Wrap, Earcon::Back, Earcon::Menu] {
        assert!(e.contains(&want), "{want:?} in {e:?}");
    }
    let d = sim("set:0x1004=1 wait:500 prev ok down*11 ok down*4 ok wait:300");
    assert!(earcons(&d).contains(&Earcon::Dialog), "{:?}", earcons(&d));
    // A switch: Settings › Display › List roll-around.
    let mut sw = sim("set:0x1004=1 wait:500 prev ok down*2 ok wait:300");
    let rows = describe(&mut sw);
    assert!(rows.focus.is_some());
}

// ------------------------------------------------------------ effects

/// Speech of `journey` with setting `key` at `v` (the reader on), the
/// texts, the audio hash, the frame.
fn speak_with(key: u16, v: i16, journey: &str) -> (Vec<String>, u64, u64, Vec<Earcon>) {
    let mut s = sim(&format!("set:0x{key:04x}={v} set:0x1004=1 {journey}"));
    s.a11y.as_mut().unwrap().speech.drain(20_000);
    let f = frame(&mut s);
    (texts(&s), hash(&s), f, earcons(&s))
}

const JOURNEY: &str = "wait:2000 prev wait:1500 ok wait:2500 down*2 wait:1500 ok wait:2500";

/// Settings › Accessibility › Screen reader (0x1004): on, every journey is
/// spoken; off (default), nothing is, and the screens are the same.
#[test]
fn screen_reader_speaks_only_when_on() {
    let mut off = sim(&format!("set:0x1004=0 {JOURNEY}"));
    let mut on = sim(&format!("set:0x1004=1 {JOURNEY}"));
    assert!(texts(&off).is_empty());
    assert_eq!(off.a11y.as_ref().unwrap().speech.rendered(), 0, "no speech rendered while off");
    assert!(texts(&on).len() >= 4, "{:?}", texts(&on));
    assert_eq!(frame(&mut off), frame(&mut on), "the reader changes no frame");
}

/// Settings › Accessibility › Speech rate (0x1005, 0.25×–10× in 0.25×
/// steps): the same words, faster or slower speech (the audio's length
/// with the speech drained), the frames the same.
#[test]
fn speech_rate_changes_how_fast() {
    let length = |v: i16| {
        let mut s = sim(&format!("set:0x1005={v} set:0x1004=1 wait:100"));
        let sp = &mut s.a11y.as_mut().unwrap().speech;
        sp.drain(60_000);
        let last = sp.audio.iter().rposition(|x| x.unsigned_abs() > 100).unwrap_or(0);
        (texts(&s), last, frame(&mut s))
    };
    let (t1, l1, f1) = length(4);
    for v in [1i16, 2, 8, 40] {
        let (t, l, f) = length(v);
        assert_eq!(t, t1, "the same words at {v}");
        assert_eq!(f, f1, "the same frame at {v}");
        if v < 4 {
            assert!(l > l1 * 3 / 2, "{v}: {l} vs {l1}");
        } else {
            assert!(l < l1 * 2 / 3, "{v}: {l} vs {l1}");
        }
    }
}

/// Voice (0x1006) and Pitch (0x1007): the same words, another sound; the
/// frames the same.
#[test]
fn voice_and_pitch_change_the_sound() {
    for (key, values) in [(0x1006u16, &[1i16, 2][..]), (0x1007, &[0, 1, 3, 4][..])] {
        let def = orpheus_apps::settings::def(orpheus_apps::settings::SettingId(key)).unwrap().default;
        let (t0, h0, f0, _) = speak_with(key, def, JOURNEY);
        for &v in values {
            let (t, h, f, _) = speak_with(key, v, JOURNEY);
            assert_eq!(t, t0, "{key:#06x}={v}: the same words");
            assert_ne!(h, h0, "{key:#06x}={v}: another sound");
            assert_eq!(f, f0, "{key:#06x}={v}: the same frame");
        }
    }
}

/// Verbosity (0x1008): Brief leaves out roles and positions, Detailed adds
/// what OK does; the frames the same.
#[test]
fn verbosity_changes_what_is_said() {
    let (brief, _, fb, _) = speak_with(0x1008, 0, JOURNEY);
    let (normal, _, fnorm, _) = speak_with(0x1008, 1, JOURNEY);
    let (detailed, _, fd, _) = speak_with(0x1008, 2, JOURNEY);
    assert!(normal.iter().any(|t| t.contains(" of 13")), "{normal:?}");
    assert!(!brief.iter().any(|t| t.contains(" of 13") || t.contains("submenu")), "{brief:?}");
    assert!(detailed.iter().any(|t| t.contains("OK to open")), "{detailed:?}");
    assert!(!normal.iter().any(|t| t.contains("OK to open")));
    assert_eq!((fb, fd), (fnorm, fnorm));
}

/// Sound cues (0x1009): off, no earcons (the same words); the frames the
/// same.
#[test]
fn earcons_follow_the_setting() {
    let (t1, h1, f1, e1) = speak_with(0x1009, 1, JOURNEY);
    let (t0, h0, f0, e0) = speak_with(0x1009, 0, JOURNEY);
    assert!(e1.len() >= 3, "{e1:?}");
    assert!(e0.is_empty(), "{e0:?}");
    assert_eq!(t0, t1);
    assert_ne!(h0, h1);
    assert_eq!(f0, f1);
}

/// Where am I (0x100A): Back + OK (default) or Prev + Next say where am
/// I; Off: the pair does nothing (its keys go to the screen as keys
/// pressed together would).
#[test]
fn where_am_i_keys_follow_the_setting() {
    let said = |v: i16, chord: &str| {
        let s = sim(&format!("set:0x100a={v} set:0x1004=1 prev ok wait:2500 {chord} wait:100"));
        texts(&s).last().cloned().unwrap_or_default()
    };
    assert!(said(1, "chord:back+ok:700").contains("OK to open"), "Detailed: where am I");
    assert!(!said(1, "chord:prev+next:700").contains("OK to open"));
    assert!(said(2, "chord:prev+next:700").contains("OK to open"));
    assert!(!said(2, "chord:back+ok:700").contains("OK to open"));
    assert!(!said(0, "chord:back+ok:700").contains("OK to open"));
    assert!(!said(0, "chord:prev+next:700").contains("OK to open"));
    // Without the reader the pair is two keys.
    let s = sim("set:0x100a=1 prev ok wait:100 chord:back+ok:700 wait:100");
    assert_ne!(s.screen(), "Settings", "Back went back: no chord while the reader is off");
}

/// Keys with screen off (0x100B, on by default): with the reader on and
/// the screen off by the backlight timeout, keys work (the focus moves and
/// is said) and the screen stays dark; off, the first key only wakes it.
#[test]
fn keys_work_with_the_screen_off() {
    let run_with = |v: i16| {
        let s = sim(&format!(
            "set:0x100b={v} set:0x1004=1 set:timers=run set:0x0302=0 wait:2000 wait:6000 down wait:1500"
        ));
        (s.shell.gate.screen_on, texts(&s), s.backlight())
    };
    let (lit_on, said_on, bl_on) = run_with(1);
    assert!(!lit_on && bl_on == 0, "the screen stays dark");
    assert!(said_on.last().unwrap().contains("5 of 12"), "Down moved the focus: {said_on:?}");
    let (lit_off, said_off, _) = run_with(0);
    assert!(lit_off, "the key woke the screen");
    assert!(!said_off.iter().any(|t| t.contains("5 of 12")), "and did nothing else: {said_off:?}");
    // Without the reader the setting does nothing.
    let s = sim("set:0x100b=1 set:timers=run set:0x0302=0 wait:8000 down wait:100");
    assert!(s.shell.gate.screen_on, "woken");
}
