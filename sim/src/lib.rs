//! yoton-sim: the desktop simulator (07 §3.2).
//!
//! A headless 128 × 128 RGB565 frame buffer driven by the same `yoton-ui`
//! code as the firmware, simulated time (so every run is deterministic), key
//! scripts, PNG export, and (feature `window`) a 4× window.
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
//! | `theme:dark`, `accent:green`, `lang:cyrillic` | appearance and sample text |
//! | `set:battery=8`, `set:bt=streaming` … | simulated system state |
//! | `goto:Library` | push a screen directly |

pub mod demo;
pub mod fallback;
pub mod system;
#[cfg(feature = "window")]
pub mod window;

use anyhow::{Context, Result, bail};
use yoton_ui::font::{GlyphCache, GlyphSource};
use yoton_ui::gfx::{DirtySet, FB_PIXELS, Fallback, Gfx};
use yoton_ui::input::KeyMapper;
use yoton_ui::nav::{Arg, Nav, ScreenSet};
use yoton_ui::shell::{Requests, Shell};
use yoton_ui::theme::{Accent, Theme};
use yoton_ui::{Instant, Key};

use crate::demo::Demo;
use crate::fallback::{FonFile, HexFont};
use crate::system::{Lang, SimSystem};

/// One captured frame.
pub struct Shot {
    pub name: String,
    pub fb: Vec<u16>,
}

impl Shot {
    /// The frame as PNG bytes (RGB, 8 bit; deterministic for a given frame).
    pub fn png(&self) -> Result<Vec<u8>> {
        png_bytes(&self.fb)
    }
}

/// RGB565 frame to PNG bytes through the host image helpers.
pub fn png_bytes(fb: &[u16]) -> Result<Vec<u8>> {
    let bytes: Vec<u8> = fb.iter().flat_map(|p| p.to_le_bytes()).collect();
    yoton_host::image::rgb565_png_bytes(&bytes, 128, 128)
}

/// The simulator: frame buffer, input mapper, shell and fake services.
pub struct Sim {
    pub fb: Vec<u16>,
    /// Dirty rectangles since the last `take_dirty`.
    pub dirty: DirtySet,
    cache: Box<GlyphCache>,
    font: Box<dyn GlyphSource>,
    pub mapper: KeyMapper,
    pub shell: Shell<Demo, SimSystem>,
    pub sys: SimSystem,
    pub theme: Theme,
    pub accent: Accent,
    pub now: Instant,
    /// Requests the shell made (screen off, power menu …), in order.
    pub requests: Vec<Requests>,
    /// Frames drawn and pixels covered by dirty rectangles (cost metric).
    pub frames: u32,
    pub dirty_pixels: u64,
}

impl Default for Sim {
    fn default() -> Self {
        Self::new()
    }
}

impl Sim {
    /// A simulator with the committed Unifont subset as fallback font (or
    /// the file named by `YOTON_FALLBACK_FON` when `dev_font` is used).
    #[must_use]
    pub fn new() -> Sim {
        Self::with_font(Box::new(HexFont::subset()))
    }

    /// Use the vendor font from the environment if set (development only).
    #[must_use]
    pub fn new_dev_font() -> Sim {
        match FonFile::from_env() {
            Some(f) => Self::with_font(Box::new(f)),
            None => Self::new(),
        }
    }

    #[must_use]
    pub fn with_font(font: Box<dyn GlyphSource>) -> Sim {
        let mut sys = SimSystem::default();
        let shell = Shell::new(&mut sys, Instant(0));
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
        };
        s.draw();
        s
    }

    /// Run the shell's draw pass.
    pub fn draw(&mut self) {
        let pal = self.theme.palette(self.accent);
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

    fn handle_requests(&mut self, r: Requests) {
        if r == Requests::default() {
            return;
        }
        if r.screen_off {
            self.shell.gate.screen_on = false;
            self.shell.gate.locked = true;
            self.sys.status.locked = true;
        }
        if r.wake {
            self.shell.gate.screen_on = true;
            self.shell.invalidate();
        }
        self.requests.push(r);
    }

    /// Deliver queued key phases.
    fn pump(&mut self) {
        while let Some((k, p)) = self.mapper.poll(self.now) {
            let r = self.shell.key(&mut self.sys, k, p, self.now);
            self.handle_requests(r);
        }
    }

    /// Let `ms` milliseconds pass: key timers, ticks (every 33 ms while
    /// wanted) and a draw after every step.
    pub fn advance(&mut self, ms: u64) {
        let end = self.now.0 + ms;
        while self.now.0 < end {
            let mut next = end.min(self.now.0 + u64::from(yoton_ui::shell::TICK_MS));
            if let Some(d) = self.mapper.next_deadline() {
                next = next.min(d.0.max(self.now.0 + 1));
            }
            self.now = Instant(next);
            self.pump();
            if self.shell.wants_ticks(&self.sys) {
                let r = self.shell.tick(&mut self.sys, self.now);
                self.handle_requests(r);
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
        self.shell.invalidate();
        self.draw();
    }

    /// Change the sample-text language: back to a fresh Home.
    pub fn set_lang(&mut self, lang: Lang) {
        self.sys.lang = lang;
        if let Some(np) = self.sys.now_playing.as_mut() {
            np.title = lang.pick(demo::MINI_TITLE);
        }
        self.shell = Shell::new(&mut self.sys, self.now);
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
                "lang" => self.set_lang(Lang::parse(arg)?),
                "set" => {
                    self.sys.set(arg)?;
                    self.shell.invalidate();
                    self.draw();
                }
                "goto" => {
                    let id = demo::screen_by_name(arg).with_context(|| format!("unknown screen `{arg}`"))?;
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

    /// Whether the top screen is `Demo::HOME`.
    #[must_use]
    pub fn at_home(&self) -> bool {
        self.shell.top_id() == <Demo as ScreenSet<SimSystem>>::HOME
    }
}
