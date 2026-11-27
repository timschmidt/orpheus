//! The player as the suite and the fuzzer see it (VER-7/8): keys, the
//! test-command mailbox, screenshots, INFO and the fault record. Two
//! implementations: [`UsbPlayer`] (the developer channel) and [`SimPlayer`]
//! (the simulator behind the same interface: a dry run of the suite without
//! hardware, and the host tests of the runner).

use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use orpheus_host::devchan::DevChannel;
use orpheus_proto::devchan::{REBOOT_NORMAL, REQ_KEYS, REQ_TEST};
use orpheus_proto::fault::{self, FaultRecord};
use orpheus_sim::AppSim;
use orpheus_ui::Key;

use super::script::{Step, TAP_DOWN_MS, TAP_UP_MS};

/// How long a player that stopped answering is waited for (watchdog and
/// boot-counter path, CLAUDE.md rule 7).
pub const COME_BACK: Duration = Duration::from_secs(120);

/// The UI's top screen (`ui top`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Top {
    pub id: u8,
    pub name: String,
    pub depth: u8,
    pub dialog: bool,
    pub overlay: bool,
    pub locked: bool,
    pub screen_on: bool,
    /// The screen saver on the panel (VER-13; firmware `ui top` `saver=`).
    pub saver: bool,
    /// The backlight's PWM4 duty of 16, 0 dark (VER-13 Brightness;
    /// firmware `ui top` `bl=`; `None` from a firmware without it).
    pub bl: Option<u8>,
}

impl Top {
    /// `top ID NAME depth=D dialog=0|1 overlay=0|1 locked=0|1 screen=0|1
    /// look=0|1 saver=0|1 bl=DUTY`.
    pub fn parse(s: &str) -> Option<Top> {
        let mut w = s.split_whitespace();
        if w.next()? != "top" {
            return None;
        }
        let id = w.next()?.parse().ok()?;
        let name = w.next()?.to_string();
        let mut t = Top {
            id,
            name,
            depth: 0,
            dialog: false,
            overlay: false,
            locked: false,
            screen_on: true,
            saver: false,
            bl: None,
        };
        for kv in w {
            let (k, v) = kv.split_once('=')?;
            let n: u8 = v.parse().ok()?;
            match k {
                "depth" => t.depth = n,
                "dialog" => t.dialog = n != 0,
                "overlay" => t.overlay = n != 0,
                "locked" => t.locked = n != 0,
                "screen" => t.screen_on = n != 0,
                "saver" => t.saver = n != 0,
                "bl" => t.bl = Some(n),
                _ => {}
            }
        }
        Some(t)
    }

    pub fn text(&self) -> String {
        let mut t = format!(
            "top {} {} depth={} dialog={} overlay={} locked={} screen={} saver={}",
            self.id,
            self.name,
            self.depth,
            u8::from(self.dialog),
            u8::from(self.overlay),
            u8::from(self.locked),
            u8::from(self.screen_on),
            u8::from(self.saver)
        );
        if let Some(bl) = self.bl {
            t.push_str(&format!(" bl={bl}"));
        }
        t
    }
}

/// INFO: uptime and the stack high-water.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Health {
    pub uptime_ms: u32,
    pub stack_free: u32,
    pub stack_size: u32,
}

