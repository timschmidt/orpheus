//! Golden-image tests (07 §3.2). Each case is a key script; its `shot:`
//! frames are encoded as PNG and compared byte-for-byte with
//! `sim/golden/<name>.png`. `ORPHEUS_BLESS=1` (or `cargo xtask test golden
//! --bless`) rewrites them. Every run writes a contact sheet to
//! `target/orpheus/golden.html` with old, new and diff images.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use orpheus_sim::{AppSim, DemoSim, Shot, png_bytes};

const THEMES: [&str; 2] = ["dark", "light"];
const LANGS: [&str; 3] = ["latin", "cyrillic", "cjk"];

/// (shot name, script). The script is run on a fresh simulator and must
/// take exactly one shot with that name.
fn cases() -> Vec<(String, String)> {
    let mut v = Vec::new();
    let mut add = |name: String, script: String| v.push((name.clone(), format!("{script} shot:{name}")));
    for t in THEMES {
        for l in LANGS {
            let p = format!("theme:{t} lang:{l}");
            // T-4a's dummy screen: Home.
            add(format!("home_{t}_{l}"), format!("{p} expect-screen:Home"));
            // ListView: headers, icons, values, chevrons; marquee at rest.
            add(format!("list_{t}_{l}"), format!("{p} ok expect-screen:Library down*6 wait:300"));
            // Focused long row scrolling.
            add(format!("marquee_{t}_{l}"), format!("{p} ok down*6 wait:2300"));
            // Menu sheet, then a sub-menu with radios and a title.
            add(format!("menu_{t}_{l}"), format!("{p} ok down menu"));
            add(format!("submenu_{t}_{l}"), format!("{p} ok menu down*2 ok"));
            // Destructive dialog over the dimmed list.
            add(format!("dialog_{t}_{l}"), format!("{p} ok down menu down*5 ok"));
            // Toast over the strip.
            add(format!("toast_{t}_{l}"), format!("{p} ok down menu down*4 ok wait:200"));
            // Toggles, value, radios.
            add(format!("settings_{t}_{l}"), format!("{p} down ok expect-screen:Settings down*5"));
            // Checks and section header.
            add(format!("checks_{t}_{l}"), format!("{p} down ok down*9"));
            // Now Playing (strip used for a hint, no mini-player).
            add(format!("player_{t}_{l}"), format!("{p} down*2 ok expect-screen:NowPlaying"));
            // Mini-player in the strip while browsing, title scrolled.
            add(format!("mini_{t}_{l}"), format!("{p} set:playing=1 ok down*4 wait:1800"));
        }
        let p = format!("theme:{t}");
        add(format!("volume_{t}"), format!("{p} ok down vol+ vol+"));
        add(format!("volume_max_{t}"), format!("{p} set:volume=29 vol+ vol+"));
        add(format!("text_{t}"), format!("{p} down*3 ok expect-screen:Text"));
        add(format!("text_more_{t}"), format!("{p} down*3 ok down*3"));
        add(format!("widgets_{t}"), format!("{p} down*4 ok expect-screen:Widgets wait:400"));
        add(
            format!("status_{t}"),
            format!(
                "{p} set:charging=1 set:battery=55 set:bt=streaming set:source=radio set:sleep=1 set:alarm=1 set:activity=1"
            ),
        );
        add(
            format!("status_low_{t}"),
            format!("{p} set:battery=7 set:bt=connected set:source=recording set:locked=1 set:clock=off ok"),
        );
        add(format!("letter_{t}"), format!("{p} ok down*13 press:prev wait:800"));
        add(format!("pending_{t}"), format!("{p} ok down*20"));
    }
    add("amber_home".into(), "theme:amber".into());
    add("amber_menu".into(), "theme:amber ok down menu".into());
    add("accent_green_light".into(), "theme:light accent:green down ok down".into());
    add("accent_pink_dark".into(), "accent:pink ok down menu down*5 ok".into());
    app_cases(&mut add);
    v
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn golden_dir() -> PathBuf {
    root().join("golden")
}

fn out_dir() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| root().parent().unwrap_or(&root()).join("target"), PathBuf::from);
    target.join("orpheus")
}

