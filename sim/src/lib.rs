//! orpheus-sim: the desktop simulator (07 §3.2).
//!
//! A headless 128 × 128 RGB565 frame buffer driven by the same `orpheus-ui`
//! code as the firmware, simulated time (so every run is deterministic), key
//! scripts, PNG export, and (feature `window`) a 4× window. Two hosts:
//! [`DemoSim`] runs the toolkit's widget demo (UI-1 golden images),
//! [`AppSim`] runs the real `orpheus-apps` screens over scriptable fake
//! services (UI-2 onwards).
//!
//! Script language (whitespace- or comma-separated commands):
//!
//! | Command | Meaning |
//! |---|---|
//! | `ok`, `down`, `vol+` … | tap a key (press 60 ms, then 40 ms idle) |
//! | `down*5` | tap five times |
//! | `hold:menu`, `hold:power:1600` | press, hold 700 ms (or the given ms), release |
//! | `press:next`, `release:next` | key edges, for shots taken while held |
//! | `wait:300` | let 300 ms pass |
//! | `shot:name` | capture the frame buffer as `name` |
//! | `expect-screen:Name` | fail unless the top screen is `Name` |
//! | `theme:dark`, `accent:green`, `lang:cyrillic` | appearance and language |
//! | `set:battery=8`, `set:usb=host` … | simulated system state (per host) |
//! | `goto:Library` | push a screen directly |
//! | `set:drivehost=1`, `set:drivewrite=8` | a computer uses the USB Drive (reads it all; writes 8 blocks) |
//! | `expect-typed:287082↵` | fail unless the fake computer on the USB keyboard received this (`↵` = Enter) |
//! | `set:games=off` | the Games app off Home (M14; on by default, as on the player) |
//! | `set:gameseed=7`, `set:tiles=1.2.3…`, `set:blocksrows=12`, `set:solnear=4`, `set:gamesio=on` | Games: next seed, a 4096 board, a Blocks stack, an almost won Solitaire game, the firmware's deferred card queue (`games.rs`) |

pub mod apps;
pub mod auth;
pub mod books;
pub mod bt;
pub mod card;
pub mod demo;
pub mod drive;
pub mod fallback;
pub mod games;
pub mod library;
pub mod media;
pub mod packs;
pub mod pictures;
pub mod radio;
pub mod recorder;
pub mod steps;
pub mod system;
pub mod update;
#[cfg(feature = "window")]
pub mod window;

use anyhow::{Context, Result, bail};
use orpheus_ui::event::Event;
use orpheus_ui::font::{GlyphCache, GlyphSource};
use orpheus_ui::gfx::{DirtySet, FB_PIXELS, Fallback, Gfx};
use orpheus_ui::input::{KeyGate, KeyMapper};
use orpheus_ui::nav::{Arg, Nav, ScreenId, ScreenSet};
use orpheus_ui::shell::{Requests, Shell, System};
use orpheus_ui::theme::{Accent, Palette, Theme};
use orpheus_ui::{Instant, Key};

use crate::fallback::{FonFile, HexFont};

/// One captured frame.
pub struct Shot {
    pub name: String,
    pub fb: Vec<u16>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Shot {
    /// The frame as PNG bytes (RGB, 8 bit; deterministic for a given frame).
    pub fn png(&self) -> Result<Vec<u8>> {
        png_bytes(&self.fb)
    }
}

/// RGB565 frame to PNG bytes through the host image helpers.
#[cfg(not(target_arch = "wasm32"))]
pub fn png_bytes(fb: &[u16]) -> Result<Vec<u8>> {
    let bytes: Vec<u8> = fb.iter().flat_map(|p| p.to_le_bytes()).collect();
    orpheus_host::image::rgb565_png_bytes(&bytes, 128, 128)
}

/// What differs between simulated systems.
pub trait Host: System + Default + Sized {
    type Screens: ScreenSet<Self>;