pub trait Player {
    /// A key edge (`orpheus_ui::Key`; never Power: the script refuses it).
    fn key(&mut self, k: Key, down: bool) -> Result<()>;
    /// Let `ms` pass.
    fn sleep(&mut self, ms: u64);
    /// TEST OUT: a firmware test command.
    fn send(&mut self, text: &str) -> Result<()>;
    /// TEST IN: the reply of the last command.
    fn reply(&mut self) -> Result<String>;
    /// The frame buffer, RGB565.
    fn frame(&mut self) -> Result<Vec<u16>>;
    fn health(&mut self) -> Result<Health>;
    /// The fault record (it survives resets), if one was written.
    fn fault(&mut self) -> Result<Option<FaultRecord>>;
    /// The firmware's log ring, oldest line first (empty where there is
    /// none, as in the simulator).
    fn log_lines(&mut self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    /// One word of the player's memory (`None` in the simulator).
    fn peek_word(&mut self, _addr: u32) -> Result<Option<u32>> {
        Ok(None)
    }
    /// `(uptime ms, pager line loads)` from the firmware's counters
    /// (`None` in the simulator).
    fn pager_loads(&mut self) -> Result<Option<(u32, u32)>> {
        Ok(None)
    }
    /// Normal reboot; returns once the player answers again.
    fn reboot(&mut self) -> Result<()>;
    /// The channel failed: wait (up to [`COME_BACK`]) for the player.
    fn reconnect(&mut self) -> Result<()>;
    /// A `sim:` line of a journey. The simulator player applies it so it
    /// stays in the reference's state; the real player is in that state.
    fn mirror(&mut self, _cmd: &str) -> Result<()> {
        Ok(())
    }
    /// A journey starts (the simulator player starts afresh).
    fn begin_journey(&mut self) {}
}

pub fn top(p: &mut dyn Player) -> Result<Top> {
    p.send("ui top")?;
    let r = p.reply()?;
    Top::parse(&r).ok_or_else(|| anyhow!("no `ui top` reply (got `{r}`): firmware older than VER-7?"))
}

pub fn tap(p: &mut dyn Player, k: Key) -> Result<()> {
    p.key(k, true)?;
    p.sleep(TAP_DOWN_MS);
    p.key(k, false)?;
    p.sleep(TAP_UP_MS);
    Ok(())
}

pub fn hold(p: &mut dyn Player, k: Key, ms: u64) -> Result<()> {
    p.key(k, true)?;
    p.sleep(ms);
    p.key(k, false)?;
    p.sleep(TAP_UP_MS);
    Ok(())
}

/// Poll `ui top` until the top screen is `name`.
pub fn expect_screen(p: &mut dyn Player, name: &str, timeout_ms: u64) -> Result<Top> {
    let mut waited = 0;
    loop {
        let t = top(p)?;
        if t.name == name {
            return Ok(t);
        }
        if waited >= timeout_ms {
            bail!("expected screen {name}, top is {}", t.name);
        }
        p.sleep(50);
        waited += 50;
    }
}

/// A frame that stays the same over 100 ms (at most `tries` reads; an
/// animation returns the last one).
pub fn settled_frame(p: &mut dyn Player, tries: u32) -> Result<Vec<u16>> {
    let mut f = p.frame()?;
    for _ in 1..tries {
        p.sleep(100);
        let g = p.frame()?;
        if g == f {
            return Ok(g);
        }
        f = g;
    }
    Ok(f)
}

/// Run a key step (shots and screen checks are the caller's).
pub fn key_step(p: &mut dyn Player, s: &Step) -> Result<()> {
    match s {
        Step::Tap(k, n) => {
            for _ in 0..*n {
                tap(p, *k)?;
            }
        }
        Step::Hold(k, ms) => hold(p, *k, *ms)?,
        Step::Press(k) => p.key(*k, true)?,
        Step::Release(k) => p.key(*k, false)?,
        Step::Wait(ms) => p.sleep(*ms),
        Step::Shot(_) | Step::Expect(_) => {}
    }
    Ok(())
}

/// Real time allowed beyond a command's timeout (slow polls).
#[cfg(not(test))]
const GRACE_MS: u64 = 5_000;
#[cfg(test)]
const GRACE_MS: u64 = 200;

/// Send a test command; with `want`, poll the reply (up to `timeout_ms`)
/// until it matches. The reply mailbox is first primed with `ui top`
/// (answered at once), so a stale reply of an earlier command never
/// matches.
pub fn command(
    p: &mut dyn Player,
    text: &str,
    want: Option<&regex::Regex>,
    timeout_ms: u64,
) -> Result<String> {
    let primed = match want {
        Some(_) => {
            // `ui top` itself is primed with another reply answered at once
            // (VER-13 journeys check the screen state with it).
            p.send(if text == "ui top" { "settings test-session" } else { "ui top" })?;
            Some(p.reply()?)
        }
        None => None,
    };
    p.send(text)?;
    let Some(re) = want else {
        p.sleep(50);
        return Ok(String::new());
    };
    let mut waited = 0;
    // Real time too: a player that answers each poll slowly (a reply of
    // ~2 s that does not fail) made `waited`, which counts only the sleeps,
    // stretch a 15 s timeout into many minutes (soak start-up hung for
    // over 28 min, 2026-11-25).
    let t0 = std::time::Instant::now();
    loop {
        let r = p.reply()?;
        if Some(&r) != primed.as_ref() && re.is_match(&r) {
            return Ok(r);
        }
        if waited >= timeout_ms || t0.elapsed().as_millis() as u64 >= timeout_ms.saturating_add(GRACE_MS) {
            let last = if Some(&r) == primed.as_ref() { "no reply".to_string() } else { format!("`{r}`") };
            bail!("`{text}`: reply does not match /{re}/: {last}");
        }
        p.sleep(100);
        waited += 100;
    }
}

/// `ui test-look on|off`: for this session the player draws with the
/// simulator's appearance (Dark, Blue, English, all Home apps, clock in the
/// bar, lists roll round) without touching the owner's stored settings; a
/// reboot clears it. On: waits for the UI's next loop (≤ 1 s) to redraw.
pub fn test_look(p: &mut dyn Player, on: bool) -> Result<()> {
    let want = if on { "test-look on" } else { "test-look off" };
    let re = regex::Regex::new(&format!("^{want}$|^commands:|^card:"))?;
    let r = command(p, &format!("ui {want}"), Some(&re), 3000)?;
    if r != want {
        bail!("the firmware does not know `ui {want}` (reply `{r}`): stage a build with the VER-7 test look");
    }
    if on {
        p.sleep(1200);
    }
    Ok(())
}

/// The developer channel of a running Orpheus.
pub struct UsbPlayer {
    d: Option<DevChannel>,
}

impl UsbPlayer {
    pub fn open() -> Result<Self> {
        Ok(Self { d: Some(DevChannel::open(Duration::from_secs(3))?) })
    }

