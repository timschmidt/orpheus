//! Orpheus in the browser (WEB-2, 09 §4): the real `orpheus-apps` screens
//! over the simulator's services (`orpheus-sim`), a FAT32 card built here
//! (demo set, the visitor's files, the library of the previous visit), the
//! firmware's codecs and DSP chain for sound, and the key-sequence recorder
//! in the fuzz corpus format. Compiled to `wasm32-unknown-unknown` and
//! bound to the page (`site/try/app.js`) with `wasm-bindgen` by
//! `cargo xtask wasm`; everything also builds and is tested on the host.
//!
//! The page drives it: wall-clock milliseconds in ([`Web::advance`]), key
//! edges in, RGBA frames and stereo PCM out, the persistent state as text.

#![deny(unsafe_code)]

pub mod audio;
pub mod bundle;
pub mod card;
pub mod demo;
pub mod rec;

/// The `ui_keys` fuzz target's body; the browser uses its operation
/// decoder (`command`) and limits.
#[allow(dead_code, clippy::all, clippy::pedantic)]
#[path = "../../fuzz/src/lib.rs"]
mod fuzz;

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::Arc;

use orpheus_apps::services::{CardInfo, FsKind, SettingsService};
use orpheus_apps::settings::{SettingId, id};
use orpheus_fs::DirEntry;
use orpheus_fs::host::{Base, block_on};
use orpheus_sim::AppSim;
use orpheus_sim::apps::HostServices;
use orpheus_sim::fallback::HexFont;
use orpheus_sim::media::Media;
use orpheus_ui::Key;
use wasm_bindgen::prelude::*;

/// Bumped when the demo set changes: a library saved for another card is
/// not restored.
const CARD_VERSION: u32 = 2;
/// The longest step simulated at once (a tab that slept catches up only
/// this far).
const MAX_STEP_MS: f64 = 5000.0;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn error(s: &str);
}

/// Panics go to the console (the module aborts after).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
fn start() {
    std::panic::set_hook(Box::new(|p| error(&p.to_string())));
}

/// One session in the page.
#[wasm_bindgen]
pub struct Web {
    /// The demo set ([`Web::add_demo`]).
    demo: Vec<card::File>,
    user: Vec<card::File>,
    library: Vec<card::File>,
    base: Option<Arc<[u8]>>,
    user_paths: Vec<String>,
    sim: Option<AppSim>,
    engine: audio::Engine,
    /// Frame counter and screen state of the last frame handed out.
    shown: Option<(u32, bool)>,
    /// Recording: start time and key edges.
    rec: Option<(u64, Vec<rec::Edge>)>,
    replay: VecDeque<String>,
}

impl Default for Web {
    fn default() -> Self {
        Self::new()
    }
}