    /// Palette for this frame (`theme`/`accent` are the script's choice).
    fn palette(&self, theme: Theme, accent: Accent) -> Palette;
    /// `theme:`/`accent:` commands (the apps store them as settings).
    fn appearance(&mut self, _theme: Theme, _accent: Accent) -> Vec<Event> {
        Vec::new()
    }
    /// `lang:` command; `Ok(true)` restarts the shell at Home.
    fn set_lang(&mut self, lang: &str) -> Result<bool>;
    /// `set:key=value`; returns service events to deliver.
    fn set(&mut self, kv: &str) -> Result<Vec<Event>>;
    fn screen_by_name(name: &str) -> Option<ScreenId>;
    /// Root screen at start.
    fn root() -> (ScreenId, Arg) {
        (Self::Screens::HOME, Arg(0))
    }
    /// After each step: apply what screens asked of the services that
    /// affects the key gate (lock keys, screen off after the lock notice).
    fn after_step(&mut self, _gate: &mut KeyGate) {}
    /// Events the host produced on its own (settings changed …).
    fn take_events(&mut self) -> Vec<Event> {
        Vec::new()
    }
    /// Simulated time passes (playback position, pending loads).
    fn advance(&mut self, _ms: u64) {}
    /// What the fake computer on the USB keyboard has received.
    fn typed(&self) -> &str {
        ""
    }
}

/// The simulator: frame buffer, input mapper, shell and fake services.
pub struct Sim<H: Host> {
    pub fb: Vec<u16>,
    /// Dirty rectangles since the last `take_dirty`.
    pub dirty: DirtySet,
    cache: Box<GlyphCache>,
    font: Box<dyn GlyphSource>,
    pub mapper: KeyMapper,
    pub shell: Shell<H::Screens, H>,
    pub sys: H,
    pub theme: Theme,
    pub accent: Accent,
    pub now: Instant,
    /// Requests the shell made (screen off, power menu …), in order.
    pub requests: Vec<Requests>,
    /// Frames drawn and pixels covered by dirty rectangles (cost metric).
    pub frames: u32,
    pub dirty_pixels: u64,
    /// The screen was off before a lock notice lit it.
    notice_lit: bool,
}

/// The widget demo of UI-1.
pub type DemoSim = Sim<system::SimSystem>;
/// The apps of `orpheus-apps` over fake services.
pub type AppSim = Sim<apps::HostServices>;

impl<H: Host> Default for Sim<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H: Host> Sim<H> {
    /// A simulator with the committed Unifont subset as fallback font.
    #[must_use]
    pub fn new() -> Self {
        Self::with_font(Box::new(HexFont::subset()))
    }

    /// Use the font file named by `ORPHEUS_FALLBACK_FON` if set (development
    /// only, e.g. the vendor font in a NOR dump).
    #[must_use]
    pub fn new_dev_font() -> Self {
        match FonFile::from_env() {
            Some(f) => Self::with_font(Box::new(f)),
            None => Self::new(),
        }
    }

    #[must_use]
    pub fn with_font(font: Box<dyn GlyphSource>) -> Self {
        Self::with_sys(H::default(), font)
    }

    /// A simulator over services built by the caller (the browser build
    /// brings its own card).
    #[must_use]
    pub fn with_sys(mut sys: H, font: Box<dyn GlyphSource>) -> Self {
        let (root, arg) = H::root();
        let shell = Shell::with_root(&mut sys, root, arg, Instant(0));
        let mut s = Sim {
            fb: vec![0; FB_PIXELS],
            dirty: DirtySet::new(),
            cache: Box::new(GlyphCache::new()),
            font,
            mapper: KeyMapper::new(),
            shell,
            sys,
            theme: Theme::Dark,
            accent: Accent::Blue,
            now: Instant(0),
            requests: Vec::new(),
            frames: 0,
            dirty_pixels: 0,
            notice_lit: false,
        };
        s.draw();
        s
    }