fn bless() -> bool {
    std::env::var("ORPHEUS_BLESS").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// Scripts starting with `@apps` run the `orpheus-apps` screens; the others
/// the UI-1 widget demo.
fn run_case(script: &str) -> Vec<Shot> {
    let r = match script.strip_prefix("@apps ") {
        Some(s) => AppSim::new().run_script(s),
        None => DemoSim::new().run_script(script),
    };
    r.unwrap_or_else(|e| panic!("script `{script}`: {e:#}"))
}

/// UI-3 / UI-4: Files, text viewer, Music root, Now Playing views,
/// equaliser. `{album}` is the script's album (`set:queue=`), `{artist}`
/// the rows down to its artist folder in /Music, `{book}` the rows down to
/// its book in /Books, `{lyr}` the track with lyrics, `{track}` the rows
/// down to the first `.mp3` in the album folder (an `.lrc` may sort first).
fn media_cases(add: &mut dyn FnMut(String, String)) {
    let journeys: &[(&str, &str)] = &[
        ("files_root", "next ok wait:100 expect-screen:Files"),
        ("files_music", "next ok down*2 ok wait:100"),
        ("files_album", "next ok down*2 ok down*{artist} ok ok wait:100 down*{track}"),
        ("files_big", "next ok down*2 ok ok wait:150"),
        ("files_big_sorted", "next ok down*2 ok ok wait:600 down*3 wait:100"),
        ("files_letter", "next ok down*2 ok ok wait:600 press:next wait:700"),
        ("files_menu", "next ok down*2 ok down*{artist} ok ok wait:100 down*{track} menu"),
        ("files_sort", "next ok down*2 ok down*{artist} ok ok wait:100 down*{track} menu down*6 ok"),
        ("files_props", "next ok down*2 ok down*{artist} ok ok wait:100 down*{track} menu down*4 ok"),
        ("files_delete", "next ok down*2 ok down*{artist} ok ok wait:100 down*{track} menu down*5 ok"),
        ("files_hidden", "next ok menu down*4 ok menu wait:100"),
        ("files_notice", "next ok down ok wait:100 ok expect-screen:Notice"),
        ("text", "next ok ok wait:100 down*{book} ok wait:200 expect-screen:TextViewer"),
        ("text_page", "next ok ok wait:100 down*{book} ok wait:200 next wait:200 next wait:200"),
        ("music", "ok wait:100 expect-screen:Music"),
        ("music_playing", "set:queue={album} ok wait:100"),
        ("music_resume", "set:resume=1 ok wait:100"),
        ("music_folders", "ok down*8 ok wait:100 expect-screen:Folders"),
        ("bookmarks", "set:queue={album} set:pos=61000 set:bookmark=1 ok down*10 ok wait:100"),
        ("np", "set:queue={album} set:track=1 set:pos=47000 goto:NowPlaying wait:100"),
        ("np_paused", "set:queue={album} set:track=1 set:pos=47000 goto:NowPlaying ok wait:100"),
        ("np_bigart", "set:0x0d03=1 set:queue={album} goto:NowPlaying wait:100"),
        ("np_bigart_clean", "set:0x0d03=1 set:queue={album} goto:NowPlaying wait:3200"),
        (
            "np_lyrics",
            "set:0x0d03=2 set:queue={album} set:track={lyr} set:pos=25000 goto:NowPlaying wait:100",
        ),
        ("np_upnext", "set:0x0d03=3 set:queue={album} goto:NowPlaying wait:100 down"),
        ("np_details", "set:0x0d03=4 set:queue={album} set:track=1 goto:NowPlaying wait:100"),
        ("np_levels", "set:0x0d03=5 set:queue={album} set:pos=12000 goto:NowPlaying wait:100"),
        ("np_menu", "set:queue={album} goto:NowPlaying wait:100 menu"),
        ("np_idle", "goto:NowPlaying wait:100"),
        ("eq", "set:queue={album} goto:Equaliser wait:100"),
        ("eq_custom", "set:queue={album} goto:Equaliser up*3 next up*2 next down*2 wait:100"),
        ("eq_presets", "goto:Equaliser ok wait:100 expect-screen:Choice"),
    ];
    for t in ["dark", "light"] {
        for (l, album, artist, book, lyr, track) in
            [("latin", "takk", 2, 0, 1, 0), ("cyrillic", "kino", 4, 2, 0, 1), ("cjk", "yoasobi", 3, 3, 0, 1)]
        {
            for (name, script) in journeys {
                let script = script
                    .replace("{album}", album)
                    .replace("{artist}", &artist.to_string())
                    .replace("{book}", &book.to_string())
                    .replace("{lyr}", &lyr.to_string())
                    .replace("{track}", &track.to_string());
                add(format!("media_{name}_{t}_{l}"), format!("@apps theme:{t} lang:{l} {script}"));
            }
        }
    }
    pictures_cases(add);
    library_cases(add);
    bluetooth_cases(add);
    radio_cases(add);
    recorder_cases(add);
    steps_cases(add);
    tools_cases(add);
    auth_cases(add);
}

/// OTP-2 / USB-5: the Authenticator over the vault service and the fake
/// computer on the USB keyboard (`sim/src/auth.rs`; demo vault PIN 1234:
/// GitHub, Proton, Bank (HOTP, 8 digits), Mastodon, Госуслуги (60 s), AWS
/// (8 digits), 日本). Home grid: the ninth app. Pad keys: OK presses the
/// focused key (the cursor starts on 1), hold OK finishes. List menu on
/// the first row: Import, Move down, Delete…, Time check, Change PIN,
/// Lock, Wipe vault…; on a big code: Type the code, (Next code,) Delete…,
/// Time check, Lock.
fn auth_cases(add: &mut dyn FnMut(String, String)) {
    let app = "goto:Authenticator wait:100 expect-screen:Authenticator";
    let open = format!("set:vault=unlocked {app}");
    let locked = format!("set:vault=locked {app}");
    let journeys: &[(&str, String)] = &[
        ("warning", "next*8 ok wait:100 expect-screen:Authenticator".into()),
        ("setup", format!("{app} ok")),
        ("setup_digits", format!("{app} ok ok next ok next ok down ok")),
        ("confirm", format!("{app} ok ok next ok next ok down ok hold:ok wait:50")),
        ("mismatch", format!("{app} ok ok*4 hold:ok ok*3 next ok hold:ok wait:50")),
        ("locked", locked.clone()),
        ("unlocking", format!("{locked} ok next ok next ok down ok press:ok wait:510")),
        ("wrong", format!("{locked} ok*4 hold:ok wait:200")),
        ("backoff", format!("set:vault=locked set:wrongpin=4 {app} ok ok")),
        ("list", open.clone()),
        ("list_end", format!("{open} down*6")),
        ("code", format!("{open} ok wait:50")),
        ("code_60s", format!("{open} down*4 ok wait:50")),
        ("code_hotp", format!("{open} down*2 ok wait:50")),
        ("code_hotp_next", format!("{open} down*2 ok wait:50 menu down ok wait:100")),
        ("menu", format!("{open} menu")),
        ("code_menu", format!("{open} ok wait:50 menu")),
        ("typed", format!("set:usb=host {open} ok wait:50 ok wait:100")),
        ("no_computer", format!("{open} ok wait:50 ok wait:100")),
        ("delete", format!("{open} menu down*2 ok")),
        ("time_check", format!("{open} menu down*3 ok")),
        ("no_clock", format!("set:clockvalid=0 {open}")),
        ("no_clock_code", format!("set:clockvalid=0 {open} ok wait:50")),
        ("empty", format!("set:vault=empty {app}")),
        ("imported", format!("set:vault=empty set:otpfile=demo {app} menu ok wait:100")),
        ("import_kept", format!("set:vault=empty set:otpfile=bad {app} menu ok wait:100")),
        ("no_file", format!("set:vault=empty {app} menu ok wait:100")),
        ("provision", format!("{open} set:provision=demo wait:50")),
        ("change_pin", format!("{open} menu down*4 ok")),
        ("wipe", format!("{open} menu down*6 ok")),
        ("damaged", format!("set:vault=damaged {app}")),
    ];
    for t in ["dark", "light"] {
        for l in ["latin", "cyrillic", "cjk"] {
            for (name, script) in journeys {
                add(format!("auth_{name}_{t}_{l}"), format!("@apps theme:{t} lang:{l} {script}"));
            }
        }
    }
    add("auth_list_amber".into(), format!("@apps theme:amber {open}"));
    add("auth_code_amber".into(), format!("@apps theme:amber {open} ok wait:50"));
    add("auth_locked_amber".into(), format!("@apps theme:amber {locked}"));
    // Settings › USB with the keyboard rows (group 9).
    add("auth_usb_settings_dark_latin".into(), "@apps prev ok down*8 ok down*5".into());
}

/// UI-5b: the Recorder over the recording session and the fake ADC
/// (`sim/src/recorder.rs`), writing to the simulated card. Home grid: the
/// Recorder is the fifth app. Menu rows while ready: Source, Format,
/// Quality, Automatic gain, Voice activation, Auto split, Monitor, Low-cut,
/// Recordings…. The stop question focuses Stop.
fn recorder_cases(add: &mut dyn FnMut(String, String)) {
    let app = "next*4 ok wait:200 expect-screen:Recorder";
    let two = format!(
        "{app} ok wait:1500 hold:ok ok wait:200 set:clock=12:45:10 ok wait:2500 hold:ok ok wait:3500"
    );
    let list = format!("{two} menu down*8 ok wait:200 expect-screen:Recordings");
    let journeys: &[(&str, String)] = &[
        ("ready", format!("{app} wait:1500")),
        ("recording", format!("set:recsignal=tone {app} ok wait:12340")),
        ("paused", format!("{app} ok wait:5000 ok wait:600")),
        ("waiting", format!("set:0x0806=3 set:recsignal=quiet {app} ok wait:1500")),
        ("fm", format!("set:0x0801=1 set:0x0803=2 {app} wait:1500 ok wait:3200")),
        ("clip", format!("set:0x0804=0 set:recsignal=loud {app} ok wait:1000")),
        (
            "dropped",
            format!("set:0x0803=2 set:recsignal=noise {app} ok wait:500 set:recstall=3000 wait:3000"),
        ),
        ("menu", format!("{app} menu")),
        ("menu_recording", format!("{app} ok wait:1000 menu")),
        ("source", format!("{app} menu ok")),
        ("format", format!("{app} menu down ok")),
        ("quality", format!("{app} menu down*2 ok")),
        ("vox", format!("{app} menu down*4 ok")),
        ("stop_ask", format!("{app} ok wait:2000 back")),
        ("saved", format!("{app} ok wait:2000 back ok wait:300")),
        ("gain", format!("set:0x0804=0 {app} wait:500 up up up wait:100")),
        ("list", list.clone()),
        ("list_menu", format!("{list} menu")),
        ("list_delete", format!("{list} menu down*2 ok")),
        ("list_empty", format!("{app} menu down*8 ok wait:200")),
    ];
    for t in ["dark", "light"] {
        for l in ["latin", "cyrillic", "cjk"] {
            for (name, script) in journeys {
                add(format!("recorder_{name}_{t}_{l}"), format!("@apps theme:{t} lang:{l} {script}"));
            }
        }
    }
    add(
        "recorder_recording_amber".into(),
        format!("@apps theme:amber set:recsignal=tone {app} ok wait:12340"),
    );
    add("recorder_list_amber".into(), format!("@apps theme:amber {list}"));
}

/// UI-5c: Steps over the fake accelerometer (`sim/src/steps.rs`). Home
/// grid: Steps is the seventh app (`next*6 ok`). The week: today (a
/// Tuesday) first.
fn steps_cases(add: &mut dyn FnMut(String, String)) {
    let week = "set:week=6482/9120/3400/12010/7700/0/5230";
    let app = "next*6 ok wait:100 expect-screen:Steps";
    let journeys: &[(&str, String)] = &[
        ("today", format!("{week} {app}")),
        ("goal_reached", format!("set:steps=10420 {app}")),
        ("week", format!("{week} {app} next")),
        ("week_day", format!("{week} {app} next down*2")),
        ("menu", format!("{week} {app} menu")),
        ("goal", format!("{app} menu ok wait:100 expect-screen:Slider up*4")),
        ("reset", format!("{week} {app} menu down*6 ok")),
        ("walking", format!("set:walk=on {app} wait:20000")),
        ("no_sensor", format!("set:sensor=off {app}")),
        ("no_clock", format!("set:clockvalid=0 set:steps=120 {app}")),
        ("background_off", format!("set:0x0d24=0 {week} {app}")),
        ("miles", format!("set:0x0d23=1 {week} {app}")),
    ];
    for t in ["dark", "light"] {
        for l in ["latin", "cyrillic", "cjk"] {
            for (name, script) in journeys {
                add(format!("steps_{name}_{t}_{l}"), format!("@apps theme:{t} lang:{l} {script}"));
            }
        }
    }
    add("steps_today_amber".into(), format!("@apps theme:amber {week} {app}"));
}

/// UI-5d: Stopwatch · Timer · Alarm, the alert and the status bar
/// indicators. Home grid: Stopwatch is the eighth app (`next*7 ok`).
/// Tabs: Prev goes back a tab (Stopwatch → Alarm → Timer); Next leaves an
/// idle Stopwatch or Timer for the next tab.
fn tools_cases(add: &mut dyn FnMut(String, String)) {
    let app = "next*7 ok wait:100 expect-screen:Stopwatch";
    let laps = format!("{app} ok wait:12340 next wait:7000 next wait:3210 next wait:1500");
    let timer = format!("{app} next");
    let alarm = format!("{app} prev");
    let ring = "set:0x0d35=1 set:clock=07:29:58 set:clockrun=1 wait:3000 expect-screen:Alert";
    let journeys: &[(&str, String)] = &[
        ("sw_idle", app.into()),
        ("sw_running", laps.clone()),
        ("sw_stopped", format!("{laps} ok down")),
        ("sw_reset", format!("{laps} ok next")),
        ("timer", timer.clone()),
        ("timer_preset", format!("{timer} down*3")),
        ("timer_running", format!("{timer} ok wait:2500")),
        ("timer_paused", format!("{timer} down ok wait:61500 ok")),
        ("timer_picker", format!("{timer} menu ok wait:100 expect-screen:Picker next*2 up*5")),
        ("timer_alert", format!("{timer} ok wait:60100 expect-screen:Alert")),
        ("alarm", format!("set:0x0d35=1 {alarm}")),
        ("alarm_menu", format!("{alarm} menu")),
        ("alarm_picker", format!("{alarm} down menu ok wait:100 expect-screen:Picker up*2")),
        ("alarm_days", format!("{alarm} down*2 menu down ok wait:100 expect-screen:AlarmDays down*5")),
        ("alarm_alert", ring.into()),
        ("indicators", format!("set:0x0d32=1 {app} ok back wait:100 expect-screen:Home")),
    ];
    for t in ["dark", "light"] {
        for l in ["latin", "cyrillic", "cjk"] {
            for (name, script) in journeys {
                add(format!("tools_{name}_{t}_{l}"), format!("@apps theme:{t} lang:{l} {script}"));
            }
        }
    }
    add("tools_alarm_wake_dark_latin".into(), format!("@apps set:alarmwake=1 set:0x0d35=1 {alarm}"));
    add("tools_sw_running_amber".into(), format!("@apps theme:amber {laps}"));
}

/// UI-5a: the Radio app over the firmware's radio service and the fake
/// tuner (`orpheus_fm::fake`). Home grid: Radio is the third app. Menu
/// rows off a preset: Save preset, Presets…, Auto-scan, Force mono,
/// Region, Stop radio.
fn radio_cases(add: &mut dyn FnMut(String, String)) {
    let app = "goto:Radio wait:300 expect-screen:Radio";
    let journeys: &[(&str, String)] = &[
        ("playing", format!("set:presets=demo set:radio=101900 {app}")),
        ("open", "next*2 ok wait:1100 expect-screen:Radio".into()),
        ("starting", "goto:Radio wait:300 expect-screen:Radio".into()),
        ("text", format!("set:presets=demo set:radio=101900 {app} wait:2500")),
        ("muted", format!("set:radio=94700 {app} ok wait:100")),
        ("off", format!("set:radio=94700 {app} hold:ok wait:2200")),
        ("seeking", format!("set:seekreads=200 set:radio=94700 {app} hold:next wait:300")),
        ("scanning", format!("set:radio=94700 {app} menu down*2 ok wait:250")),
        ("scanned", format!("set:radio=94700 {app} menu down*2 ok wait:1500")),
        ("scan_ask", format!("set:presets=demo set:radio=94700 {app} menu down*2 ok")),
        ("menu", format!("set:presets=demo set:radio=101900 {app} menu")),
        ("region", format!("set:radio=94700 {app} menu down*4 ok")),
        (
            "presets",
            format!(
                "set:presets=demo set:radio=94700 {app} menu down ok wait:100 expect-screen:RadioPresets"
            ),
        ),
        ("presets_menu", format!("set:presets=demo set:radio=94700 {app} menu down ok wait:100 down menu")),
        ("presets_empty", format!("set:radio=94700 {app} menu down ok wait:100")),
        ("no_antenna", format!("set:antenna=0 set:radio=91500 {app}")),
        ("no_tuner", "set:tuner=absent goto:Radio wait:1500 expect-screen:Radio".into()),
        ("japan", format!("set:0x0701=3 set:radio=77100 {app}")),
        ("home", "set:radio=101900 wait:3000 expect-screen:Home".into()),
    ];
    for t in ["dark", "light"] {
        for l in ["latin", "cyrillic", "cjk"] {
            for (name, script) in journeys {
                add(format!("radio_{name}_{t}_{l}"), format!("@apps theme:{t} lang:{l} {script}"));
            }
        }
    }
    add("radio_playing_amber".into(), format!("@apps theme:amber set:presets=demo set:radio=101900 {app}"));
    add(
        "radio_presets_amber".into(),
        format!("@apps theme:amber set:presets=demo set:radio=94700 {app} menu down ok wait:100"),
    );
}

/// UI-6a: the Bluetooth app over the fake module (`sim/src/bt.rs`). Home
/// grid: Bluetooth is the fourth app (`next*3 ok`). App rows: Bluetooth,
/// state, (connected device), Pair new device, Paired devices, Reconnect
/// automatically.
fn bluetooth_cases(add: &mut dyn FnMut(String, String)) {
    let app = "next*3 ok wait:100 expect-screen:Bluetooth";
    let scan = format!("set:bt=on {app} down*2 ok expect-screen:BtScan");
    let journeys: &[(&str, String)] = &[
        ("off", app.into()),
        ("starting", format!("{app} ok wait:300")),
        ("ready", format!("set:bt=on {app}")),
        ("streaming", format!("set:bt=streaming {app} down")),
        ("dead", format!("set:bt=dead {app}")),
        ("disconnect", format!("set:bt=connected {app} down ok")),
        ("scan_live", format!("{scan} wait:1700")),
        ("scan_done", format!("{scan} wait:6000 down*2")),
        ("connecting", format!("{scan} wait:6000 ok wait:500 expect-screen:BtConnect")),
        ("connected", format!("{scan} wait:6000 ok wait:2200 expect-screen:BtScan")),
        ("failed", format!("{scan} wait:6000 down*3 ok wait:2300 expect-screen:BtConnect")),
        ("paired", format!("set:bt=connected {app} down*4 ok expect-screen:BtPaired")),
        ("paired_menu", format!("set:bt=connected {app} down*4 ok down menu")),
        ("forget", format!("set:bt=connected {app} down*4 ok menu down ok")),
        ("paired_empty", format!("set:bt=on {app} down*3 ok menu down ok down ok wait:100")),
    ];
    for t in ["dark", "light"] {
        for l in ["latin", "cyrillic", "cjk"] {
            for (name, script) in journeys {
                add(format!("bt_{name}_{t}_{l}"), format!("@apps theme:{t} lang:{l} {script}"));
            }
        }
    }
    add("bt_ready_amber".into(), format!("@apps theme:amber set:bt=on {app}"));
}

/// AU-12: the music library, built for real from the card image
/// (`set:library=ready`). Music root rows: Shuffle all, Artists, Albums,
/// Songs, Genres, Playlists, Recently added, Folders, Bookmarks.
fn library_cases(add: &mut dyn FnMut(String, String)) {
    let lib = "set:library=ready ok wait:100";
    let journeys: &[(&str, String)] = &[
        ("root", format!("{lib} expect-screen:Music")),
        ("artists", format!("{lib} down ok wait:100 expect-screen:MusicLibrary")),
        ("artist_albums", format!("{lib} down ok wait:100 down*2 ok wait:100")),
        ("album_tracks", format!("{lib} down ok wait:100 down*2 ok wait:100 ok wait:100 down")),
        ("albums", format!("{lib} down*2 ok wait:100 down*3")),
        ("songs", format!("{lib} down*3 ok wait:100")),
        ("genres", format!("{lib} down*4 ok wait:100")),
        ("recent", format!("{lib} down*6 ok wait:100")),
        ("menu", format!("{lib} down ok wait:100 menu")),
        ("play", format!("{lib} down ok wait:100 down*2 ok wait:100 ok wait:100 down ok wait:100")),
        ("np_menu", "set:library=ready set:queue=kino goto:NowPlaying wait:100 menu down*8".into()),
        (
            "goto_album",
            "set:library=ready set:queue=kino goto:NowPlaying wait:100 menu down*8 ok wait:100".into(),
        ),
        (
            "goto_artist",
            "set:library=ready set:queue=kino goto:NowPlaying wait:100 menu down*9 ok wait:100".into(),
        ),
    ];
    for (t, l) in [("dark", "latin"), ("light", "cjk")] {
        for (name, script) in journeys {
            add(format!("library_{name}_{t}_{l}"), format!("@apps theme:{t} lang:{l} {script}"));
        }
    }
}

/// AU-14: Pictures (folders and pictures only) and the viewer, on the
/// card's /Pictures: Aurora.png, Iceland.jpg (damaged), Lake.bmp,
/// Sunset.qoi (portrait), Москва.png (damaged).
fn pictures_cases(add: &mut dyn FnMut(String, String)) {
    let open = "next*5 ok wait:100 down*3 ok wait:100";
    let journeys: &[(&str, String)] = &[
        ("root", "next*5 ok wait:100 expect-screen:Pictures".into()),
        ("folder", open.into()),
        ("folder_menu", format!("{open} menu")),
        ("png", format!("{open} ok wait:100 expect-screen:PictureViewer")),
        ("damaged", format!("{open} ok wait:100 next wait:100")),
        ("bmp", format!("{open} ok wait:100 next*2 wait:100")),
        ("rotate", format!("{open} ok wait:100 next*2 down wait:100")),
        ("portrait", format!("{open} ok wait:100 next*3 wait:100")),
        ("fill", format!("{open} ok wait:100 next*3 ok wait:100")),
        ("menu", format!("{open} ok wait:100 menu")),
        ("busy", format!("set:queue=takk {open} ok wait:100")),
    ];
    for (t, l) in [("dark", "latin"), ("light", "cjk")] {
        for (name, script) in journeys {
            add(format!("pictures_{name}_{t}_{l}"), format!("@apps theme:{t} lang:{l} {script}"));
        }
    }
}

/// UI-2: Home, Settings and the system screens, through the real apps.
/// Settings is the last app on Home: `prev ok` opens it.
fn app_cases(add: &mut dyn FnMut(String, String)) {
    let journeys: &[(&str, &str)] = &[
        ("home", "expect-screen:Home"),
        ("home_playing", "set:playing=1 next wait:1100"),
        ("settings", "prev ok expect-screen:Settings"),
        ("sound", "prev ok ok expect-screen:SettingsGroup"),
        ("display", "prev ok down*2 ok down*3"),
        ("choice", "prev ok down*2 ok down*3 ok expect-screen:Choice"),
        ("slider", "prev ok down*2 ok ok expect-screen:Slider up*3"),
        ("time", "prev ok down*4 ok ok expect-screen:Picker next up"),
        ("date", "prev ok down*4 ok down ok expect-screen:Picker"),
        ("about", "prev ok down*11 ok ok expect-screen:Info"),
        ("card_info", "prev ok down*10 ok ok expect-screen:Info"),
        ("battery", "prev ok down*3 ok down*4 ok expect-screen:Info"),
        ("reset_dialog", "prev ok down*11 ok down*4 ok"),
        ("format_dialog", "prev ok down*10 ok down*4 ok"),
        ("home_apps", "prev ok down*2 ok down*10 ok expect-screen:HomeApps down*2"),
        ("power_menu", "hold:power expect-screen:PowerMenu"),
        ("quick", "set:playing=1 set:bt=connected hold:menu expect-screen:QuickPanel down*3 next"),
        ("lock", "set:playing=1 power down expect-screen:Lock"),
        ("no_card", "set:card=absent ok expect-screen:Notice"),
        // The Authenticator (the Recorder opens since UI-5b).
        ("not_yet", "down*2 next ok expect-screen:Notice"),
        ("battery_low", "set:battery=5 expect-screen:Notice"),
        ("charging", "set:charge=charging set:battery=64 goto:Charging expect-screen:Charging"),
        ("usb", "set:usb=host expect-screen:UsbConnect"),
        ("boot", "goto:Boot expect-screen:Boot"),
        ("goodbye", "hold:power ok expect-screen:Goodbye"),
        // PWR-5 / D-4: System › Performance, its choice, the developer
        // unlock in About and the Developer rows with a log running.
        ("performance", "prev ok down*11 ok down*6"),
        ("performance_choice", "prev ok down*11 ok down*6 ok expect-screen:Choice"),
        ("developer_unlock", "prev ok down*11 ok ok expect-screen:Info ok*5 wait:200"),
        (
            "developer",
            "set:developer=1 set:drain=on set:drainrows=42 prev ok down*11 ok down*7 ok expect-screen:Developer",
        ),
    ];
    for t in ["dark", "light"] {
        for l in ["latin", "cyrillic", "cjk"] {
            for (name, script) in journeys {
                add(format!("app_{name}_{t}_{l}"), format!("@apps theme:{t} lang:{l} {script}"));
            }
        }
    }
    // Every Settings group once (Dark, English) and in Light Japanese.
    for g in 0..12 {
        add(format!("app_group{g:02}_dark_latin"), format!("@apps prev ok down*{g} ok"));
        add(format!("app_group{g:02}_light_cjk"), format!("@apps theme:light lang:cjk prev ok down*{g} ok"));
    }
    media_cases(add);
    add("app_amber_home".into(), "@apps theme:amber".into());
    add("app_amber_settings".into(), "@apps theme:amber prev ok".into());
    add("app_accent_red_home".into(), "@apps accent:red".into());
}

/// Outcome of comparing one frame.
#[derive(Debug, PartialEq)]
enum Verdict {
    Same,
    Missing,
    /// Bytes differ; pixel count and bounding box of differing pixels
    /// (0 pixels: only the encoding differs).
    Differs {
        pixels: usize,
        bbox: Option<(usize, usize, usize, usize)>,
    },
}

fn rgb_of(fb: &[u16]) -> Vec<u8> {
    fb.iter().flat_map(|p| orpheus_ui::color::to_rgb888(*p)).collect()
}

/// Compare a new frame (and its PNG) with the golden PNG bytes.
fn compare(new_fb: &[u16], new_png: &[u8], old_png: Option<&[u8]>) -> (Verdict, Option<Vec<u8>>) {
    let Some(old) = old_png else { return (Verdict::Missing, None) };
    if old == new_png {
        return (Verdict::Same, None);
    }
    let dir = std::env::temp_dir().join(format!("orpheus-golden-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(format!("{:x}.png", old.len() ^ new_png.len() ^ (old.as_ptr() as usize)));
    std::fs::write(&p, old).unwrap();
    let decoded = orpheus_host::image::read_png_rgb(&p);
    let _ = std::fs::remove_file(&p);
    let new_rgb = rgb_of(new_fb);
    let Ok((128, 128, old_rgb)) = decoded else {
        return (Verdict::Differs { pixels: 128 * 128, bbox: None }, None);
    };
    let mut n = 0;
    let mut bbox: Option<(usize, usize, usize, usize)> = None;
    let mut diff = Vec::with_capacity(128 * 128 * 3);
    for i in 0..128 * 128 {
        let (a, b) = (&old_rgb[i * 3..i * 3 + 3], &new_rgb[i * 3..i * 3 + 3]);
        if a == b {
            // Faded copy of the frame for orientation.
            diff.extend(b.iter().map(|v| v / 4 + 40));
        } else {
            n += 1;
            let (x, y) = (i % 128, i / 128);
            bbox = Some(match bbox {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
            diff.extend([255, 0, 255]);
        }
    }
    (Verdict::Differs { pixels: n, bbox }, Some(diff))
}

fn rgb_png(rgb: &[u8]) -> Vec<u8> {
    // Pack RGB888 into RGB565-free PNG via the frame path: build a u16
    // frame that expands back to these colours closely enough for review.
    let fb: Vec<u16> = rgb.chunks(3).map(|c| orpheus_ui::color::rgb(c[0], c[1], c[2])).collect();
    png_bytes(&fb).unwrap()
}

struct Row {
    name: String,
    verdict: Verdict,
    blessed: bool,
}

#[test]
fn golden_images() {
    let gdir = golden_dir();
    let out = out_dir();
    let new_dir = out.join("golden").join("new");
    let diff_dir = out.join("golden").join("diff");
    let old_dir = out.join("golden").join("old");
    std::fs::create_dir_all(&old_dir).unwrap();
    std::fs::create_dir_all(&new_dir).unwrap();
    std::fs::create_dir_all(&diff_dir).unwrap();
    std::fs::create_dir_all(&gdir).unwrap();
    let blessing = bless();

    let mut rows = Vec::new();
    let mut names = BTreeSet::new();
    for (name, script) in cases() {
        let shots = run_case(&script);
        assert_eq!(shots.len(), 1, "case {name} must take exactly one shot");
        let shot = &shots[0];
        assert_eq!(shot.name, name);
        assert!(names.insert(name.clone()), "duplicate shot name {name}");
        let png = shot.png().unwrap();
        let gpath = gdir.join(format!("{name}.png"));
        let old = std::fs::read(&gpath).ok();
        let (verdict, diff) = compare(&shot.fb, &png, old.as_deref());
        std::fs::write(new_dir.join(format!("{name}.png")), &png).unwrap();
        if let Some(d) = diff {
            std::fs::write(diff_dir.join(format!("{name}.png")), rgb_png(&d)).unwrap();
            if let Some(o) = &old {
                std::fs::write(old_dir.join(format!("{name}.png")), o).unwrap();
            }
        } else {
            let _ = std::fs::remove_file(diff_dir.join(format!("{name}.png")));
        }
        let mut blessed = false;
        if blessing && verdict != Verdict::Same {
            std::fs::write(&gpath, &png).unwrap();
            blessed = true;
        }
        rows.push(Row { name, verdict, blessed });
    }

    // Golden files that no case produces any more.
    let mut stale = Vec::new();
    for e in std::fs::read_dir(&gdir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "png") {
            let stem = p.file_stem().unwrap().to_string_lossy().to_string();
            if !names.contains(&stem) {
                if blessing {
                    std::fs::remove_file(&p).unwrap();
                } else {
                    stale.push(stem);
                }
            }
        }
    }

    write_contact_sheet(&out, &rows, &stale);

    let failures: Vec<String> = rows
        .iter()
        .filter(|r| r.verdict != Verdict::Same && !r.blessed)
        .map(|r| format!("{}: {:?}", r.name, r.verdict))
        .collect();
    let sheet = out.join("golden.html");
    assert!(
        failures.is_empty() && stale.is_empty(),
        "{} golden image(s) differ, stale: {stale:?}\n{}\ncontact sheet: {}\nre-bless with `cargo xtask test golden --bless` after reviewing it",
        failures.len(),
        failures.join("\n"),
        sheet.display()
    );
}

fn write_contact_sheet(out: &Path, rows: &[Row], stale: &[String]) {
    let mut h = String::new();
    h.push_str(
        "<!doctype html><meta charset=utf-8><title>Orpheus golden images</title>\
         <style>body{font:14px system-ui;background:#222;color:#ddd;margin:16px}\
         img{width:256px;height:256px;image-rendering:pixelated;background:#000}\
         td{padding:4px 8px;vertical-align:top}.bad{color:#f66}.ok{color:#6c6}\
         .grid{display:flex;flex-wrap:wrap;gap:12px}.cell{width:256px}</style>",
    );
    let bad: Vec<&Row> = rows.iter().filter(|r| r.verdict != Verdict::Same).collect();
    let _ = write!(h, "<h1>Golden images: {} cases, {} changed</h1>", rows.len(), bad.len());
    if !stale.is_empty() {
        let _ = write!(h, "<p class=bad>Stale golden files: {}</p>", stale.join(", "));
    }
    if !bad.is_empty() {
        h.push_str("<h2>Changed</h2><table><tr><th>name</th><th>old</th><th>new</th><th>diff</th></tr>");
        for r in &bad {
            let n = &r.name;
            let _ = write!(
                h,
                "<tr><td class=bad>{n}<br>{:?}{}</td><td><img src=\"golden/old/{n}.png\"></td>\
                 <td><img src=\"golden/new/{n}.png\"></td><td><img src=\"golden/diff/{n}.png\"></td></tr>",
                r.verdict,
                if r.blessed { "<br>(blessed)" } else { "" },
            );
        }
        h.push_str("</table>");
    }
    h.push_str("<h2>All</h2><div class=grid>");
    for r in rows {
        let class = if r.verdict == Verdict::Same { "ok" } else { "bad" };
        let _ = write!(
            h,
            "<div class=cell><img src=\"golden/new/{}.png\"><div class={class}>{}</div></div>",
            r.name, r.name
        );
    }
    h.push_str("</div>");
    std::fs::write(out.join("golden.html"), h).unwrap();
}

/// T-4a acceptance: a single changed pixel makes the comparison fail, and
/// the diff locates it.
#[test]
fn one_changed_pixel_fails() {
    let (name, script) = cases().into_iter().next().unwrap();
    let shot = run_case(&script).pop().unwrap();
    let good = shot.png().unwrap();
    // The comparator accepts the identical frame …
    assert_eq!(compare(&shot.fb, &good, Some(&good)).0, Verdict::Same);
    // … and rejects the same frame with one pixel changed.
    let mut fb = shot.fb.clone();
    fb[37 * 128 + 91] ^= 0x0841;
    let bad = png_bytes(&fb).unwrap();
    assert_ne!(bad, good);
    let (v, diff) = compare(&fb, &bad, Some(&good));
    assert_eq!(v, Verdict::Differs { pixels: 1, bbox: Some((91, 37, 91, 37)) });
    assert!(diff.is_some());
    // Against the committed golden (unless it is being rewritten right now).
    if !bless() {
        let committed = std::fs::read(golden_dir().join(format!("{name}.png")))
            .expect("committed golden image (run `cargo xtask test golden --bless` once)");
        assert_eq!(compare(&shot.fb, &good, Some(&committed)).0, Verdict::Same, "{name} matches its golden");
        assert!(matches!(compare(&fb, &bad, Some(&committed)).0, Verdict::Differs { pixels: 1, .. }));
    }
}

/// Rendering is deterministic: the same script gives the same bytes.
#[test]
fn deterministic() {
    let (_, script) = cases().into_iter().nth(4).unwrap();
    let a = run_case(&script).pop().unwrap().png().unwrap();
    let b = run_case(&script).pop().unwrap().png().unwrap();
    assert_eq!(a, b);
}