fn json(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// The simulator over a card image, library loaded.
fn new_sim(base: &Arc<[u8]>, state: &str) -> Result<AppSim, String> {
    let mut media = Media::with_card(Base::Shared(Arc::clone(base))).ok_or("the card does not mount")?;
    media.duration_probe = Some(audio::duration_ms);
    let mut sys = HostServices::with_media(media);
    // No original firmware in the browser.
    sys.dual_boot = false;
    let vol = sys.media.volume();
    let cluster = u64::from(vol.cluster_size());
    let mib = |clusters: u64| (clusters * cluster) >> 20;
    let free = block_on(vol.free_clusters()).unwrap_or(0);
    let total = (base.len() as u64) >> 20;
    sys.card_info = CardInfo {
        capacity_mib: u32::try_from(total).unwrap_or(0),
        free_mib: u32::try_from(mib(u64::from(free))).unwrap_or(0),
        fs: FsKind::Fat32,
    };
    load_state(&mut sys, state);
    let mut sim = AppSim::with_sys(sys, Box::new(HexFont::subset()));
    sim.run_script("set:library=ready").map_err(|e| format!("{e:#}"))?;
    Ok(sim)
}

/// Restore what [`Web::state`] saved (unknown lines are ignored).
fn load_state(sys: &mut HostServices, state: &str) {
    for line in state.lines() {
        let mut it = line.splitn(3, ' ');
        match (it.next(), it.next(), it.next()) {
            (Some("s"), Some(k), Some(v)) => {
                if let (Ok(k), Ok(v)) = (u16::from_str_radix(k, 16), v.parse::<i16>()) {
                    sys.set_setting(SettingId(k), v);
                }
            }
            (Some("vol"), Some(v), None) => {
                if let Ok(v) = v.parse::<u8>() {
                    let max = u8::try_from(sys.settings.get(id::VOLUME_LIMIT)).unwrap_or(30);
                    sys.volume = v.min(max);
                }
            }
            (Some("resume"), Some(ms), Some(path)) => {
                if let Ok(ms) = ms.parse() {
                    sys.media.set_resume_at(path, ms);
                }
            }
            _ => {}
        }
    }
}

#[wasm_bindgen]
impl Web {
    /// An empty session: add files, then [`Web::boot`].
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Web {
        Web {
            demo: Vec::new(),
            user: Vec::new(),
            library: Vec::new(),
            base: None,
            user_paths: Vec::new(),
            sim: None,
            engine: audio::Engine::new(),
            shown: None,
            rec: None,
            replay: VecDeque::new(),
        }
    }

    /// The demo set: the page's `demo.bin` ([`bundle`]). Returns an error
    /// message, or empty. Without it the card has only the visitor's files.
    pub fn add_demo(&mut self, bundle: &[u8]) -> String {
        match bundle::decode(bundle) {
            Ok(files) => {
                self.demo = files;
                String::new()
            }
            Err(e) => e,
        }
    }

    /// A visitor's file for `/Your files` (takes effect at the next boot).
    pub fn add_file(&mut self, name: String, data: Vec<u8>) {
        self.user.push(card::File { path: name, data });
    }

    /// Forget the visitor's files (takes effect at the next boot).
    pub fn clear_files(&mut self) {
        self.user.clear();
    }

    /// A file of the saved library (name inside the library folder).
    pub fn add_library_file(&mut self, name: String, data: Vec<u8>) {
        self.library.push(card::File { path: name, data });
    }

    /// Identifies the card's content: a saved library is restored only
    /// onto a card with the same fingerprint.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut eat = |b: &[u8]| {
            for x in b {
                h = (h ^ u64::from(*x)).wrapping_mul(0x0100_0000_01b3);
            }
        };
        eat(&CARD_VERSION.to_le_bytes());
        for f in &self.demo {
            eat(f.path.as_bytes());
            eat(&(f.data.len() as u64).to_le_bytes());
        }
        for f in &self.user {
            eat(f.path.as_bytes());
            eat(&(f.data.len() as u64).to_le_bytes());
        }
        format!("{h:016x}")
    }

    /// Build the card and start (again) from `state` ([`Web::state`]'s
    /// text, or empty for defaults). Returns an error message, or empty.
    pub fn boot(&mut self, state: &str) -> String {
        let (base, paths) = match card::build(&self.demo, &self.user, &self.library) {
            Ok(v) => v,
            Err(e) => return e,
        };
        // Restored once: a rebuilt card gets a fresh library.
        self.library.clear();
        self.user_paths = paths;
        self.base = Some(Arc::clone(&base));
        self.engine = audio::Engine::new();
        self.shown = None;
        self.replay.clear();
        match new_sim(&base, state) {
            Ok(s) => {
                self.sim = Some(s);
                String::new()
            }
            Err(e) => e,
        }
    }

    /// Paths of the visitor's files on the card, one per line.
    #[must_use]
    pub fn user_paths(&self) -> String {
        self.user_paths.join("\n")
    }

    /// Bytes of the card image.
    #[must_use]
    pub fn card_bytes(&self) -> usize {
        self.base.as_ref().map_or(0, |b| b.len())
    }

    /// Let `ms` milliseconds pass, then let the audio follow the player.
    pub fn advance(&mut self, ms: f64) {
        let Some(sim) = self.sim.as_mut() else { return };
        let ms = ms.clamp(0.0, MAX_STEP_MS) as u64;
        if ms > 0 {
            sim.advance(ms);
        }
        let st = &sim.sys.settings;
        self.engine.set_sound(audio::Sound::from_settings(|k| st.get(SettingId(k))));
        // Settings › Sound › Fade on pause (VER-13).
        self.engine.set_fade_on_pause(st.get(SettingId(0x0109)) != 0);
        let now = sim.sys.media.now_audio();
        let media = &mut sim.sys.media;
        self.engine.sync(now, |p| media.read_file(p));
    }

    fn key(&mut self, name: &str, press: bool) -> bool {
        let (Some(sim), Some(k)) = (self.sim.as_mut(), Key::from_name(name)) else { return false };
        if let (Some((_, edges)), Some(key)) = (self.rec.as_mut(), rec::key_index(name)) {
            edges.push(rec::Edge { ms: sim.now.0, press, key });
        }
        if press {
            sim.press(k);
        } else {
            sim.release(k);
        }
        true
    }

    /// A key goes down (`up`, `down`, `prev`, `next`, `ok`, `back`, `menu`,
    /// `vol+`, `vol-`, `power`). False for an unknown key.
    pub fn press(&mut self, key: &str) -> bool {
        self.key(key, true)
    }

    /// A key comes up.
    pub fn release(&mut self, key: &str) -> bool {
        self.key(key, false)
    }

    /// The panel as RGBA (128 × 128), or empty if unchanged since the last
    /// call. Black while the screen is off.
    pub fn frame(&mut self) -> Vec<u8> {
        let Some(sim) = self.sim.as_ref() else { return Vec::new() };
        let on = sim.shell.gate.screen_on;
        if self.shown == Some((sim.frames, on)) {
            return Vec::new();
        }
        self.shown = Some((sim.frames, on));
        let mut out = vec![0u8; sim.fb.len() * 4];
        for (px, o) in sim.fb.iter().zip(out.chunks_exact_mut(4)) {
            let [r, g, b] = if on { orpheus_ui::color::to_rgb888(*px) } else { [0, 0, 0] };
            o.copy_from_slice(&[r, g, b, 255]);
        }
        out
    }

    /// Whether the backlight is on.
    #[must_use]
    pub fn screen_on(&self) -> bool {
        self.sim.as_ref().is_some_and(|s| s.shell.gate.screen_on)
    }

    /// Name of the top screen.
    #[must_use]
    pub fn screen(&self) -> String {
        self.sim.as_ref().map_or(String::new(), |s| s.screen().to_string())
    }

    /// Up to `max_frames` frames of stereo PCM (interleaved) at
    /// [`Web::audio_rate`].
    pub fn pull_audio(&mut self, max_frames: u32) -> Vec<f32> {
        self.engine.pull(max_frames as usize)
    }

    #[must_use]
    pub fn audio_rate(&self) -> u32 {
        self.engine.rate()
    }

    /// Whether a track is being decoded (the page counts dropouts only then).
    #[must_use]
    pub fn audio_active(&self) -> bool {
        self.engine.active()
    }

    /// The page should drop the audio it has buffered (seek, pause, new
    /// track); cleared by reading.
    pub fn take_flush(&mut self) -> bool {
        self.engine.take_flush()
    }

    /// The output gain of the player's volume.
    #[must_use]
    pub fn gain(&self) -> f32 {
        self.sim.as_ref().map_or(0.0, |s| audio::volume_gain(s.sys.volume))
    }

    /// Settings, volume and resume point as text (for IndexedDB).
    #[must_use]
    pub fn state(&self) -> String {
        let Some(sim) = self.sim.as_ref() else { return String::new() };
        let mut s = String::from("orpheus-web 1\n");
        for set in orpheus_apps::settings::all().filter(|s| s.stored()) {
            let v = sim.sys.settings.get(set.id);
            if v != set.default {
                let _ = writeln!(s, "s {:04x} {v}", set.id.0);
            }
        }
        let _ = writeln!(s, "vol {}", sim.sys.volume);
        if let Some((path, ms)) = sim.sys.media.resume_point() {
            let _ = writeln!(s, "resume {ms} {path}");
        }
        s
    }

    /// The library's file names (in its folder), one per line.
    pub fn library_names(&mut self) -> String {
        let Some(sim) = self.sim.as_mut() else { return String::new() };
        let vol = sim.sys.media.volume();
        let mut names = Vec::new();
        if let Ok(mut cur) = block_on(vol.open_dir(card::LIBRARY)) {
            let mut e = DirEntry::new();
            while let Ok(true) = block_on(vol.read_dir(&mut cur, &mut e)) {
                if !e.is_dir() {
                    names.push(e.name().to_string());
                }
            }
        }
        names.join("\n")
    }

    /// A file of the library folder.
    pub fn library_file(&mut self, name: &str) -> Vec<u8> {
        let Some(sim) = self.sim.as_mut() else { return Vec::new() };
        sim.sys.media.read_file(&format!("{}/{name}", card::LIBRARY))
    }

    /// The player's clock (it runs on from here).
    #[allow(clippy::too_many_arguments)]
    pub fn set_clock(
        &mut self,
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
        weekday: u8,
    ) {
        let Some(sim) = self.sim.as_mut() else { return };
        sim.sys.clock = orpheus_apps::services::DateTime { year, month, day, hour, minute, second, weekday };
        sim.sys.clock_run = true;
        // The status bar shows it at once.
        sim.shell.invalidate();
        sim.draw();
    }

    /// Start recording key edges.
    pub fn rec_start(&mut self) {
        let t = self.sim.as_ref().map_or(0, |s| s.now.0);
        self.rec = Some((t, Vec::new()));
    }

    /// Stop recording: the sequence as a `ui_keys` corpus input.
    pub fn rec_stop(&mut self) -> Vec<u8> {
        self.rec.take().map_or_else(Vec::new, |(t, e)| rec::encode(&e, t))
    }

    /// Operations recorded so far.
    #[must_use]
    pub fn rec_ops(&self) -> u32 {
        self.rec.as_ref().map_or(0, |(t, e)| rec::ops(&rec::encode(e, *t)) as u32)
    }

    /// Load a corpus input: restart on the same card with default
    /// settings (as the fuzz target starts) and queue its operations.
    /// Returns their number.
    pub fn replay_load(&mut self, data: &[u8]) -> u32 {
        let Some(base) = self.base.clone() else { return 0 };
        let cmds = rec::decode(data);
        self.engine = audio::Engine::new();
        self.shown = None;
        match new_sim(&base, "") {
            Ok(s) => self.sim = Some(s),
            Err(_) => return 0,
        }
        self.replay = cmds.into();
        self.replay.len() as u32
    }

    /// Run the next replayed operation; returns it, or empty when done.
    pub fn replay_step(&mut self) -> String {
        let Some(sim) = self.sim.as_mut() else { return String::new() };
        if sim.now.0 > rec::MAX_MS {
            self.replay.clear();
        }
        let Some(cmd) = self.replay.pop_front() else { return String::new() };
        match sim.run_script(&cmd) {
            Ok(_) => cmd,
            Err(e) => format!("{cmd} (refused: {e:#})"),
        }
    }

    /// Run a simulator script (tests, developer panel). Returns an error
    /// message, or empty.
    pub fn run_script(&mut self, script: &str) -> String {
        let Some(sim) = self.sim.as_mut() else { return "not booted".into() };
        match sim.run_script(script) {
            Ok(_) => String::new(),
            Err(e) => format!("{e:#}"),
        }
    }

    /// State for the developer panel, as JSON.
    #[must_use]
    pub fn info(&self) -> String {
        let Some(sim) = self.sim.as_ref() else { return "{}".into() };
        let (desc, arena, ms) = self.engine.describe();
        let track = sim.sys.media.now_audio().map(|n| n.path).unwrap_or_default();
        format!(
            "{{\"screen\":{},\"now_ms\":{},\"frames\":{},\"dirty_pixels\":{},\"card_bytes\":{},\"track\":{},\"codec\":{},\"arena\":{},\"audio_ms\":{},\"audio_rate\":{},\"tracks_started\":{},\"audio_error\":{},\"volume\":{}}}",
            json(sim.screen()),
            sim.now.0,
            sim.frames,
            sim.dirty_pixels,
            self.card_bytes(),
            json(&track),
            json(&desc),
            arena,
            ms,
            self.engine.rate(),
            self.engine.started,
            json(self.engine.last_error.unwrap_or("")),
            sim.sys.volume,
        )
    }
}