    fn d(&mut self) -> Result<&DevChannel> {
        if self.d.is_none() {
            self.d = Some(DevChannel::open(Duration::from_secs(3))?);
        }
        self.d.as_ref().ok_or_else(|| anyhow!("no developer channel"))
    }

    /// Upload a file (under `/.orpheus/`) through the firmware.
    pub fn upload(&mut self, remote: &str, data: &[u8]) -> Result<()> {
        super::upload(self.d()?, remote, data)
    }

    /// STATS group `g` (words; the soak's pager, clock and ring counters).
    pub fn stats(&mut self, g: u16) -> Result<Vec<u32>> {
        self.d()?.stats(g)
    }

    /// INFO's battery voltage (mV).
    pub fn battery_mv(&mut self) -> Result<u16> {
        Ok(self.d()?.info()?.battery_mv)
    }

    fn come_back(&mut self) -> Result<()> {
        let t0 = Instant::now();
        let d = DevChannel::open(COME_BACK).with_context(|| {
            format!(
                "the player did not come back within {} s: stop and ask the user (CLAUDE.md rule 7)",
                COME_BACK.as_secs()
            )
        })?;
        // The USB task answers before the UI has drawn its first frame.
        std::thread::sleep(Duration::from_millis(1500));
        crate::util::log(&format!("   player back after {:.1} s", t0.elapsed().as_secs_f32()));
        self.d = Some(d);
        Ok(())
    }
}

/// Developer-channel key index (`orpheus_input::Key`) of a UI key.
pub fn hw_key(k: Key) -> u8 {
    use orpheus_input::Key as H;
    (match k {
        Key::Power => H::Power,
        Key::Back => H::Back,
        Key::Menu => H::Menu,
        Key::Ok => H::Ok,
        Key::Prev => H::Prev,
        Key::Next => H::Next,
        Key::Up => H::Up,
        Key::Down => H::Down,
        Key::VolUp => H::VolUp,
        Key::VolDown => H::VolDown,
    }) as u8
}

impl Player for UsbPlayer {
    fn key(&mut self, k: Key, down: bool) -> Result<()> {
        if k == Key::Power {
            bail!("refusing the Power key (CLAUDE.md rule 4)");
        }
        self.d()?.control_out(REQ_KEYS, 0, 0, &[hw_key(k), u8::from(down)])
    }
    fn sleep(&mut self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }
    fn send(&mut self, text: &str) -> Result<()> {
        self.d()?.control_out(REQ_TEST, 0, 0, text.as_bytes())
    }
    fn reply(&mut self) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.d()?.control_in(REQ_TEST, 0, 0, 1024)?).into_owned())
    }
    fn frame(&mut self) -> Result<Vec<u16>> {
        let b = self.d()?.frame()?;
        if b.len() < 128 * 128 * 2 {
            bail!("short frame ({} bytes)", b.len());
        }
        Ok(b.as_chunks::<2>().0.iter().map(|p| u16::from_le_bytes(*p)).collect())
    }
    fn health(&mut self) -> Result<Health> {
        let i = self.d()?.info()?;
        Ok(Health { uptime_ms: i.uptime_ms, stack_free: i.reserved & 0xFFFF, stack_size: i.reserved >> 16 })
    }
    fn fault(&mut self) -> Result<Option<FaultRecord>> {
        Ok(FaultRecord::from_bytes(&self.d()?.peek(fault::BASE, fault::SIZE)?))
    }
    fn pager_loads(&mut self) -> Result<Option<(u32, u32)>> {
        let r = self.d()?.control_in(orpheus_proto::devchan::REQ_STATS, 0, 0, 128)?;
        let w = |i: usize| r.get(4 * i..4 * i + 4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]));
        Ok(w(8).zip(w(4)))
    }
    fn peek_word(&mut self, addr: u32) -> Result<Option<u32>> {
        let b = self.d()?.peek(addr, 4)?;
        Ok(b.get(..4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])))
    }
    fn log_lines(&mut self) -> Result<Vec<String>> {
        let area = self.d()?.log_area()?;
        let mut out = vec![0u8; area.len()];
        let Some((n, _)) = orpheus_proto::logring::unroll(&area, &mut out) else { return Ok(Vec::new()) };
        Ok(out[..n]
            .split(|b| *b == 0)
            .filter(|l| !l.is_empty())
            .map(|l| String::from_utf8_lossy(l).into_owned())
            .collect())
    }
    fn reboot(&mut self) -> Result<()> {
        if let Some(d) = self.d.take() {
            d.reboot(REBOOT_NORMAL);
        }
        std::thread::sleep(Duration::from_secs(2));
        self.come_back()
    }
    fn reconnect(&mut self) -> Result<()> {
        self.d = None;
        self.come_back()
    }
}

