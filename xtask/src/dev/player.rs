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
}

impl Top {
    /// `top ID NAME depth=D dialog=0|1 overlay=0|1 locked=0|1 screen=0|1`.
    pub fn parse(s: &str) -> Option<Top> {
        let mut w = s.split_whitespace();
        if w.next()? != "top" {
            return None;
        }
        let id = w.next()?.parse().ok()?;
        let name = w.next()?.to_string();
        let mut t = Top { id, name, depth: 0, dialog: false, overlay: false, locked: false, screen_on: true };
        for kv in w {
            let (k, v) = kv.split_once('=')?;
            let n: u8 = v.parse().ok()?;
            match k {
                "depth" => t.depth = n,
                "dialog" => t.dialog = n != 0,
                "overlay" => t.overlay = n != 0,
                "locked" => t.locked = n != 0,
                "screen" => t.screen_on = n != 0,
                _ => {}
            }
        }
        Some(t)
    }

    pub fn text(&self) -> String {
        format!(
            "top {} {} depth={} dialog={} overlay={} locked={} screen={}",
            self.id,
            self.name,
            self.depth,
            u8::from(self.dialog),
            u8::from(self.overlay),
            u8::from(self.locked),
            u8::from(self.screen_on)
        )
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
            p.send("ui top")?;
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
    loop {
        let r = p.reply()?;
        if Some(&r) != primed.as_ref() && re.is_match(&r) {
            return Ok(r);
        }
        if waited >= timeout_ms {
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
}

impl Default for SimPlayer {
    fn default() -> Self {
        Self { sim: AppSim::new(), reply: String::new(), fresh: true }
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
                match self.sim.sys.playing().map(|p| p.source) {
                    Some(orpheus_ui::event::Source::Playing) => {
                        "playing frames=44100 rate=44100 underruns=0 corrupt=0".into()
                    }
                    _ => "idle frames=0 rate=0 underruns=0 corrupt=0".into(),
                }
            }
            "fm status" => {
                let i = self.sim.sys.radio.radio.info();
                format!("fm status: phase={:?} khz={} rssi={}", i.phase, i.khz, i.rssi)
            }
            t if t.starts_with("ls ") => "Music/\n.orpheus/\n(2 entries)".into(),
            t if t.starts_with("playdir ") => "playdir ok".into(),
            // The simulator's defaults are the test look already.
            "ui test-look on" => "test-look on".into(),
            "ui test-look off" => "test-look off".into(),
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
        }
    }
}

#[cfg(test)]
mod tests {
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
        };
        assert_eq!(Top::parse(&t.text()), Some(t));
        // The firmware's reply (fw/firmware/src/ui.rs `top_reply`).
        let f = Top::parse("top 1 Home depth=1 dialog=0 overlay=0 locked=0 screen=1").unwrap();
        assert_eq!((f.id, f.name.as_str(), f.depth, f.screen_on), (1, "Home", 1, true));
        assert_eq!(Top::parse("idle frames=0"), None);
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
    /// 0x0304–0x030C, language and Now Playing view read as their defaults)
    /// is what the simulator starts with, so it needs no counterpart.
    #[test]
    fn simulator_starts_in_the_test_look() {
        use orpheus_apps::settings::{SettingId, def, id};
        let sim = AppSim::new();
        let ids = (0x0304..=0x030Cu16).chain([0x0A01, 0x0D03]).map(SettingId);
        for sid in ids {
            let Some(d) = def(sid) else { continue };
            assert_eq!(sim.sys.settings.get(sid), d.default, "{:#06x}", sid.0);
        }
        for (sid, v) in [
            (id::THEME, 0),
            (id::ACCENT, 0),
            (id::CLOCK_IN_BAR, 1),
            (id::LIST_ROLL, 1),
            (id::HOME_APPS, 0x0FFF),
        ] {
            assert_eq!(def(sid).unwrap().default, v, "{:#06x}", sid.0);
        }
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