impl Web {
    /// The simulator (host tests).
    #[must_use]
    pub fn sim(&self) -> Option<&AppSim> {
        self.sim.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orpheus_apps::services::LibraryState;

    /// A session with the demo set, as the page makes it.
    fn with_demo() -> Web {
        let mut w = Web::new();
        assert_eq!(w.add_demo(&bundle::encode(&demo::committed())), "");
        w
    }

    fn booted() -> Web {
        let mut w = with_demo();
        assert_eq!(w.boot(""), "");
        w
    }

    fn golden(name: &str) -> Vec<u8> {
        let p =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../golden").join(format!("{name}.png"));
        let mut r =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&p).unwrap())).read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height, info.color_type), (128, 128, png::ColorType::Rgb));
        buf.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect()
    }

    /// The screens the page's self-test compares (`site/try/app.js`,
    /// `SELFTEST`) look as their golden images in the browser build too.
    #[test]
    fn selftest_screens_match_their_goldens() {
        for (name, script) in [("app_home_dark_latin", ""), ("app_settings_dark_latin", "prev ok wait:500")] {
            let mut w = booted();
            assert_eq!(w.run_script(script), "");
            let f = w.frame();
            let g = golden(name);
            let diff = f.chunks(4).zip(g.chunks(4)).filter(|(a, b)| a != b).count();
            assert_eq!(diff, 0, "{name}: {diff} pixels differ");
        }
    }

    #[test]
    fn boots_at_home_with_the_library_built() {
        let mut w = booted();
        assert_eq!(w.screen(), "Home");
        assert_eq!(w.sim().unwrap().sys.media.library, LibraryState::Ready);
        assert_eq!(w.frame().len(), 128 * 128 * 4);
        assert!(w.frame().is_empty(), "unchanged frame");
        let names = w.library_names();
        assert!(!names.is_empty(), "library files on the card");
    }

    #[test]
    fn plays_a_demo_track_through_the_codecs() {
        let mut w = booted();
        // Music › Folders › Bach › Goldberg Variations › first track.
        let path = demo::ARIA;
        w.sim.as_mut().unwrap().sys.media.set_resume_at(path, 2000);
        assert_eq!(w.run_script("ok wait:100 ok wait:200"), "");
        w.advance(10.0);
        let now = w.sim().unwrap().sys.media.now_audio().expect("playing");
        assert_eq!(now.path, path);
        assert!(now.playing);
        assert!(now.duration_ms.abs_diff(30_000) < 150, "{}", now.duration_ms);
        let pcm = w.pull_audio(4410);
        assert_eq!(pcm.len(), 8820);
        assert_eq!(w.audio_rate(), 44_100);
        assert!(w.info().contains(&format!("\"track\":\"{}\"", demo::ARIA)));
    }

    /// The self-test's key path from Home to the first demo track
    /// (`site/try/app.js`).
    #[test]
    fn selftest_key_path_plays_the_aria() {
        let mut w = booted();
        let mut seen = Vec::new();
        // Files › Music › Bach › Goldberg Variations › 01 Aria.mp3.
        for k in ["next", "ok", "down", "down", "ok", "ok", "ok", "ok"] {
            assert_eq!(w.run_script(&format!("{k} wait:250")), "");
            seen.push(w.screen());
        }
        w.advance(10.0);
        let now = w.sim().unwrap().sys.media.now_audio();
        assert_eq!(now.map(|n| n.path).as_deref(), Some(demo::ARIA), "{seen:?}");
    }

    #[test]
    fn state_round_trips() {
        let mut w = booted();
        w.sim.as_mut().unwrap().sys.set_setting(id::THEME, 1);
        w.sim.as_mut().unwrap().sys.volume = 17;
        w.sim.as_mut().unwrap().sys.media.set_resume_at("/Music/Satie/Gymnopédie No. 1.wav", 1500);
        let st = w.state();
        assert!(st.contains("vol 17"), "{st}");
        assert!(st.contains("resume 1500 /Music/Satie/Gymnopédie No. 1.wav"), "{st}");
        let mut w2 = with_demo();
        assert_eq!(w2.boot(&st), "");
        assert_eq!(w2.state(), st);
    }

    #[test]
    fn saved_library_is_reused() {
        let mut w = booted();
        let names: Vec<String> = w.library_names().lines().map(str::to_string).collect();
        let mut w2 = with_demo();
        for n in &names {
            let data = w.library_file(n);
            assert!(!data.is_empty());
            w2.add_library_file(n.clone(), data);
        }
        assert_eq!(w2.fingerprint(), w.fingerprint());
        assert_eq!(w2.boot(""), "");
        assert_eq!(w2.sim().unwrap().sys.media.library, LibraryState::Ready);
        assert_eq!(w2.run_script("ok wait:200"), "");
    }

    #[test]
    fn visitors_files_appear_and_change_the_fingerprint() {
        let mut w = with_demo();
        let f0 = w.fingerprint();
        assert_ne!(f0, Web::new().fingerprint(), "the demo set is part of it");
        let wav = demo::committed().into_iter().find(|f| f.path.ends_with(".wav")).unwrap().data;
        w.add_file("my song.wav".into(), wav);
        assert_ne!(w.fingerprint(), f0);
        assert_eq!(w.boot(""), "");
        assert_eq!(w.user_paths(), "/Your files/my song.wav");
        let d = w.sim.as_mut().unwrap().sys.media.read_file("/Your files/my song.wav");
        assert_eq!(&d[..4], b"RIFF");
    }

    #[test]
    fn records_and_replays_keys() {
        let mut w = booted();
        w.rec_start();
        w.press("next");
        w.advance(80.0);
        w.release("next");
        w.advance(500.0);
        w.press("ok");
        w.advance(60.0);
        w.release("ok");
        assert_eq!(w.rec_ops(), 3);
        let data = w.rec_stop();
        assert_eq!(rec::decode(&data), ["next", "wait:445", "ok"]);
        let screen = w.screen();
        assert_eq!(w.replay_load(&data), 3);
        assert_eq!(w.screen(), "Home");
        while !w.replay_step().is_empty() {}
        assert_eq!(w.screen(), screen);
    }

    #[test]
    fn volume_gain_follows_the_keys() {
        let mut w = booted();
        let g = w.gain();
        w.press("vol+");
        w.advance(60.0);
        w.release("vol+");
        assert!(w.gain() > g);
    }
}