/// The simulator as a player: what the real player would show if the
/// firmware behaved exactly like the simulator.
pub struct SimPlayer {
    pub sim: AppSim,
    reply: String,
    /// Fresh simulator at every journey (the default), or one session.
    pub fresh: bool,
    /// The screen timers a session armed (firmware `ui.rs` `TIMER_HOLD`).
    hold: orpheus_apps::services::TimerHold,
}

impl Default for SimPlayer {
    fn default() -> Self {
        Self { sim: AppSim::new(), reply: String::new(), fresh: true, hold: Default::default() }
    }
}

impl SimPlayer {
    /// A session's `test-set` as the firmware takes it (VER-13,
    /// `ui.rs` `session_set_hooks`): the screen timers armed while one is
    /// off its default; when the last goes back, the screen comes on and
    /// the keys unlock.
    fn session_set(&mut self, id: i32, v: i32) {
        let (Ok(id), Ok(v)) = (u16::try_from(id), i16::try_from(v)) else { return };
        let was = self.hold.bits();
        self.hold.test_set(orpheus_apps::settings::SettingId(id), v);
        if self.sim.sys.timer_hold.is_some() {
            self.sim.sys.timer_hold = Some(self.hold);
        }
        if was != 0 && self.hold.bits() == 0 {
            self.sim.shell.gate.screen_on = true;
            self.sim.shell.gate.locked = false;
            self.sim.shell.invalidate();
        }
    }
}

#[cfg(test)]
impl SimPlayer {
    /// One simulator for the whole session (state carries over, as on the
    /// player).
    pub fn one_session() -> Self {
        Self { fresh: false, ..Self::default() }
    }
}

/// The `ui top` reply for a simulator.
pub fn sim_top(sim: &AppSim) -> Top {
    Top {
        id: sim.shell.top_id().0,
        name: sim.screen().to_string(),
        depth: u8::try_from(sim.shell.depth()).unwrap_or(u8::MAX),
        dialog: sim.shell.dialog_open(),
        overlay: sim.shell.overlay_shown(),
        locked: sim.shell.gate.locked,
        screen_on: sim.shell.gate.screen_on,
        saver: sim.shell.gate.saver && !sim.shell.gate.screen_on,
        bl: Some(sim.backlight()),
    }
}

