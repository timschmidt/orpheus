//! Host implementation of the services the toolkit and the demo screens
//! use. Everything is plain state that scripts can set (`set:battery=8`).

use anyhow::{Result, bail};
use orpheus_ui::event::{BtState, Source};
use orpheus_ui::shell::System;
use orpheus_ui::widgets::bars::{MiniPlayer, Status};

/// Sample-text language of the demo screens.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Lang {
    #[default]
    Latin,
    Cyrillic,
    Cjk,
}

impl Lang {
    pub const ALL: [Lang; 3] = [Lang::Latin, Lang::Cyrillic, Lang::Cjk];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Lang::Latin => "latin",
            Lang::Cyrillic => "cyrillic",
            Lang::Cjk => "cjk",
        }
    }

    pub fn parse(s: &str) -> Result<Lang> {
        Ok(match s {
            "latin" | "en" => Lang::Latin,
            "cyrillic" | "ru" => Lang::Cyrillic,
            "cjk" | "ja" => Lang::Cjk,
            _ => bail!("unknown language `{s}` (latin, cyrillic, cjk)"),
        })
    }

    /// Pick the string for this language.
    #[must_use]
    pub fn pick(self, t: [&'static str; 3]) -> &'static str {
        match self {
            Lang::Latin => t[0],
            Lang::Cyrillic => t[1],
            Lang::Cjk => t[2],
        }
    }
}

/// Something playing (drives the status bar and the mini-player).
#[derive(Clone, Copy, Debug)]
pub struct NowPlaying {
    pub title: &'static str,
    pub playing: bool,
    pub progress: u16,
}

pub struct SimSystem {
    pub status: Status,
    pub volume: u8,
    pub volume_max: u8,
    pub now_playing: Option<NowPlaying>,
    pub lang: Lang,
    // Demo settings the screens and menus edit.
    pub shuffle: bool,
    pub gapless: bool,
    pub repeat: u8,
    pub home_items: [bool; 3],
    /// Things the demo did, for test assertions.
    pub log: Vec<String>,
}

impl Default for SimSystem {
    fn default() -> Self {
        SimSystem {
            status: Status {
                clock: Some((12, 41)),
                battery: 72,
                charging: false,
                bt: BtState::Off,
                source: Source::Idle,
                locked: false,
                sleep_timer: false,
                alarm: false,
                activity: false,
            },
            volume: 12,
            volume_max: 30,
            now_playing: None,
            lang: Lang::Latin,
            shuffle: false,
            gapless: true,
            repeat: 0,
            home_items: [true, true, false],
            log: Vec::new(),
        }
    }
}

impl SimSystem {
    /// Start "playing" a title.
    pub fn play(&mut self, title: &'static str) {
        self.now_playing = Some(NowPlaying { title, playing: true, progress: 460 });
        self.status.source = Source::Playing;
    }

    /// `set:key=value` from scripts.
    pub fn set(&mut self, kv: &str) -> Result<()> {
        let Some((k, v)) = kv.split_once('=') else { bail!("set:{kv}: expected key=value") };
        let on = matches!(v, "1" | "on" | "true" | "yes");
        match k {
            "battery" => self.status.battery = v.parse()?,
            "charging" => self.status.charging = on,
            "locked" => self.status.locked = on,
            "sleep" => self.status.sleep_timer = on,
            "alarm" => self.status.alarm = on,
            "activity" => self.status.activity = on,
            "clock" => {
                self.status.clock = match v.split_once(':') {
                    Some((h, m)) => Some((h.parse()?, m.parse()?)),
                    None => None,
                }
            }
            "bt" => {
                self.status.bt = match v {
                    "off" => BtState::Off,
                    "on" => BtState::On,
                    "connected" => BtState::Connected,
                    "streaming" => BtState::Streaming,
                    _ => bail!("bt: off, on, connected, streaming"),
                }
            }
            "source" => {
                self.status.source = match v {
                    "idle" => Source::Idle,
                    "playing" => Source::Playing,
                    "paused" => Source::Paused,
                    "recording" => Source::Recording,
                    "radio" => Source::Radio,
                    _ => bail!("source: idle, playing, paused, recording, radio"),
                }
            }
            "playing" => {
                if on {
                    let t = self.lang.pick(crate::demo::MINI_TITLE);
                    self.play(t);
                } else {
                    self.now_playing = None;
                    self.status.source = Source::Idle;
                }
            }
            "volume" => self.volume = v.parse()?,
            _ => bail!("set: unknown key `{k}`"),
        }
        Ok(())
    }
}

impl System for SimSystem {
    fn status(&self) -> Status {
        self.status
    }

    fn volume_step(&mut self, delta: i8) -> (u8, u8) {
        let v = i16::from(self.volume) + i16::from(delta);
        self.volume = u8::try_from(v.clamp(0, i16::from(self.volume_max))).unwrap_or(0);
        (self.volume, self.volume_max)
    }

    fn mini_player(&self) -> Option<MiniPlayer<'_>> {
        self.now_playing.map(|n| MiniPlayer { title: n.title, playing: n.playing, progress: n.progress })
    }
}

impl crate::Host for SimSystem {
    type Screens = crate::demo::Demo;

    fn palette(
        &self,
        theme: orpheus_ui::theme::Theme,
        accent: orpheus_ui::theme::Accent,
    ) -> orpheus_ui::theme::Palette {
        theme.palette(accent)
    }

    fn set_lang(&mut self, lang: &str) -> Result<bool> {
        self.lang = Lang::parse(lang)?;
        if let Some(np) = self.now_playing.as_mut() {
            np.title = self.lang.pick(crate::demo::MINI_TITLE);
        }
        Ok(true)
    }

    fn set(&mut self, kv: &str) -> Result<Vec<orpheus_ui::event::Event>> {
        SimSystem::set(self, kv)?;
        Ok(Vec::new())
    }

    fn screen_by_name(name: &str) -> Option<orpheus_ui::nav::ScreenId> {
        crate::demo::screen_by_name(name)
    }
}
