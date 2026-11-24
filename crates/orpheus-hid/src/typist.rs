//! Text to keystrokes: one press report and one all-released report per
//! character, each followed by a pause, so the same character twice is two
//! presses and nothing is ever left held down.
//!
//! Pacing: the computer polls the interrupt endpoint (USB: every 1–4 ms
//! with the firmware's interval); the pause between reports is longer than
//! that, so every report is read before the next replaces it (the
//! controller's behaviour with a full interrupt FIFO is U-USB-11).

use crate::layout::{self, Layout};
use crate::report::KeyboardReport;

/// Default pause after each report.
pub const DEFAULT_DELAY_MS: u16 = 12;
/// Pause bounds accepted from settings and the developer channel.
pub const MIN_DELAY_MS: u16 = 2;
pub const MAX_DELAY_MS: u16 = 500;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TypistConfig {
    pub layout: Layout,
    /// Digits from the keypad (Num Lock must be on at the computer).
    pub keypad_digits: bool,
    /// Pause after each report, milliseconds.
    pub delay_ms: u16,
}

impl Default for TypistConfig {
    fn default() -> Self {
        TypistConfig { layout: Layout::Us, keypad_digits: false, delay_ms: DEFAULT_DELAY_MS }
    }
}

/// One report and the pause after it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Out {
    pub report: KeyboardReport,
    pub wait_ms: u16,
}

/// The press/release sequence for one text. The text is the caller's (it
/// may be a secret the caller wipes); the typist holds only a position.
#[derive(Clone, Copy, Debug)]
pub struct Typist {
    cfg: TypistConfig,
    pos: usize,
    pressed: bool,
    done: bool,
    typed: u32,
}

impl Typist {
    #[must_use]
    pub fn new(cfg: TypistConfig) -> Typist {
        let delay_ms = cfg.delay_ms.clamp(MIN_DELAY_MS, MAX_DELAY_MS);
        Typist { cfg: TypistConfig { delay_ms, ..cfg }, pos: 0, pressed: false, done: false, typed: 0 }
    }

    #[must_use]
    pub fn config(&self) -> TypistConfig {
        self.cfg
    }

    /// The first character the layout cannot type: refuse the whole text
    /// then, before any key is sent.
    #[must_use]
    pub fn check(&self, text: &str) -> Option<char> {
        layout::untypable(self.cfg.layout, text, self.cfg.keypad_digits)
    }

    /// The next report for `text` (the same text every call), or `None`
    /// when all is typed and released.
    pub fn next(&mut self, text: &str) -> Option<Out> {
        let wait_ms = self.cfg.delay_ms;
        if self.done {
            return None;
        }
        if self.pressed {
            self.pressed = false;
            return Some(Out { report: KeyboardReport::RELEASED, wait_ms });
        }
        loop {
            let Some(c) = text.get(self.pos..).and_then(|s| s.chars().next()) else {
                self.done = true;
                return None;
            };
            self.pos += c.len_utf8();
            // Untypable characters were refused by `check`; one that slips
            // through is skipped, never guessed.
            if let Some(s) = layout::stroke(self.cfg.layout, c, self.cfg.keypad_digits) {
                self.pressed = true;
                self.typed += 1;
                return Some(Out { report: KeyboardReport::key(s.modifiers, s.usage), wait_ms });
            }
        }
    }

    /// Stop now (suspend, detach, cancel). Returns the release report to
    /// send if a key is down.
    pub fn abort(&mut self) -> Option<KeyboardReport> {
        self.done = true;
        core::mem::take(&mut self.pressed).then_some(KeyboardReport::RELEASED)
    }

    /// Characters pressed so far.
    #[must_use]
    pub fn typed(&self) -> u32 {
        self.typed
    }

    #[must_use]
    pub fn is_done(&self) -> bool {
        self.done
    }
}

/// Something that delivers keyboard reports to a computer: the USB
/// interrupt endpoint (USB-5), the Bluetooth module (BT-4), the simulator.
#[allow(async_fn_in_trait)]
pub trait Transport {
    type Error;
    async fn send(&mut self, report: &KeyboardReport) -> Result<(), Self::Error>;
}

/// Why typing stopped early.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TypeError<E> {
    /// The layout cannot type this character; nothing was sent.
    Untypable(char),
    /// `cancel` said stop (suspend, detach, the user); keys were released.
    Cancelled,
    /// The transport failed; a release was attempted.
    Transport(E),
}