impl Player for SimPlayer {
    fn key(&mut self, k: Key, down: bool) -> Result<()> {
        if k == Key::Power {
            bail!("refusing the Power key (CLAUDE.md rule 4)");
        }
        if down {
            self.sim.press(k);
        } else {
            self.sim.release(k);
        }
        Ok(())
    }
    fn sleep(&mut self, ms: u64) {
        self.sim.advance(ms);
    }
    fn send(&mut self, text: &str) -> Result<()> {
        self.reply = match text {
            "ui top" => sim_top(&self.sim).text(),
            "status" => {
                use orpheus_apps::services::PlaybackService;
                // `shuf=`: the queue follows Settings › Playback › Shuffle
                // (VER-13 journey `settings_shuffle`); `fade=`, `gapless=`,
                // `clicks=` as the firmware's (journey `settings_playback`).
                let shuf = self.sim.sys.settings.get(orpheus_apps::settings::id::SHUFFLE);
                let m = &self.sim.sys.media;
                let fade = m.last_fade.map_or("-".to_string(), |f| f.to_string());
                let gapless = m.last_change.map_or("-", |g| if g { "1" } else { "0" });
                let tail = format!("fade={fade} gapless={gapless} clicks={}", self.sim.sys.clicks);
                // The firmware says "playing" while a track is open (also
                // paused).
                match self.sim.sys.playing().map(|p| p.source) {
                    // `syn:` as the firmware's while a synth source plays
                    // (SYN-3; the simulator's stand-in: one voice sounding,
                    // the limit at its maximum, never overloaded).
                    Some(orpheus_ui::event::Source::Playing | orpheus_ui::event::Source::Paused) => format!(
                        "playing frames=44100 rate=44100 underruns=0 corrupt=0 shuf={shuf} {tail} \
                         syn: v=1 vlim=16 over=0 reads=0"
                    ),
                    _ => format!("idle frames=0 rate=0 underruns=0 corrupt=0 {tail}"),
                }
            }
            // VER-12/13: the RAM-only settings session and its settings.
            "settings test-session" | "settings test-session on" => {
                "test-session on held=0 store=ok keys=0 crc=00000000".into()
            }
            t if t.starts_with("settings test-set ") => {
                let w: Vec<&str> = t.split_whitespace().skip(2).collect();
                let num = |s: &str| match s.strip_prefix("0x") {
                    Some(h) => i32::from_str_radix(h, 16).ok(),
                    None => s.parse().ok(),
                };
                match (w.first().and_then(|s| num(s)), w.get(1).and_then(|s| num(s))) {
                    // As the firmware: only what a session may set (never
                    // the developer channel, power, clock, USB …).
                    (Some(id), Some(_)) if !u16::try_from(id).is_ok_and(super::soak::session_settable) => {
                        "test-set: not allowed".into()
                    }
                    (Some(id), Some(v)) => {
                        self.sim.run_script(&format!("set:{id:#06x}={v}"))?;
                        self.session_set(id, v);
                        format!("test-set {id}={v} ok")
                    }
                    _ => "usage: settings test-set ID VALUE".into(),
                }
            }
            // TTS-3: the speech voice as the firmware's `ui speech` says it
            // (fw/firmware/src/speech.rs `status_reply`).
            "ui speech" => sim_speech(&self.sim),
            // Books (BOOK-1/2 wiring), as the firmware's `books status`
            // and `books test on|off` (`crate::books`).
            "books status" => sim_books_status(&self.sim),
            "books test on" | "books test off" => {
                let on = text.ends_with("on");
                self.sim.run_script(if on { "set:bookstest=on" } else { "set:bookstest=off" })?;
                if on { "books test on" } else { "books test off" }.into()
            }
            "fm status" => {
                let i = self.sim.sys.radio.radio.info();
                format!("fm status: phase={:?} khz={} rssi={}", i.phase, i.khz, i.rssi)
            }
            t if t.starts_with("ls ") => "Music/\n.orpheus/\n(2 entries)".into(),
            // SYN-3: the bench's load on the host (the same samples as the
            // player's: its hash is the reference).
            t if t.starts_with("synth bench ") => sim_bench(t),
            t if t.starts_with("playdir ") => "playdir ok".into(),
            // The simulator's defaults are the test look already; as on the
            // player the look holds the screen timers (unless a session
            // armed them) and off they run (VER-13).
            "ui test-look on" => {
                self.sim.sys.timer_hold = Some(self.hold);
                "test-look on".into()
            }
            "ui test-look off" => {
                self.sim.sys.timer_hold = None;
                "test-look off".into()
            }
            // Commands without a reply leave the last one.
            "stop" | "crc on" | "crc off" => return Ok(()),
            _ => "commands: ls PATH, play PATH, playdir PATH [START], vol N".into(),
        };
        Ok(())
    }
    fn reply(&mut self) -> Result<String> {
        Ok(self.reply.clone())
    }
    fn frame(&mut self) -> Result<Vec<u16>> {
        self.sim.draw();
        Ok(self.sim.fb.clone())
    }
    fn health(&mut self) -> Result<Health> {
        Ok(Health {
            uptime_ms: u32::try_from(self.sim.now.0).unwrap_or(u32::MAX),
            stack_free: 6000,
            stack_size: 27_000,
        })
    }
    fn fault(&mut self) -> Result<Option<FaultRecord>> {
        Ok(None)
    }
    fn reboot(&mut self) -> Result<()> {
        self.sim = AppSim::new();
        self.hold = Default::default();
        Ok(())
    }
    fn reconnect(&mut self) -> Result<()> {
        Ok(())
    }
    fn mirror(&mut self, cmd: &str) -> Result<()> {
        self.sim.run_script(cmd).map(|_| ())
    }
    fn begin_journey(&mut self) {
        if self.fresh {
            self.sim = AppSim::new();
            self.hold = Default::default();
        }
    }
}

