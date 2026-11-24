//! Golden-image tests (07 §3.2). Each case is a key script; its `shot:`
//! frames are encoded as PNG and compared byte-for-byte with
//! `sim/golden/<name>.png`. `YOTON_BLESS=1` (or `cargo xtask test golden
//! --bless`) rewrites them. Every run writes a contact sheet to
//! `target/yoton/golden.html` with old, new and diff images.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use yoton_sim::{AppSim, DemoSim, Shot, png_bytes};

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
    target.join("yoton")
}

fn bless() -> bool {
    std::env::var("YOTON_BLESS").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// Scripts starting with `@apps` run the `yoton-apps` screens; the others
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
        ("files_notice", "next ok down*3 ok wait:100 ok expect-screen:Notice"),
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
        ("not_yet", "next*2 ok expect-screen:Notice"),
        ("battery_low", "set:battery=5 expect-screen:Notice"),
        ("charging", "set:charge=charging set:battery=64 goto:Charging expect-screen:Charging"),
        ("usb", "set:usb=host expect-screen:UsbConnect"),
        ("boot", "goto:Boot expect-screen:Boot"),
        ("goodbye", "hold:power ok expect-screen:Goodbye"),
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
    fb.iter().flat_map(|p| yoton_ui::color::to_rgb888(*p)).collect()
}

/// Compare a new frame (and its PNG) with the golden PNG bytes.
fn compare(new_fb: &[u16], new_png: &[u8], old_png: Option<&[u8]>) -> (Verdict, Option<Vec<u8>>) {
    let Some(old) = old_png else { return (Verdict::Missing, None) };
    if old == new_png {
        return (Verdict::Same, None);
    }
    let dir = std::env::temp_dir().join(format!("yoton-golden-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(format!("{:x}.png", old.len() ^ new_png.len() ^ (old.as_ptr() as usize)));
    std::fs::write(&p, old).unwrap();
    let decoded = yoton_host::image::read_png_rgb(&p);
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
    let fb: Vec<u16> = rgb.chunks(3).map(|c| yoton_ui::color::rgb(c[0], c[1], c[2])).collect();
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
        "<!doctype html><meta charset=utf-8><title>yoton golden images</title>\
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