/// Type `text`: `sleep(ms)` between reports, `cancel()` checked before each
/// report. Every path that sent a press ends with a release attempt.
/// Returns the characters typed.
pub async fn type_text<T, S, F>(
    t: &mut T,
    cfg: TypistConfig,
    text: &str,
    mut sleep: S,
    mut cancel: impl FnMut() -> bool,
) -> Result<u32, TypeError<T::Error>>
where
    T: Transport,
    S: FnMut(u16) -> F,
    F: core::future::Future<Output = ()>,
{
    let mut ty = Typist::new(cfg);
    if let Some(c) = ty.check(text) {
        return Err(TypeError::Untypable(c));
    }
    loop {
        if cancel() {
            if let Some(r) = ty.abort() {
                let _ = t.send(&r).await;
            }
            return Err(TypeError::Cancelled);
        }
        let Some(o) = ty.next(text) else { return Ok(ty.typed()) };
        if let Err(e) = t.send(&o.report).await {
            // Whatever failed (a press or a release), release everything.
            let _ = ty.abort();
            let _ = t.send(&KeyboardReport::RELEASED).await;
            return Err(TypeError::Transport(e));
        }
        sleep(o.wait_ms).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::FakeHost;
    use embassy_futures::block_on;
    use std::string::String;
    use std::vec::Vec;

    struct Wire {
        host: FakeHost,
        reports: Vec<[u8; 8]>,
        fail_at: Option<usize>,
    }

    impl Transport for Wire {
        type Error = ();
        async fn send(&mut self, r: &KeyboardReport) -> Result<(), ()> {
            if self.fail_at == Some(self.reports.len()) {
                self.fail_at = None;
                return Err(());
            }
            self.reports.push(r.bytes());
            self.host.feed(&r.bytes());
            Ok(())
        }
    }

    fn wire(layout: Layout) -> Wire {
        Wire { host: FakeHost::new(layout), reports: Vec::new(), fail_at: None }
    }

    #[test]
    fn types_and_releases_every_character() {
        for l in Layout::ALL {
            let mut w = wire(l);
            let text = "Hello, world 2026!\n";
            let mut waited = 0u32;
            let cfg = TypistConfig { layout: l, ..TypistConfig::default() };
            let n = block_on(type_text(
                &mut w,
                cfg,
                text,
                |ms| {
                    waited += u32::from(ms);
                    core::future::ready(())
                },
                || false,
            ))
            .unwrap();
            assert_eq!(n, 19);
            assert_eq!(String::from(w.host.text()), text, "{l:?}");
            assert_eq!(w.reports.len(), 38, "press + release per character");
            assert!(w.host.all_released());
            assert_eq!(waited, 38 * u32::from(DEFAULT_DELAY_MS));
            // Double letters are two presses with a release between.
            assert!(w.reports.windows(2).all(|p| p[0] != p[1]));
        }
    }

    #[test]
    fn keypad_digits_and_refusals() {
        let mut w = wire(Layout::Fr);
        let cfg = TypistConfig { layout: Layout::Fr, keypad_digits: true, delay_ms: 1 };
        block_on(type_text(&mut w, cfg, "123456\n", |_| core::future::ready(()), || false)).unwrap();
        assert_eq!(w.host.text(), "123456\n");
        assert!(w.reports.iter().all(|r| r[0] == 0), "no Shift with the keypad");
        let mut w = wire(Layout::De);
        let cfg = TypistConfig { layout: Layout::De, ..TypistConfig::default() };
        let r = block_on(type_text(&mut w, cfg, "a^b", |_| core::future::ready(()), || false));
        assert_eq!(r, Err(TypeError::Untypable('^')));
        assert!(w.reports.is_empty(), "nothing sent for a refused text");
        assert_eq!(Typist::new(TypistConfig { delay_ms: 0, ..cfg }).config().delay_ms, MIN_DELAY_MS);
    }

    #[test]
    fn cancel_and_transport_errors_release() {
        // Cancelled after the first press: a release follows.
        let mut w = wire(Layout::Us);
        let mut polls = 0;
        let r = block_on(type_text(
            &mut w,
            TypistConfig::default(),
            "abc",
            |_| core::future::ready(()),
            || {
                polls += 1;
                polls == 2
            },
        ));
        assert_eq!(r, Err(TypeError::Cancelled));
        assert_eq!(w.host.text(), "a");
        assert!(w.host.all_released());
        // The release report itself fails once: the retry releases.
        let mut w = wire(Layout::Us);
        w.fail_at = Some(1);
        let r =
            block_on(type_text(&mut w, TypistConfig::default(), "ab", |_| core::future::ready(()), || false));
        assert_eq!(r, Err(TypeError::Transport(())));
        assert!(w.host.all_released());
        // Abort with nothing pressed sends nothing.
        let mut t = Typist::new(TypistConfig::default());
        assert_eq!(t.abort(), None);
        assert!(t.is_done() && t.next("x").is_none());
    }
}