/// `ui speech` for a simulator: the fields of the firmware's reply that the
/// simulator knows (its speech has no ring: `ring=0 dropped=0 late=0`; it
/// is never alone while music plays).
pub fn sim_speech(sim: &AppSim) -> String {
    use orpheus_sim::speech::Said;
    use orpheus_tts::mix::{MusicMode, Q16_ONE};
    let Some(a) = sim.a11y.as_ref() else { return "speech: not in this build (feature speech)".into() };
    let sp = &a.speech;
    let c = orpheus_apps::services::reader_config(&sim.sys);
    let busy = sp.busy();
    let duck = if busy && sp.music { MusicMode::from_index(c.music).duck_q16() } else { Q16_ONE };
    let count = |f: fn(&Said) -> bool| sp.log.iter().filter(|l| f(l)).count();
    let mut said = sp.last().unwrap_or("").to_string();
    if said.len() > 64 {
        let mut n = 64;
        while !said.is_char_boundary(n) {
            n -= 1;
        }
        said.truncate(n);
    }
    format!(
        "speech sounding={} alone={} queued={} ring=0 vol={} music={} duck={duck} texts={} earcons={} stops={} \
         dropped=0 late=0 rendered={} said={said}",
        u8::from(busy),
        u8::from(busy && !sp.music),
        u8::from(busy),
        c.volume,
        c.music,
        count(|l| matches!(l, Said::Text(..))),
        count(|l| matches!(l, Said::Earcon(..))),
        count(|l| matches!(l, Said::Stop(..))),
        sp.rendered(),
    )
}

/// `synth bench I [N] [BLOCKS]` on the host: the reply's words the
/// simulator can know (no cycles), with the render's hash.
/// `books status` as the firmware answers it (`fw/firmware/src/books/on.rs`).
fn sim_books_status(sim: &AppSim) -> String {
    use orpheus_apps::books::core::shelf::{LibPhase, Phase};
    let b = &sim.sys.books;
    let (l, k) = (&b.shelf.lib, &b.shelf.book);
    let lib = match l.phase {
        LibPhase::Loading => "loading",
        LibPhase::Searching => "searching",
        LibPhase::Ready => "ready",
        LibPhase::Unknown => "unknown",
    };
    let book = match k.phase {
        Phase::Opening => "opening",
        Phase::Converting => "converting",
        Phase::Ready => "ready",
        Phase::Failed => "failed",
        Phase::Closed => "closed",
    };
    let page = k.page_no.map_or("-".to_string(), |p| p.to_string());
    format!(
        "books: lib={lib} n={} found={} book={book} conv={} wait={} top={} page={page} pages={} laid={} \
         steps={} lent=0 retaken=0 mkdir=0 test={}",
        l.count,
        l.found,
        k.permille,
        u8::from(k.waiting),
        k.top.offset,
        k.pages,
        u8::from(k.complete),
        b.shelf.steps,
        u8::from(b.test),
    )
}