    /// Run the shell's draw pass.
    pub fn draw(&mut self) {
        let pal = self.sys.palette(self.theme, self.accent);
        let mut dirty = DirtySet::new();
        {
            let fb = Fallback { cache: &mut self.cache, src: self.font.as_mut() };
            let mut g = Gfx::new(&mut self.fb, &mut dirty, pal, Some(fb));
            self.shell.draw(&mut self.sys, &mut g);
        }
        if !dirty.is_empty() {
            self.frames += 1;
            self.dirty_pixels +=
                dirty.rects().iter().map(|r| u64::try_from(r.area()).unwrap_or(0)).sum::<u64>();
            for r in dirty.rects() {
                self.dirty.add(*r);
            }
        }
    }

    pub fn take_dirty(&mut self) -> DirtySet {
        self.dirty.take()
    }

    /// What the firmware does with the shell's requests (05 §3, §6.3).
    fn handle_requests(&mut self, r: Requests) {
        if r == Requests::default() {
            return;
        }
        let mut redraw = false;
        let gate = &mut self.shell.gate;
        if r.screen_off {
            if gate.locked || !gate.screen_on {
                // Power while off or locked: wake and unlock.
                gate.locked = false;
                gate.screen_on = true;
                self.notice_lit = false;
                redraw = true;
            } else {
                gate.screen_on = false;
                gate.locked = true;
            }
        }
        if r.wake {
            gate.screen_on = true;
            redraw = true;
        }
        if r.lock_notice && !gate.screen_on {
            gate.screen_on = true;
            self.notice_lit = true;
            redraw = true;
        }
        if redraw {
            self.shell.invalidate();
        }
        self.requests.push(r);
    }

    /// Deliver events from the host to the shell.
    pub fn deliver(&mut self, evs: Vec<Event>) {
        for ev in evs {
            let r = self.shell.event(&mut self.sys, ev, self.now);
            self.handle_requests(r);
        }
    }

    fn settle(&mut self) {
        let was_locked = self.shell.gate.locked;
        self.sys.after_step(&mut self.shell.gate);
        if self.notice_lit && !self.shell.gate.screen_on {
            self.notice_lit = false;
        }
        if was_locked != self.shell.gate.locked {
            self.shell.invalidate();
        }
        let evs = self.sys.take_events();
        self.deliver(evs);
    }

    /// Deliver queued key phases.
    fn pump(&mut self) {
        while let Some((k, p)) = self.mapper.poll(self.now) {
            let r = self.shell.key(&mut self.sys, k, p, self.now);
            self.handle_requests(r);
            self.settle();
        }
    }

    /// Let `ms` milliseconds pass: key timers, ticks (every 33 ms while
    /// wanted) and a draw after every step.
    pub fn advance(&mut self, ms: u64) {
        let end = self.now.0 + ms;
        while self.now.0 < end {
            let mut next = end.min(self.now.0 + u64::from(orpheus_ui::shell::TICK_MS));
            if let Some(d) = self.mapper.next_deadline() {
                next = next.min(d.0.max(self.now.0 + 1));
            }
            let dt = next - self.now.0;
            self.now = Instant(next);
            self.sys.advance(dt);
            self.settle();
            self.pump();
            if self.shell.wants_ticks(&self.sys) {
                let r = self.shell.tick(&mut self.sys, self.now);
                self.handle_requests(r);
                self.settle();
            }
            self.draw();
        }
    }

    pub fn press(&mut self, k: Key) {
        self.mapper.press(k, self.now);
        self.pump();
        self.draw();
    }

    pub fn release(&mut self, k: Key) {
        self.mapper.release(k, self.now);
        self.pump();
        self.draw();
    }

    /// Tap: press, 60 ms, release, 40 ms.
    pub fn tap(&mut self, k: Key) {
        self.press(k);
        self.advance(60);
        self.release(k);
        self.advance(40);
    }

    /// Press, hold for `ms`, release.
    pub fn hold(&mut self, k: Key, ms: u64) {
        self.press(k);
        self.advance(ms);
        self.release(k);
        self.advance(40);
    }