fn sim_bench(t: &str) -> String {
    use orpheus_synth::bench;
    let mut w = t.split_whitespace().skip(2).map(str::parse::<u32>);
    let i = w.next().and_then(Result::ok).unwrap_or(u32::MAX) as usize;
    let n = w.next().and_then(Result::ok).map_or(bench::VOICES, |n| n.min(255) as u8);
    let blocks = w.next().and_then(Result::ok).unwrap_or(bench::BLOCKS).clamp(1, 100_000);
    let mut e = Box::new(<orpheus_synth::Engine as bytemuck::Zeroable>::zeroed());
    let mut buf = vec![0i16; 2 * bench::BLOCK];
    match bench::run(i, n, bench::WARM, blocks, &mut e, &mut buf, |_| {}) {
        Some((h, _)) => {
            let n = bench::voices(bench::LOADS[i], n);
            format!("bench {i} n={n} frames={} hash={h:016x}", u64::from(blocks) * bench::BLOCK as u64)
        }
        None => format!("synth bench: no load {i}"),
    }
}

#[cfg(test)]
mod tests {
    /// A player whose replies each take real time and never match.
    struct Slow;
    impl Player for Slow {
        fn key(&mut self, _: Key, _: bool) -> Result<()> {
            Ok(())
        }
        fn sleep(&mut self, _: u64) {}
        fn send(&mut self, _: &str) -> Result<()> {
            Ok(())
        }
        fn reply(&mut self) -> Result<String> {
            std::thread::sleep(std::time::Duration::from_millis(300));
            Ok("busy".into())
        }
        fn frame(&mut self) -> Result<Vec<u16>> {
            Ok(Vec::new())
        }
        fn health(&mut self) -> Result<Health> {
            anyhow::bail!("unused")
        }
        fn fault(&mut self) -> Result<Option<FaultRecord>> {
            Ok(None)
        }
        fn reboot(&mut self) -> Result<()> {
            Ok(())
        }
        fn reconnect(&mut self) -> Result<()> {
            Ok(())
        }
    }

    /// Regression (2026-11-25): the timeout counted only the sleeps, so a
    /// player answering each poll slowly kept a soak's start-up waiting
    /// for many minutes. The wait is bounded in real time too.
    #[test]
    fn regress_command_timeout_is_bounded_in_real_time() {
        let re = regex::Regex::new("^never$").unwrap();
        let t0 = std::time::Instant::now();
        // Counting only the 100 ms sleeps, 3 s took 30 polls of 0.3 s: 9 s.
        assert!(command(&mut Slow, "ls /", Some(&re), 3_000).is_err());
        assert!(t0.elapsed() < std::time::Duration::from_secs(5), "{:?}", t0.elapsed());
    }

    use super::*;

    #[test]
    fn top_round_trip() {
        let t = Top {
            id: 7,
            name: "SettingsGroup".into(),
            depth: 3,
            dialog: true,
            overlay: true,
            locked: false,
            screen_on: true,
            saver: false,
            bl: Some(8),
        };
        assert_eq!(Top::parse(&t.text()), Some(t.clone()));
        assert_eq!(Top::parse(&Top { bl: None, ..t }.text()).map(|t| t.bl), Some(None));
        // The firmware's reply (fw/firmware/src/ui.rs `top_reply`).
        let f = Top::parse("top 1 Home depth=1 dialog=0 overlay=0 locked=0 screen=1").unwrap();
        assert_eq!((f.id, f.name.as_str(), f.depth, f.screen_on), (1, "Home", 1, true));
        let f = Top::parse("top 1 Home depth=1 dialog=0 overlay=0 locked=1 screen=0 look=1 saver=1").unwrap();
        assert!(f.locked && !f.screen_on && f.saver);
        assert_eq!(f.bl, None, "a firmware without bl=");
        let f = Top::parse("top 1 Home depth=1 dialog=0 overlay=0 locked=0 screen=1 look=1 saver=0 bl=15")
            .unwrap();
        assert_eq!(f.bl, Some(15));
        assert_eq!(Top::parse("idle frames=0"), None);
    }

    /// Regression (VER-13, 2026-11-26): `command` primed the reply with
    /// `ui top` before every checked command, so a journey's `cmd: ui top ~
    /// …` check could never pass (its reply equals the primed one: "no
    /// reply"). `ui top` is now primed with another command.
    #[test]
    fn regress_ui_top_check_was_primed_with_itself() {
        let mut p = SimPlayer::default();
        let re = regex::Regex::new("screen=1").unwrap();
        let r = command(&mut p, "ui top", Some(&re), 1000).unwrap();
        assert!(r.starts_with("top ") && r.contains("screen=1"), "{r}");
    }

    /// VER-13: the RAM-only session never sets the developer channel (the
    /// lead's only way to the player), power, clock or USB settings; the
    /// simulator standing in for the player refuses them as the firmware.
    #[test]
    fn session_refuses_the_developer_channel() {
        for id in [0x0903u16, 0x0902, 0x0901, 0x0403, 0x0402, 0x0505, 0x0506, 0x0507, 0x0601] {
            assert!(!super::super::soak::session_settable(id), "{id:#06x}");
        }
        let mut p = SimPlayer::default();
        p.send("settings test-set 0x0903 0").unwrap();
        assert_eq!(p.reply().unwrap(), "test-set: not allowed");
        assert_eq!(p.sim.sys.settings.get(orpheus_apps::settings::id::DEV_CHANNEL), 1);
    }

    /// VER-13: the screen timers in a session as on the player: held until
    /// a test-set arms them; back at their defaults the screen comes on and
    /// the keys unlock (no journey leaves the player locked).
    #[test]
    fn session_screen_timers() {
        let mut p = SimPlayer::default();
        let top = |p: &mut SimPlayer| {
            p.send("ui top").unwrap();
            Top::parse(&p.reply().unwrap()).unwrap()
        };
        p.sleep(30_000);
        assert!(top(&mut p).screen_on, "held");
        p.send("settings test-set 0x0302 0").unwrap();
        p.sleep(5_500);
        let t = top(&mut p);
        assert!(!t.screen_on && t.locked, "5 s, then locked at once");
        p.send("settings test-set 0x0302 2").unwrap();
        let t = top(&mut p);
        assert!(t.screen_on && !t.locked, "set back: on and free");
        p.sleep(30_000);
        assert!(top(&mut p).screen_on, "held again");
    }

    #[test]
    fn key_indices_match_the_input_crate() {
        for (i, k) in orpheus_input::Key::ALL.iter().enumerate() {
            let name = match k {
                orpheus_input::Key::VolUp => "vol+".to_string(),
                orpheus_input::Key::VolDown => "vol-".to_string(),
                other => format!("{other:?}").to_lowercase(),
            };
            assert_eq!(usize::from(hw_key(Key::from_name(&name).unwrap())), i, "{name}");
        }
    }

    /// The firmware's test look (`ui test-look on`: the settings
    /// 0x0304–0x030C, Power mode, language, Now Playing view and, until a session sets
    /// them, the accessibility settings 0x1001–0x1003 read as their defaults)
    /// is what the simulator starts with, so it needs no counterpart.
    #[test]
    fn simulator_starts_in_the_test_look() {
        use orpheus_apps::settings::{SettingId, def, id};
        let sim = AppSim::new();
        let ids = (0x0304..=0x030Cu16).chain([0x0401, 0x0A01, 0x0D03, 0x1001, 0x1002, 0x1003]).map(SettingId);
        for sid in ids {
            let Some(d) = def(sid) else { continue };
            assert_eq!(sim.sys.settings.get(sid), d.default, "{:#06x}", sid.0);
        }
        for (sid, v) in [(id::THEME, 0), (id::ACCENT, 0), (id::CLOCK_IN_BAR, 1), (id::LIST_ROLL, 1)] {
            assert_eq!(def(sid).unwrap().default, v, "{:#06x}", sid.0);
        }
        // Every app on Home; of the player's apps (thirteen with Books since
        // BOOK-1/2; the workspace's `orpheus-apps` has the Sequencer too,
        // through the simulator, which the player does not).
        let player: i16 =
            orpheus_apps::App::ALL.iter().filter(|a| a.on_player()).fold(0, |m, &a| m | 1 << a as u8);
        assert_eq!(def(id::HOME_APPS).unwrap().default & player, 0x1FFF);
        let mut p = SimPlayer::default();
        test_look(&mut p, true).unwrap();
        test_look(&mut p, false).unwrap();
    }

    #[test]
    fn sim_player_commands() {
        let mut p = SimPlayer::default();
        let re = regex::Regex::new("underruns=0").unwrap();
        assert!(command(&mut p, "status", Some(&re), 500).unwrap().contains("underruns=0"));
        // A command without a reply never matches the primed `ui top`.
        let re = regex::Regex::new("top|.").unwrap();
        assert!(command(&mut p, "stop", Some(&re), 300).is_err());
        tap(&mut p, Key::Prev).unwrap();
        tap(&mut p, Key::Ok).unwrap();
        assert_eq!(expect_screen(&mut p, "Settings", 500).unwrap().depth, 2);
        hold(&mut p, Key::Back, 700).unwrap();
        assert_eq!(top(&mut p).unwrap().name, "Home");
    }
}