    /// Change theme or accent: everything is redrawn.
    pub fn set_theme(&mut self, theme: Theme, accent: Accent) {
        self.theme = theme;
        self.accent = accent;
        let evs = self.sys.appearance(theme, accent);
        self.deliver(evs);
        self.shell.invalidate();
        self.draw();
    }

    #[must_use]
    pub fn shot(&self, name: &str) -> Shot {
        Shot { name: name.to_string(), fb: self.fb.clone() }
    }

    /// Run a script; returns the shots it took.
    pub fn run_script(&mut self, script: &str) -> Result<Vec<Shot>> {
        let mut shots = Vec::new();
        for cmd in script.split(|c: char| c.is_whitespace() || c == ',').filter(|s| !s.is_empty()) {
            self.command(cmd, &mut shots).with_context(|| format!("script command `{cmd}`"))?;
        }
        Ok(shots)
    }

    fn command(&mut self, cmd: &str, shots: &mut Vec<Shot>) -> Result<()> {
        let key = |s: &str| Key::from_name(s).with_context(|| format!("unknown key `{s}`"));
        if let Some((op, arg)) = cmd.split_once(':') {
            match op {
                "hold" => {
                    let (k, ms) = match arg.split_once(':') {
                        Some((k, ms)) => (k, ms.parse()?),
                        None => (arg, if arg == "power" { 1600 } else { 700 }),
                    };
                    self.hold(key(k)?, ms);
                }
                "press" => self.press(key(arg)?),
                "release" => self.release(key(arg)?),
                "wait" => self.advance(arg.parse()?),
                "shot" => {
                    self.draw();
                    shots.push(self.shot(arg));
                }
                "expect-screen" => {
                    let n = self.shell.top_name();
                    if n != arg {
                        bail!("expected screen {arg}, top is {n}");
                    }
                }
                "expect-typed" => {
                    // `↵` stands for Enter (scripts split on whitespace).
                    let want = arg.replace('↵', "\n");
                    let got = self.sys.typed();
                    if got != want {
                        bail!("expected typed {want:?}, the computer got {got:?}");
                    }
                }
                "theme" => {
                    let t = match arg {
                        "dark" => Theme::Dark,
                        "light" => Theme::Light,
                        "amber" => Theme::Amber,
                        _ => bail!("theme: dark, light, amber"),
                    };
                    self.set_theme(t, self.accent);
                }
                "accent" => {
                    let a = match arg {
                        "blue" => Accent::Blue,
                        "green" => Accent::Green,
                        "pink" => Accent::Pink,
                        "red" => Accent::Red,
                        "grey" => Accent::Grey,
                        _ => bail!("accent: blue, green, pink, red, grey"),
                    };
                    self.set_theme(self.theme, a);
                }
                "lang" => {
                    if self.sys.set_lang(arg)? {
                        self.shell = Shell::new(&mut self.sys, self.now);
                    }
                    self.shell.invalidate();
                    let evs = self.sys.take_events();
                    self.deliver(evs);
                    self.draw();
                }
                "set" => {
                    let evs = self.sys.set(arg)?;
                    self.deliver(evs);
                    self.settle();
                    self.shell.invalidate();
                    self.draw();
                }
                "goto" => {
                    let id = H::screen_by_name(arg).with_context(|| format!("unknown screen `{arg}`"))?;
                    self.shell.navigate(&mut self.sys, Nav::Push(id, Arg(0)), self.now);
                    self.draw();
                }
                _ => bail!("unknown command"),
            }
            return Ok(());
        }
        let (name, n) = match cmd.split_once('*') {
            Some((k, n)) => (k, n.parse::<u32>()?),
            None => (cmd, 1),
        };
        let k = key(name)?;
        for _ in 0..n {
            self.tap(k);
        }
        Ok(())
    }

    /// Name of the top screen.
    #[must_use]
    pub fn screen(&self) -> &'static str {
        self.shell.top_name()
    }

    /// Whether the top screen is the host's Home.
    #[must_use]
    pub fn at_home(&self) -> bool {
        self.shell.top_id() == <H::Screens as ScreenSet<H>>::HOME
    }
}
