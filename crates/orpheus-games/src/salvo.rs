//! Salvo (GAME-4/5): a turn-based artillery duel for 2–4 tanks.
//!
//! **World.** A height map of up to [`MAX_W`] columns (128–384 px; the
//! screen shows 128), one `u8` per column: the ground is solid below
//! `heights[x]` pixels from the bottom of a [`WORLD_H`]-pixel world. It is
//! made per round from the seed in one of five styles (rolling hills,
//! mountains, a canyon, islands over water, plateaus) with roughness and
//! relief settings. Explosions cut discs out of it and the dirt above a
//! cut falls into the hole at once (a column only ever loses the cut
//! length); dirt bombs add a disc that falls onto the ground. Tanks rest on
//! the column under their centre, fall when it drops (fall damage past
//! [`FALL_FREE`] pixels), slide down steep slopes, and sink in water.
//!
//! **Physics.** SI units, one pixel is [`M_PER_PX`] metres. Shells are
//! integrated with semi-implicit Euler at [`PHYS_HZ`] (the velocity first,
//! then the position with the new velocity), [`SUB`] steps per 60 Hz logic
//! step (time runs 6× faster than real time on screen; OK during the
//! flight makes it [`FAST`] times faster again). Gravity is one of the
//! presets in m/s² ([`GRAVITY`]: Moon, Mars, Earth, Jupiter); the air
//! moves horizontally at the wind speed, re-drawn each turn within the set
//! maximum, with optional gusts that change every [`SUB`] steps of a
//! flight; drag is quadratic in the shell's speed *relative to the air*
//! (`a = −ρ·k·|v − w|·(v − w)`, `k = ½·Cd·A/m` per weapon, [`SPECS`]), so
//! the wind acts through the drag, as it does. Only `+ − × ÷` and a square
//! root by Newton's method ([`sqrt`]) are used, never a fused
//! multiply-add, so the same game runs bit for bit the same on the host,
//! in the browser and on the player's FPU. A shell moves less than a pixel
//! per step (the probe splits longer steps), so it never passes through
//! the ground or a tank.
//!
//! **Weapons** (own set, [`Weapon`]): shell, heavy shell, cluster (splits
//! into three bomblets at the top of its flight), roller (rolls downhill
//! after landing and goes off where it stops or meets a tank), digger
//! (cuts a trench along its way into the ground, then a small blast), dirt
//! bomb (a mound of dirt, no damage), smoke (puffs along its way and where
//! it lands that drift with the wind: a wind indicator). Damage falls off
//! linearly to the edge of the blast. An optional shop between rounds sells
//! the special weapons for the credits earned (a point per damage point
//! dealt, 50 per tank destroyed, 100 for winning a round, 30 a round).
//!
//! **Play.** Players take turns (each round another one starts); a human
//! turn after another human's starts with a "pass to Player n" screen.
//! Computer players ([`Kind`]) choose a target, estimate the wind (with a
//! noise and a bias per round that shrink with the difficulty; Expert
//! knows it and, with gusts on, the gusts as well), then search angle and
//! power by simulating candidate shots with this physics (bisection on the
//! power for a few angles; the budget grows with the difficulty), and
//! correct their aim point from where their last shot at the same target
//! landed (bracketing). They never read the gusts below Expert. The
//! search runs one simulated shot per logic step, so the UI never waits
//! for more than one flight's simulation.
//!
//! Invariants ([`Salvo::check`], `tests/salvo.rs`, the `salvo_keys` fuzz
//! target): no NaN or infinity anywhere; no flying shell below the ground;
//! heights within `0..=TOP` and zero beyond the world; tanks inside the
//! world, health within the setting; the phase consistent with the shells.

use crate::save::{Reader, Writer};
use crate::{Button, Input, Rng};

/// Save format version.
pub const VERSION: u8 = 1;
/// Columns of the widest world.
pub const MAX_W: usize = 384;
/// World widths (px) of the width setting.
pub const WIDTHS: [u16; 4] = [128, 192, 256, 384];
/// The screen's width (px): the view.
pub const VIEW_W: i16 = 128;
/// World height (px); y grows upwards from the bottom.
pub const WORLD_H: i16 = 128;
/// Highest ground (px): room above it for the info line.
pub const TOP: u8 = 104;
pub const MAX_PLAYERS: usize = 4;
/// Metres per pixel.
pub const M_PER_PX: f32 = 4.0;
/// Physics steps per second of game time and the step (s).
pub const PHYS_HZ: u32 = 200;
pub const DT: f32 = 1.0 / 200.0;
/// Physics steps per 60 Hz logic step: 0.1 s of game time, so game time
/// runs 6× faster than real time.
pub const SUB: u32 = 20;
/// OK during a flight: this many times faster still.
pub const FAST: u32 = 4;
/// Longest flight (physics steps, 180 s of game time).
pub const MAX_FLIGHT: u32 = 36_000;
/// Gravity presets (m/s²): Moon, Mars, Earth, Jupiter.
pub const GRAVITY: [f32; 4] = [1.62, 3.71, 9.81, 24.8];
/// Air density (kg/m³) of the drag settings: off, realistic (sea level),
/// heavy (three times that).
pub const AIR: [f32; 3] = [0.0, 1.225, 3.675];
/// Drag coefficient of every shell.
pub const CD: f32 = 0.4;
/// Tank half width (px) and body height (px).
pub const TANK_HALF: i16 = 3;
pub const TANK_H: i16 = 4;
/// Barrel length (px) from the turret's pivot.
pub const BARREL: f32 = 5.0;
/// Free fall without damage (px); each pixel more costs [`FALL_HURT`].
pub const FALL_FREE: i16 = 4;
pub const FALL_HURT: i16 = 2;
/// Logic steps a blast is shown.
pub const BLAST_STEPS: u8 = 18;
/// Smoke puffs at most, and their life (logic steps).
pub const PUFFS: usize = 3;
pub const PUFF_LIFE: u16 = 720;
/// Shells in the air at most (a cluster's three bomblets).
pub const SHELLS: usize = 3;
/// Power is per mille of the round's top speed.
pub const POWER_MAX: u16 = 1000;
/// Logic steps a computer player at least seems to think.
const THINK_MIN: u16 = 30;
/// Credits every player gets per round, and for winning one.
pub const ROUND_CREDITS: u16 = 30;
pub const WIN_POINTS: i16 = 100;
pub const KILL_POINTS: i16 = 50;

// ------------------------------------------------------------- numbers

/// sin of 0 ..= 90 whole degrees (f32, rounded from the exact values).
const SIN: [f32; 91] = [
    0.0,
    0.017452406,
    0.034899496,
    0.052335955,
    0.06975647,
    0.087155744,
    0.104528464,
    0.12186934,
    0.1391731,
    0.15643446,
    0.17364818,
    0.190809,
    0.20791169,
    0.22495106,
    0.2419219,
    0.25881904,
    0.27563736,
    0.2923717,
    0.309017,
    0.32556817,
    0.34202015,
    0.35836795,
    0.37460658,
    0.39073113,
    0.40673664,
    0.42261827,
    0.43837115,
    0.4539905,
    0.46947157,
    0.4848096,
    0.5,
    0.5150381,
    0.52991927,
    0.54463905,
    0.5591929,
    0.57357645,
    0.58778524,
    0.60181504,
    0.6156615,
    0.6293204,
    0.64278764,
    0.656059,
    0.6691306,
    0.6819984,
    0.6946584,
    0.70710677,
    0.7193398,
    0.7313537,
    0.7431448,
    0.7547096,
    0.76604444,
    0.777146,
    0.7880108,
    0.7986355,
    0.809017,
    0.81915206,
    0.82903755,
    0.83867055,
    0.8480481,
    0.8571673,
    0.8660254,
    0.8746197,
    0.88294756,
    0.8910065,
    0.89879405,
    0.9063078,
    0.9135454,
    0.92050487,
    0.92718387,
    0.9335804,
    0.9396926,
    0.94551855,
    0.95105654,
    0.9563047,
    0.9612617,
    0.9659258,
    0.9702957,
    0.97437006,
    0.9781476,
    0.98162717,
    0.9848077,
    0.98768836,
    0.99026805,
    0.99254614,
    0.9945219,
    0.9961947,
    0.9975641,
    0.9986295,
    0.99939084,
    0.9998477,
    1.0,
];

/// sin of `deg` (0 ..= 180).
#[must_use]
pub fn sin_deg(deg: u8) -> f32 {
    let a = usize::from(deg.min(180));
    if a <= 90 { SIN[a] } else { SIN[180 - a] }
}

/// cos of `deg` (0 ..= 180).
#[must_use]
pub fn cos_deg(deg: u8) -> f32 {
    let a = usize::from(deg.min(180));
    if a <= 90 { SIN[90 - a] } else { -SIN[a - 90] }
}

/// Square root by four Newton steps from a halved exponent: only
/// IEEE-754 `+ ÷ ×`, so every target computes the same bits. Within an
/// ulp or two of the true root; 0 for zero, negative numbers and NaN.
#[must_use]
pub fn sqrt(x: f32) -> f32 {
    if x.is_nan() || x <= 0.0 {
        return 0.0;
    }
    if x > f32::MAX {
        return x;
    }
    let mut y = f32::from_bits((x.to_bits() >> 1) + 0x1FC0_0000);
    for _ in 0..4 {
        y = 0.5 * (y + x / y);
    }
    y
}

fn fabs(x: f32) -> f32 {
    if x < 0.0 { -x } else { x }
}

/// Integer square root (floor).
#[must_use]
pub fn isqrt(v: u32) -> u32 {
    if v < 2 {
        return v;
    }
    let mut x = v;
    let mut y = (x >> 1) + (x & 1);
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x
}

/// `f32` compared by bits (game state is `Eq`).
fn same(a: f32, b: f32) -> bool {
    a.to_bits() == b.to_bits()
}

// ------------------------------------------------------------- weapons

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Weapon {
    Shell = 0,
    Heavy,
    Cluster,
    Roller,
    Digger,
    Dirt,
    Smoke,
}

pub const WEAPONS: usize = 7;
/// A cluster's bomblet (`Shell::kind`, not a weapon of its own).
pub const BOMBLET: u8 = 7;

impl Weapon {
    pub const ALL: [Weapon; WEAPONS] = [
        Weapon::Shell,
        Weapon::Heavy,
        Weapon::Cluster,
        Weapon::Roller,
        Weapon::Digger,
        Weapon::Dirt,
        Weapon::Smoke,
    ];

    #[must_use]
    pub fn from_u8(v: u8) -> Option<Weapon> {
        Weapon::ALL.get(usize::from(v)).copied()
    }
}

/// What a weapon is: mass (kg) and diameter (m) for the drag, its blast
/// (radius px, damage at the centre), its price and how many each player
/// starts with in the full set.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Spec {
    pub mass: f32,
    pub diam: f32,
    /// `½·Cd·A/m` (m²/kg); times the air density, the drag factor.
    pub k: f32,
    pub radius: u8,
    pub damage: u8,
    pub price: u16,
    pub start: u8,
}

const fn spec(mass: f32, diam: f32, radius: u8, damage: u8, price: u16, start: u8) -> Spec {
    let area = core::f32::consts::PI / 4.0 * diam * diam;
    Spec { mass, diam, k: 0.5 * CD * area / mass, radius, damage, price, start }
}

/// Shell, heavy shell, cluster, roller, digger, dirt bomb, smoke; then the
/// cluster's bomblet ([`BOMBLET`]).
pub const SPECS: [Spec; WEAPONS + 1] = [
    spec(10.0, 0.114, 10, 40, 0, 0),
    spec(30.0, 0.155, 15, 65, 40, 3),
    spec(12.0, 0.14, 7, 24, 60, 2),
    spec(10.0, 0.12, 11, 45, 30, 2),
    spec(12.0, 0.10, 6, 22, 25, 2),
    spec(10.0, 0.13, 14, 0, 20, 2),
    spec(3.0, 0.10, 0, 0, 10, 3),
    spec(1.5, 0.06, 7, 24, 0, 0),
];

/// A cluster's bomblets fly apart at these horizontal speeds (m/s).
const SPLIT_DV: [f32; SHELLS] = [-18.0, 0.0, 18.0];
/// A digger's trench: length and half width (px).
const DIG_LEN: i32 = 24;
const DIG_R: i32 = 2;

/// Top speed (m/s) of a shot at full power per gravity × drag setting:
/// a 45° shell from flat ground in still air goes at least 1.25 times the
/// widest world (`tests/salvo.rs::top_speed_reaches`).
pub const V_MAX: [[f32; 3]; 4] =
    [[57.0, 70.0, 119.0], [86.0, 106.0, 180.0], [140.0, 172.0, 292.0], [223.0, 274.0, 465.0]];

// ------------------------------------------------------------- settings

/// Who plays a tank.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Kind {
    #[default]
    Human = 0,
    Novice,
    Easy,
    Hard,
    Expert,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Human, Kind::Novice, Kind::Easy, Kind::Hard, Kind::Expert];

    #[must_use]
    pub fn from_u8(v: u8) -> Option<Kind> {
        Kind::ALL.get(usize::from(v)).copied()
    }

    #[must_use]
    pub fn is_computer(self) -> bool {
        self != Kind::Human
    }

    /// Difficulty index 0 ..= 3 of a computer player.
    fn level(self) -> usize {
        (self as usize).saturating_sub(1).min(3)
    }
}

/// Terrain styles; `Mixed` picks one per round.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Style {
    #[default]
    Hills = 0,
    Mountains,
    Canyon,
    Islands,
    Plateaus,
    Mixed,
}

impl Style {
    pub const ALL: [Style; 6] =
        [Style::Hills, Style::Mountains, Style::Canyon, Style::Islands, Style::Plateaus, Style::Mixed];

    #[must_use]
    pub fn from_u8(v: u8) -> Option<Style> {
        Style::ALL.get(usize::from(v)).copied()
    }
}

/// Rule settings, fixed for a game.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Config {
    /// 2 ..= 4.
    pub players: u8,
    pub kinds: [Kind; MAX_PLAYERS],
    /// 1 ..= 10.
    pub rounds: u8,
    /// Index into [`GRAVITY`].
    pub gravity: u8,
    /// Largest wind (m/s, 0 = calm), 0 ..= 30.
    pub wind: u8,
    pub gusts: bool,
    /// Index into [`AIR`].
    pub drag: u8,
    pub style: Style,
    /// 0 ..= 2: smooth, medium, rough.
    pub rough: u8,
    /// 0 ..= 2: low, medium, high relief.
    pub relief: u8,
    /// 0: a new terrain every game; else the same terrains every game.
    pub terrain_seed: u16,
    /// Index into [`WIDTHS`].
    pub width: u8,
    /// Tank health, 25 ..= 250.
    pub health: u8,
    pub fall_damage: bool,
    /// All seven weapons (else the shell only, unless bought).
    pub full_set: bool,
    pub shop: bool,
    /// Seconds per human turn (0 = no limit); when it runs out the tank
    /// fires as aimed.
    pub timer: u8,
}

impl Config {
    /// What suits a first game: you against an easy computer, three
    /// rounds of rolling hills 256 px wide on Earth, light wind, the full
    /// set with the shop.
    pub const DEFAULT: Config = Config {
        players: 2,
        kinds: [Kind::Human, Kind::Easy, Kind::Easy, Kind::Easy],
        rounds: 3,
        gravity: 2,
        wind: 4,
        gusts: false,
        drag: 1,
        style: Style::Hills,
        rough: 1,
        relief: 1,
        terrain_seed: 0,
        width: 2,
        health: 100,
        fall_damage: true,
        full_set: true,
        shop: true,
        timer: 0,
    };

    /// Values out of range replaced by the defaults.
    #[must_use]
    pub fn sanitised(self) -> Config {
        let d = Config::DEFAULT;
        Config {
            players: if (2..=4).contains(&self.players) { self.players } else { d.players },
            kinds: self.kinds,
            rounds: if (1..=10).contains(&self.rounds) { self.rounds } else { d.rounds },
            gravity: if usize::from(self.gravity) < GRAVITY.len() { self.gravity } else { d.gravity },
            wind: self.wind.min(30),
            gusts: self.gusts,
            drag: if usize::from(self.drag) < AIR.len() { self.drag } else { d.drag },
            style: self.style,
            rough: self.rough.min(2),
            relief: self.relief.min(2),
            terrain_seed: self.terrain_seed,
            width: if usize::from(self.width) < WIDTHS.len() { self.width } else { d.width },
            health: if (25..=250).contains(&self.health) { self.health } else { d.health },
            fall_damage: self.fall_damage,
            full_set: self.full_set,
            shop: self.shop,
            timer: self.timer.min(120),
        }
    }

    #[must_use]
    pub fn world_w(&self) -> u16 {
        WIDTHS[usize::from(self.width.min(3))]
    }

    #[must_use]
    pub fn humans(&self) -> usize {
        self.kinds[..usize::from(self.players)].iter().filter(|k| !k.is_computer()).count()
    }

    /// The score table's variant: players and opponents, world, physics
    /// and weapon rules.
    #[must_use]
    pub fn variant(&self) -> u16 {
        let hardest = self.kinds[..usize::from(self.players)].iter().map(|k| *k as u16).max().unwrap_or(0);
        let wind = match self.wind {
            0 => 0,
            1..=5 => 1,
            6..=12 => 2,
            _ => 3,
        };
        u16::from(self.players - 2)
            | u16::from(self.gravity & 3) << 2
            | wind << 4
            | u16::from(self.drag & 3) << 6
            | u16::from(self.width & 3) << 8
            | u16::from(self.full_set) << 10
            | u16::from(self.shop) << 11
            | (hardest.min(4) & 7) << 12
            | u16::from(self.gusts) << 15
    }

    fn encode(&self, w: &mut Writer<'_>) {
        w.u8(self.players);
        for k in self.kinds {
            w.u8(k as u8);
        }
        for v in [self.rounds, self.gravity, self.wind, self.drag, self.style as u8, self.rough, self.relief]
        {
            w.u8(v);
        }
        w.u16(self.terrain_seed);
        w.u8(self.width);
        w.u8(self.health);
        w.u8(u8::from(self.gusts)
            | u8::from(self.fall_damage) << 1
            | u8::from(self.full_set) << 2
            | u8::from(self.shop) << 3);
        w.u8(self.timer);
    }

    fn decode(r: &mut Reader<'_>) -> Option<Config> {
        let players = r.u8()?;
        let mut kinds = [Kind::Human; MAX_PLAYERS];
        for k in &mut kinds {
            *k = Kind::from_u8(r.u8()?)?;
        }
        let [rounds, gravity, wind, drag, style, rough, relief] = r.array::<7>()?;
        let terrain_seed = r.u16()?;
        let width = r.u8()?;
        let health = r.u8()?;
        let flags = r.u8()?;
        let timer = r.u8()?;
        let c = Config {
            players,
            kinds,
            rounds,
            gravity,
            wind,
            gusts: flags & 1 != 0,
            drag,
            style: Style::from_u8(style)?,
            rough,
            relief,
            terrain_seed,
            width,
            health,
            fall_damage: flags & 2 != 0,
            full_set: flags & 4 != 0,
            shop: flags & 8 != 0,
            timer,
        };
        (c.sanitised() == c).then_some(c)
    }
}

impl Default for Config {
    fn default() -> Self {
        Config::DEFAULT
    }
}

// ------------------------------------------------------------- state

/// A tank and its player's standing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Tank {
    /// Centre column.
    pub x: i16,
    /// Bottom, 1/16 px.
    pub y: i16,
    /// Falling speed, 1/16 px per logic step.
    pub vy: i8,
    /// Logic steps until it may slide another pixel.
    pub slide: u8,
    /// Barrel angle, degrees anticlockwise from the right (0 ..= 180).
    pub angle: u8,
    /// Per mille of the round's top speed.
    pub power: u16,
    /// 0: destroyed.
    pub health: u8,
    pub weapon: u8,
    /// Rounds of the weapons after the shell (unlimited).
    pub ammo: [u8; WEAPONS - 1],
    pub score: i16,
    pub credits: u16,
    /// Computer: the tank last aimed at, the aim-point correction (px)
    /// learnt from where that shot landed, the wind-estimate bias of this
    /// round (0.25 m/s).
    pub ai_target: u8,
    pub ai_off: i8,
    pub ai_bias: i8,
    /// The last shot (aim guide): angle, power, weapon (0xFF none), the
    /// wind then (0.5 m/s).
    pub last_angle: u8,
    pub last_power: u16,
    pub last_weapon: u8,
    pub last_wind: i8,
}

impl Tank {
    /// Rounds of weapon `w` (the shell: 255, unlimited).
    #[must_use]
    pub fn rounds(&self, w: u8) -> u8 {
        match usize::from(w) {
            0 => u8::MAX,
            k => self.ammo.get(k - 1).copied().unwrap_or(0),
        }
    }

    fn rounds_mut(&mut self, w: u8) -> Option<&mut u8> {
        usize::from(w).checked_sub(1).and_then(|k| self.ammo.get_mut(k))
    }

    #[must_use]
    pub fn alive(&self) -> bool {
        self.health > 0
    }

    /// The bottom in whole pixels.
    #[must_use]
    pub fn y_px(&self) -> i16 {
        self.y >> 4
    }

    #[must_use]
    pub fn facing_right(&self) -> bool {
        self.angle <= 90
    }
}

/// A shell in the air (or rolling): position (m), velocity (m/s).
#[derive(Clone, Copy, Debug, Default)]
pub struct Shell {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    /// Weapon, or [`BOMBLET`].
    pub kind: u8,
    pub mode: Mode,
    /// Physics steps in its mode.
    pub t: u16,
}

impl PartialEq for Shell {
    fn eq(&self, o: &Shell) -> bool {
        same(self.x, o.x)
            && same(self.y, o.y)
            && same(self.vx, o.vx)
            && same(self.vy, o.vy)
            && (self.kind, self.mode, self.t) == (o.kind, o.mode, o.t)
    }
}

impl Eq for Shell {}

impl Shell {
    #[must_use]
    pub fn active(&self) -> bool {
        self.mode != Mode::Off
    }

    /// Position in pixels.
    #[must_use]
    pub fn px(&self) -> (f32, f32) {
        (self.x / M_PER_PX, self.y / M_PER_PX)
    }

    fn finite(&self) -> bool {
        [self.x, self.y, self.vx, self.vy].iter().all(|v| v.is_finite())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Mode {
    #[default]
    Off = 0,
    Fly,
    Roll,
}

/// A smoke puff: position (1/64 px across, 1/16 px up) and age.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Puff {
    pub x: i16,
    pub y: i16,
    /// Logic steps since it was made; 0: none.
    pub age: u16,
}

impl Puff {
    #[must_use]
    pub fn px(&self) -> (i16, i16) {
        (self.x >> 6, self.y >> 4)
    }
}

/// The last explosion (drawing): centre and radius (px), age (steps).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Blast {
    pub x: i16,
    pub y: i16,
    pub r: u8,
    pub age: u8,
}

impl Blast {
    const NONE: Blast = Blast { x: 0, y: 0, r: 0, age: BLAST_STEPS };

    #[must_use]
    pub fn shown(&self) -> bool {
        self.age < BLAST_STEPS
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Phase {
    /// "Pass to Player n": waiting for OK.
    #[default]
    Pass = 0,
    /// A human aims.
    Aim,
    /// A computer searches for its shot.
    Think,
    /// A computer turns its barrel to the shot it chose.
    Turn,
    /// Shells in the air.
    Flight,
    /// Tanks falling and sliding, the last blast fading.
    Settle,
    /// The round is over: waiting for OK.
    RoundOver,
    /// Between rounds: the shop.
    Shop,
    /// The game is over.
    Over,
}

impl Phase {
    const ALL: [Phase; 9] = [
        Phase::Pass,
        Phase::Aim,
        Phase::Think,
        Phase::Turn,
        Phase::Flight,
        Phase::Settle,
        Phase::RoundOver,
        Phase::Shop,
        Phase::Over,
    ];
}

/// A computer player's search: the candidate angle, the power bracket,
/// the best shot so far, the estimate of the wind.
#[derive(Clone, Copy, Debug, Default)]
struct Search {
    target: u8,
    cand: u8,
    iter: u8,
    sims: u8,
    lo: u16,
    hi: u16,
    best_angle: u8,
    best_power: u16,
    /// |miss| of the best (px × 4).
    best_err: u16,
    /// Where it aims (px).
    aim_x: i16,
    wind: f32,
    done: bool,
    weapon: u8,
}

impl PartialEq for Search {
    fn eq(&self, o: &Search) -> bool {
        same(self.wind, o.wind)
            && (self.target, self.cand, self.iter, self.sims, self.lo, self.hi, self.best_angle)
                == (o.target, o.cand, o.iter, o.sims, o.lo, o.hi, o.best_angle)
            && (self.best_power, self.best_err, self.aim_x, self.done, self.weapon)
                == (o.best_power, o.best_err, o.aim_x, o.done, o.weapon)
    }
}

impl Eq for Search {}

/// Candidate elevations (degrees) of the search, in order.
const ANGLES: [u8; 6] = [45, 60, 30, 72, 20, 80];
/// Per difficulty (Novice … Expert): angles tried, bisection steps per
/// angle, wind noise and bias (per mille of the set maximum), aim noise
/// (degrees, per mille of power), share of the last miss corrected
/// (tenths).
const AI_ANGLES: [u8; 4] = [2, 3, 4, 6];
const AI_ITERS: [u8; 4] = [4, 6, 8, 11];
const AI_WIND_NOISE: [i32; 4] = [600, 300, 120, 0];
const AI_WIND_BIAS: [i32; 4] = [400, 200, 80, 0];
const AI_ANGLE_NOISE: [u32; 4] = [3, 2, 1, 0];
const AI_POWER_NOISE: [u32; 4] = [30, 15, 6, 0];
const AI_GAIN: [i32; 4] = [4, 6, 8, 10];

/// Where a shell ended.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hit {
    /// The ground (or the water's surface) at (px, px).
    Ground(f32, f32),
    /// Tank `i`, at (px, px).
    Tank(u8, f32, f32),
    /// Off the side of the world, or still in the air after
    /// [`MAX_FLIGHT`] steps.
    Out,
}

/// A broken invariant ([`Salvo::check`]). An enum, not text: the
/// firmware keeps string constants resident.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Broken {
    TerrainAboveTop,
    TerrainBeyondWorld,
    TurnOfNoPlayer,
    HealthAboveSetting,
    TankOutsideWorld,
    AimOutOfRange,
    TankInGround,
    ShellNotFinite,
    ShellOutsideFlight,
    ShellInGround,
    WindNotFinite,
    ViewOutsideWorld,
}

/// What a logic step or a key did, for the app (sounds, redraws).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SalvoOut {
    pub fired: bool,
    pub exploded: bool,
    pub ended: bool,
}

/// The game.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Salvo {
    cfg: Config,
    rng: Rng,
    heights: [u8; MAX_W],
    /// Water level (px), 0 = none.
    water: u8,
    tanks: [Tank; MAX_PLAYERS],
    shells: [Shell; SHELLS],
    puffs: [Puff; PUFFS],
    blast: Blast,
    /// Wind of this turn (0.1 m/s).
    wind: i16,
    /// Gust on top of it now (m/s, bits of an `f32`).
    gust: u32,
    phase: Phase,
    turn: u8,
    round: u8,
    /// Logic steps in the phase.
    t: u16,
    /// Physics steps of the flight (at most [`MAX_FLIGHT`]).
    flight: u16,
    /// Human turn: logic steps left (timer setting).
    timer: u16,
    /// Held direction button and the steps it has been held.
    held: Option<Button>,
    held_t: u16,
    ok_down: bool,
    fast: bool,
    overview: bool,
    /// The view's left edge (px) and whether it follows the action.
    cam: i16,
    follow: bool,
    /// The weapon list: open with its cursor.
    menu: Option<u8>,
    /// Shop: whose turn and the row.
    shop_player: u8,
    shop_row: u8,
    /// Where the shot's first shell landed (px), for the computer's
    /// correction.
    impact_x: i16,
    search: Search,
}

/// Logic steps per second.
const HZ: u16 = crate::STEP_HZ as u16;

impl Salvo {
    /// A new game: the first round's terrain, every tank ready.
    #[must_use]
    pub fn new(cfg: Config, seed: u64) -> Salvo {
        let mut s = Salvo::blank(cfg.sanitised(), seed);
        s.start_game();
        s
    }

    /// No world yet (to be filled in place: [`Salvo::start_game`]).
    #[must_use]
    pub fn blank(cfg: Config, seed: u64) -> Salvo {
        Salvo {
            cfg: cfg.sanitised(),
            rng: Rng::new(seed),
            heights: [0; MAX_W],
            water: 0,
            tanks: [Tank::default(); MAX_PLAYERS],
            shells: [Shell::default(); SHELLS],
            puffs: [Puff::default(); PUFFS],
            blast: Blast::NONE,
            wind: 0,
            gust: 0,
            phase: Phase::Pass,
            turn: 0,
            round: 0,
            t: 0,
            flight: 0,
            timer: 0,
            held: None,
            held_t: 0,
            ok_down: false,
            fast: false,
            overview: false,
            cam: 0,
            follow: true,
            menu: None,
            shop_player: 0,
            shop_row: 1,
            impact_x: 0,
            search: Search::default(),
        }
    }

    /// Back to [`Salvo::blank`] in place, without a copy of the game on
    /// the stack.
    pub fn reset(&mut self, cfg: Config, seed: u64) {
        self.cfg = cfg.sanitised();
        self.rng = Rng::new(seed);
        self.heights.fill(0);
        self.water = 0;
        self.tanks.fill(Tank::default());
        self.shells.fill(Shell::default());
        self.puffs.fill(Puff::default());
        self.blast = Blast::NONE;
        self.wind = 0;
        self.gust = 0;
        self.phase = Phase::Pass;
        self.turn = 0;
        self.round = 0;
        self.t = 0;
        self.flight = 0;
        self.timer = 0;
        self.held = None;
        self.held_t = 0;
        self.ok_down = false;
        self.fast = false;
        self.overview = false;
        self.cam = 0;
        self.follow = true;
        self.menu = None;
        self.shop_player = 0;
        self.shop_row = 1;
        self.impact_x = 0;
        self.search = Search::default();
    }

    /// A new game in place: [`Salvo::reset`] and [`Salvo::start_game`].
    pub fn restart(&mut self, cfg: Config, seed: u64) {
        self.reset(cfg, seed);
        self.start_game();
    }

    /// Start the game in place (after [`Salvo::blank`]).
    pub fn start_game(&mut self) {
        for t in &mut self.tanks {
            *t = Tank { last_weapon: 0xFF, ..Tank::default() };
            if self.cfg.full_set {
                for (a, s) in t.ammo.iter_mut().zip(SPECS.iter().skip(1)) {
                    *a = s.start;
                }
            }
        }
        self.start_round(0);
    }

    // --------------------------------------------------------- queries

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    #[must_use]
    pub fn width(&self) -> u16 {
        self.cfg.world_w()
    }

    /// The ground's height (px) of column `x` (0 outside the world).
    #[must_use]
    pub fn height(&self, x: i32) -> u8 {
        usize::try_from(x).ok().filter(|&x| x < usize::from(self.width())).map_or(0, |x| self.heights[x])
    }

    #[must_use]
    pub fn heights(&self) -> &[u8] {
        &self.heights[..usize::from(self.width())]
    }

    #[must_use]
    pub fn water(&self) -> u8 {
        self.water
    }

    #[must_use]
    pub fn tanks(&self) -> &[Tank] {
        &self.tanks[..usize::from(self.cfg.players)]
    }

    #[must_use]
    pub fn shells(&self) -> &[Shell; SHELLS] {
        &self.shells
    }

    #[must_use]
    pub fn puffs(&self) -> &[Puff; PUFFS] {
        &self.puffs
    }

    #[must_use]
    pub fn blast(&self) -> Blast {
        self.blast
    }

    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The player whose turn it is (whose shop, in the shop).
    #[must_use]
    pub fn turn(&self) -> u8 {
        if self.phase == Phase::Shop { self.shop_player } else { self.turn }
    }

    /// The round (0-based).
    #[must_use]
    pub fn round(&self) -> u8 {
        self.round
    }

    /// The wind of this turn (m/s, positive to the right) and the gust on
    /// top of it now.
    #[must_use]
    pub fn wind(&self) -> (f32, f32) {
        (f32::from(self.wind) / 10.0, f32::from_bits(self.gust))
    }

    /// The wind in tenths of m/s (display).
    #[must_use]
    pub fn wind_tenths(&self) -> i16 {
        self.wind
    }

    /// Seconds left on the turn timer (None: no timer or not a human turn).
    #[must_use]
    pub fn timer_left(&self) -> Option<u16> {
        (self.cfg.timer > 0 && self.phase == Phase::Aim).then(|| self.timer.div_ceil(HZ))
    }

    #[must_use]
    pub fn fast(&self) -> bool {
        self.fast
    }

    #[must_use]
    pub fn overview(&self) -> bool {
        self.overview
    }

    /// The view's left edge (px).
    #[must_use]
    pub fn camera(&self) -> i16 {
        self.cam
    }

    /// The weapon list's cursor while it is open.
    #[must_use]
    pub fn menu(&self) -> Option<u8> {
        self.menu
    }

    /// The shop's row (weapons 1 ..= 6, then 7 = done).
    #[must_use]
    pub fn shop_row(&self) -> u8 {
        self.shop_row
    }

    /// The top speed of a shot (m/s) under this game's gravity and air.
    #[must_use]
    pub fn v_max(&self) -> f32 {
        V_MAX[usize::from(self.cfg.gravity.min(3))][usize::from(self.cfg.drag.min(2))]
    }

    #[must_use]
    pub fn ended(&self) -> bool {
        self.phase == Phase::Over
    }

    /// The player (or players) with the most points.
    #[must_use]
    pub fn leader(&self) -> Option<u8> {
        let ts = self.tanks();
        let best = ts.iter().map(|t| t.score).max()?;
        let mut who = ts.iter().enumerate().filter(|(_, t)| t.score == best);
        let first = who.next()?.0;
        who.next().is_none().then(|| u8::try_from(first).unwrap_or(0))
    }

    /// The survivor of the round just over (None: none or a draw).
    #[must_use]
    pub fn round_winner(&self) -> Option<u8> {
        let mut alive = self.tanks().iter().enumerate().filter(|(_, t)| t.alive());
        let first = alive.next()?.0;
        alive.next().is_none().then(|| u8::try_from(first).unwrap_or(0))
    }

    /// The best human score and whether a human leads (for the score
    /// table); `None` without humans.
    #[must_use]
    pub fn human_result(&self) -> Option<(i16, bool)> {
        let n = usize::from(self.cfg.players);
        let best = (0..n).filter(|&i| !self.cfg.kinds[i].is_computer()).map(|i| self.tanks[i].score).max()?;
        let lead = self.leader().is_some_and(|p| !self.cfg.kinds[usize::from(p)].is_computer());
        Some((best, lead))
    }

    /// The pivot of tank `i`'s barrel (px).
    #[must_use]
    pub fn pivot(&self, i: usize) -> (f32, f32) {
        let t = &self.tanks[i];
        (f32::from(t.x) + 0.5, f32::from(t.y) / 16.0 + f32::from(TANK_H) - 1.0)
    }

    /// The shell of tank `i` leaving the barrel at `angle` and `power`.
    #[must_use]
    pub fn launch(&self, i: usize, angle: u8, power: u16, kind: u8) -> Shell {
        let (px, py) = self.pivot(i);
        let (c, s) = (cos_deg(angle), sin_deg(angle));
        let v = self.v_max() * f32::from(power.min(POWER_MAX)) / 1000.0;
        Shell {
            x: (px + c * BARREL) * M_PER_PX,
            y: (py + s * BARREL) * M_PER_PX,
            vx: v * c,
            vy: v * s,
            kind,
            mode: Mode::Fly,
            t: 0,
        }
    }

    /// The weapons tank `i` can fire.
    #[must_use]
    pub fn available(&self, i: usize, w: u8) -> bool {
        usize::from(w) < WEAPONS && self.tanks.get(i).is_some_and(|t| t.rounds(w) > 0)
    }

    // --------------------------------------------------------- physics

    fn g(&self) -> f32 {
        GRAVITY[usize::from(self.cfg.gravity.min(3))]
    }

    fn rho(&self) -> f32 {
        AIR[usize::from(self.cfg.drag.min(2))]
    }

    /// One physics step of a flying shell in air moving at `wind` (m/s).
    pub fn advance(s: &mut Shell, g: f32, k: f32, wind: f32) {
        let rx = s.vx - wind;
        let ry = s.vy;
        let d = k * sqrt(rx * rx + ry * ry);
        let ax = -(d * rx);
        let ay = -g - d * ry;
        s.vx += ax * DT;
        s.vy += ay * DT;
        s.x += s.vx * DT;
        s.y += s.vy * DT;
    }

    /// The surface a shell stops at in column `col` (ground or water).
    fn surface(&self, col: i32) -> f32 {
        f32::from(self.height(col).max(self.water))
    }

    /// Does a shell at (px, py) hit something? `own`: the firing tank,
    /// not hit while `grace` (its shell leaving the barrel).
    fn hit_at(&self, px: f32, py: f32, own: usize, grace: bool) -> Option<Hit> {
        let w = f32::from(self.width());
        if !(px >= 0.0 && px < w) {
            return Some(Hit::Out);
        }
        for (i, t) in self.tanks().iter().enumerate() {
            if !t.alive() || (grace && i == own) {
                continue;
            }
            let dx = px - (f32::from(t.x) + 0.5);
            let by = f32::from(t.y) / 16.0;
            if fabs(dx) <= f32::from(TANK_HALF) + 0.5 && py >= by && py <= by + f32::from(TANK_H) + 1.0 {
                return Some(Hit::Tank(u8::try_from(i).unwrap_or(0), px, py));
            }
        }
        let col = px as i32;
        if py < self.surface(col) {
            return Some(Hit::Ground(px, py));
        }
        None
    }

    /// The segment a shell moved along in one step, probed at most every
    /// half pixel.
    fn probe(&self, from: (f32, f32), s: &Shell, own: usize, grace: bool) -> Option<Hit> {
        let (x1, y1) = s.px();
        let (dx, dy) = (x1 - from.0, y1 - from.1);
        let len = fabs(dx).max(fabs(dy));
        let n = if len > 0.5 { (len * 2.0) as u32 + 1 } else { 1 }.min(64);
        for k in 1..=n {
            let f = k as f32 / n as f32;
            if let Some(h) = self.hit_at(from.0 + dx * f, from.1 + dy * f, own, grace) {
                return Some(h);
            }
        }
        None
    }

    /// The gust after one more [`SUB`]-step period.
    fn next_gust(gust: f32, rng: &mut Rng, max: u8) -> f32 {
        let u = rng.below(2001) as f32 / 1000.0 - 1.0;
        let m = f32::from(max.max(2));
        let g = gust * 0.9 + u * 0.4 * m;
        g.clamp(-m, m)
    }

    /// Simulate a shot to its end, as [`Salvo::step`] would fly it, with a
    /// fixed wind (`gusts`: the generator the flight will draw them from).
    /// `f` sees the shell after every step. Clusters fly as one shell.
    pub fn simulate(
        &self,
        mut s: Shell,
        own: usize,
        wind: f32,
        mut gusts: Option<Rng>,
        mut f: impl FnMut(u32, &Shell) -> bool,
    ) -> Hit {
        let (g, k) = (self.g(), self.rho() * SPECS[usize::from(s.kind.min(7))].k);
        if let Some(h) = self.muzzle(own, &s) {
            return h;
        }
        let mut gust = 0.0f32;
        for n in 0..MAX_FLIGHT {
            if n % SUB == 0
                && let Some(r) = gusts.as_mut()
            {
                gust = Salvo::next_gust(gust, r, self.cfg.wind);
            }
            let from = s.px();
            Salvo::advance(&mut s, g, k, wind + gust);
            if let Some(h) = self.probe(from, &s, own, n < 40) {
                return h;
            }
            if !f(n, &s) {
                return Hit::Out;
            }
        }
        Hit::Out
    }

    // --------------------------------------------------------- terrain

    /// A value from the terrain seed for lattice point `i` of octave `oct`
    /// (0 ..= 1000).
    fn lattice(seed: u64, oct: u32, i: u32) -> i32 {
        let mut r = Rng::new(seed ^ u64::from(oct) << 40 ^ u64::from(i).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        (r.next_u32() % 1001) as i32
    }

    /// Smooth value noise of one octave at `x` (0 ..= 1000).
    fn value(seed: u64, oct: u32, x: u32, period: u32) -> i32 {
        let p = period.max(2);
        let i = x / p;
        let t = ((x % p) * 1000 / p) as i32;
        let s = t * t / 1000 * (3000 - 2 * t) / 1000;
        let (a, b) = (Salvo::lattice(seed, oct, i), Salvo::lattice(seed, oct, i + 1));
        a + (b - a) * s / 1000
    }

    /// Octaves of value noise, each half the period of the last, their
    /// weight `pers`/1000 of the last's (0 ..= 1000).
    fn fbm(seed: u64, x: u32, period: u32, octaves: u32, pers: i32) -> i32 {
        let (mut sum, mut total, mut amp, mut p) = (0i32, 0i32, 1000i32, period);
        for o in 0..octaves {
            sum += Salvo::value(seed, o, x, p) * amp / 1000;
            total += amp;
            amp = amp * pers / 1000;
            p = (p / 2).max(4);
        }
        if total == 0 { 500 } else { sum * 1000 / total }
    }

    /// The round's terrain and the tanks on it.
    fn make_world(&mut self) {
        let w = u32::from(self.width());
        let n = u32::from(self.cfg.players);
        let seed = if self.cfg.terrain_seed == 0 {
            self.rng.next_u64()
        } else {
            Rng::seed_from(u64::from(self.cfg.terrain_seed), u64::from(self.round))
        };
        let mut trng = Rng::new(seed);
        let style = match self.cfg.style {
            Style::Mixed => Style::from_u8(trng.below(5) as u8).unwrap_or_default(),
            s => s,
        };
        let (lo, hi) = [(16i32, 48i32), (12, 72), (8, 96)][usize::from(self.cfg.relief.min(2))];
        let (oct, pers) = [(2u32, 300i32), (4, 480), (5, 640)][usize::from(self.cfg.rough.min(2))];
        // Tank slots, players in a random order.
        let mut order = [0u8, 1, 2, 3];
        for i in (1..n as usize).rev() {
            order.swap(i, trng.below(i as u32 + 1) as usize);
        }
        let mut slots = [0i32; MAX_PLAYERS];
        for (i, s) in slots.iter_mut().enumerate().take(n as usize) {
            let span = w / (3 * n);
            let jitter = trng.below(span + 1) as i32 - (span / 2) as i32;
            *s = ((2 * i as u32 + 1) * w / (2 * n)) as i32 + jitter;
            *s = (*s).clamp(6, w as i32 - 7);
        }
        self.water = 0;
        let span = hi - lo;
        let canyon_c = (w / 2) as i32 + trng.below(w / 5 + 1) as i32 - (w / 10) as i32;
        let canyon_hw = (w / 7 + w / 20) as i32;
        let water = lo + span / 4;
        for x in 0..w {
            let h = match style {
                Style::Hills | Style::Mixed => {
                    lo + span * Salvo::fbm(seed, x, (w / 2).max(64), oct, pers) / 1000
                }
                Style::Mountains => {
                    let v = Salvo::fbm(seed, x, (w / 3).max(48), oct + 1, pers + 120);
                    let r = 1000 - (2 * v - 1000).abs();
                    lo + span * (r * r / 1000) / 1000
                }
                Style::Canyon => {
                    let top = hi - span / 6 + (Salvo::fbm(seed, x, 48, oct, pers) - 500) * span / 6000;
                    let d = (x as i32 - canyon_c).abs();
                    if d < canyon_hw {
                        let q = i64::from(d * 1000 / canyon_hw);
                        let p = 1000 - (q * q / 1000 * q / 1000 * q / 1000) as i32;
                        top - (top - lo) * p / 1000
                    } else {
                        top
                    }
                }
                Style::Islands => {
                    let half = (w / (2 * n) * 3 / 4).max(10) as i32;
                    let d = slots[..n as usize].iter().map(|&c| (x as i32 - c).abs()).min().unwrap_or(half);
                    let bed = lo / 2 + (Salvo::fbm(seed, x, 32, 2, 400) - 500) * 6 / 1000;
                    if d < half {
                        let q = d * 1000 / half;
                        let p = 1000 - q * q / 1000;
                        let peak =
                            water + (hi - water) * (400 + Salvo::fbm(seed, x, 64, oct, pers) * 6 / 10) / 1000;
                        bed + (peak - bed) * p / 1000
                    } else {
                        bed
                    }
                }
                Style::Plateaus => {
                    let v = Salvo::fbm(seed, x, (w / 3).max(48), 2, 400);
                    let level = (v * 4 / 1000).clamp(0, 3);
                    let bump =
                        (Salvo::fbm(seed ^ 1, x, 16, 2, 500) - 500) * (i32::from(self.cfg.rough) + 1) / 250;
                    lo + level * span / 3 + bump
                }
            };
            self.heights[x as usize] = u8::try_from(h.clamp(2, i32::from(TOP))).unwrap_or(TOP);
        }
        for x in w as usize..MAX_W {
            self.heights[x] = 0;
        }
        if style == Style::Islands {
            self.water = u8::try_from(water.clamp(2, i32::from(TOP) - 8)).unwrap_or(0);
        }
        // Pads under the tanks: flat, above the water.
        for i in 0..n as usize {
            let p = usize::from(order[i]);
            let x = slots[i];
            let mut h = i32::from(self.heights[x as usize]);
            if self.water > 0 {
                h = h.max(i32::from(self.water) + 3);
            }
            let h = u8::try_from(h.clamp(2, i32::from(TOP))).unwrap_or(TOP);
            for c in x - 4..=x + 4 {
                if let Some(v) =
                    usize::try_from(c).ok().and_then(|c| self.heights.get_mut(c)).filter(|_| c < w as i32)
                {
                    *v = h;
                }
            }
            let t = &mut self.tanks[p];
            t.x = i16::try_from(x).unwrap_or(0);
            t.y = i16::from(h) * 16;
            t.vy = 0;
            t.health = self.cfg.health;
            t.angle = if x < (w / 2) as i32 { 45 } else { 135 };
            t.power = 500;
            t.weapon = 0;
            t.slide = 0;
            t.ai_off = 0;
            t.ai_target = 0xFF;
            t.last_weapon = 0xFF;
        }
    }

    // --------------------------------------------------------- the turn

    fn start_round(&mut self, round: u8) {
        self.round = round;
        self.shells = [Shell::default(); SHELLS];
        self.puffs = [Puff::default(); PUFFS];
        self.blast = Blast::NONE;
        self.make_world();
        for i in 0..usize::from(self.cfg.players) {
            let k = self.cfg.kinds[i];
            let b = if k.is_computer() {
                let m = AI_WIND_BIAS[k.level()] * i32::from(self.cfg.wind) * 4 / 1000;
                self.rng.below(2 * m.unsigned_abs() + 1) as i32 - m
            } else {
                0
            };
            self.tanks[i].ai_bias = i8::try_from(b.clamp(-127, 127)).unwrap_or(0);
        }
        let first = round % self.cfg.players;
        self.begin_turn(first);
    }

    fn begin_turn(&mut self, p: u8) {
        self.turn = p;
        let max = i32::from(self.cfg.wind) * 10;
        self.wind = i16::try_from(self.rng.below((2 * max + 1) as u32) as i32 - max).unwrap_or(0);
        self.gust = 0;
        self.fast = false;
        self.follow = true;
        self.held = None;
        self.ok_down = false;
        self.menu = None;
        self.t = 0;
        self.timer = u16::from(self.cfg.timer) * HZ;
        let k = self.cfg.kinds[usize::from(p)];
        if !self.available(usize::from(p), self.tanks[usize::from(p)].weapon) {
            self.tanks[usize::from(p)].weapon = 0;
        }
        self.phase = if k.is_computer() {
            self.think_start();
            Phase::Think
        } else if self.cfg.humans() >= 2 {
            // Players sharing the player: hand it over first.
            Phase::Pass
        } else {
            Phase::Aim
        };
    }

    /// The next living player after `p`.
    fn next_alive(&self, p: u8) -> Option<u8> {
        let n = self.cfg.players;
        (1..=n).map(|d| (p + d) % n).find(|&q| self.tanks[usize::from(q)].alive())
    }

    fn fire(&mut self) -> SalvoOut {
        let i = usize::from(self.turn);
        let t = self.tanks[i];
        let w = if self.available(i, t.weapon) { t.weapon } else { 0 };
        if w != 0
            && let Some(n) = self.tanks[i].rounds_mut(w)
        {
            *n -= 1;
        }
        self.shells = [Shell::default(); SHELLS];
        self.shells[0] = self.launch(i, t.angle, t.power, w);
        let tk = &mut self.tanks[i];
        tk.last_angle = t.angle;
        tk.last_power = t.power;
        tk.last_weapon = w;
        tk.last_wind = i8::try_from((i32::from(self.wind) / 5).clamp(-127, 127)).unwrap_or(0);
        self.phase = Phase::Flight;
        self.flight = 0;
        self.t = 0;
        self.gust = 0;
        self.impact_x = i16::MIN;
        self.menu = None;
        self.held = None;
        let mut out = SalvoOut { fired: true, ..SalvoOut::default() };
        // A barrel in the ground (a tank in a pit): it goes off there.
        if let Some(h) = self.muzzle(i, &self.shells[0]) {
            self.land(0, h, &mut out);
        }
        out
    }

    /// The way from the pivot to the barrel's end, probed: what a shell
    /// leaving tank `i` meets before it flies.
    fn muzzle(&self, i: usize, s: &Shell) -> Option<Hit> {
        self.probe(self.pivot(i), s, i, true).filter(|h| !matches!(h, Hit::Out))
    }

    /// One logic step of the flight: [`SUB`] physics steps (× [`FAST`]).
    fn fly_step(&mut self, out: &mut SalvoOut) {
        let n = if self.fast { SUB * FAST } else { SUB };
        let (g, wind_max) = (self.g(), self.cfg.wind);
        let own = usize::from(self.turn);
        for _ in 0..n {
            if u32::from(self.flight) >= MAX_FLIGHT {
                self.shells = [Shell::default(); SHELLS];
            }
            if !self.shells.iter().any(Shell::active) {
                break;
            }
            if self.cfg.gusts && u32::from(self.flight) % SUB == 0 {
                let gust = Salvo::next_gust(f32::from_bits(self.gust), &mut self.rng, wind_max);
                self.gust = gust.to_bits();
            }
            let air = f32::from(self.wind) / 10.0 + f32::from_bits(self.gust);
            for k in 0..SHELLS {
                let mut s = self.shells[k];
                match s.mode {
                    Mode::Off => continue,
                    Mode::Fly => {
                        let kd = self.rho() * SPECS[usize::from(s.kind.min(7))].k;
                        let from = s.px();
                        let was_up = s.vy > 0.0;
                        Salvo::advance(&mut s, g, kd, air);
                        s.t = s.t.saturating_add(1);
                        let grace = s.t < 40 && s.kind != BOMBLET;
                        let hit = self.probe(from, &s, own, grace);
                        self.shells[k] = s;
                        if let Some(h) = hit {
                            self.land(k, h, out);
                        } else if s.kind == Weapon::Cluster as u8 && was_up && s.vy <= 0.0 {
                            self.split(k);
                        } else if s.kind == Weapon::Smoke as u8 && s.t % 50 == 0 {
                            self.puff(s.px());
                        }
                    }
                    Mode::Roll => self.roll(k, out),
                }
            }
            self.flight = self.flight.saturating_add(1);
        }
        if !self.shells.iter().any(Shell::active) {
            self.phase = Phase::Settle;
            self.t = 0;
        }
    }

    /// A cluster at the top of its flight: three bomblets.
    fn split(&mut self, k: usize) {
        let s = self.shells[k];
        for (i, dv) in SPLIT_DV.iter().enumerate() {
            self.shells[i] = Shell { vx: s.vx + dv, kind: BOMBLET, t: 40, ..s };
        }
    }

    fn puff(&mut self, (px, py): (f32, f32)) {
        let slot =
            (0..PUFFS).max_by_key(|&i| if self.puffs[i].age == 0 { u16::MAX } else { self.puffs[i].age });
        if let Some(i) = slot {
            self.puffs[i] = Puff { x: (px * 64.0) as i16, y: (py * 16.0) as i16, age: 1 };
        }
    }

    /// Shell `k` ended at `h`.
    fn land(&mut self, k: usize, h: Hit, out: &mut SalvoOut) {
        let s = self.shells[k];
        self.shells[k].mode = Mode::Off;
        let (px, py) = match h {
            Hit::Ground(x, y) | Hit::Tank(_, x, y) => (x, y),
            Hit::Out => return,
        };
        if self.impact_x == i16::MIN {
            self.impact_x = px as i16;
        }
        let (cx, cy) = (px as i32, py as i32);
        match Weapon::from_u8(s.kind) {
            Some(Weapon::Smoke) => {
                for d in [-3.0, 0.0, 3.0] {
                    self.puff((px + d, py + 2.0));
                }
            }
            Some(Weapon::Dirt) => {
                self.add_dirt(cx, cy, 14);
                self.blast = Blast { x: cx as i16, y: cy as i16, r: 3, age: 0 };
            }
            Some(Weapon::Roller) if matches!(h, Hit::Ground(..)) && py >= f32::from(self.water) => {
                let col = px as i32;
                self.shells[k] = Shell {
                    y: f32::from(self.height(col)) * M_PER_PX,
                    vx: s.vx * 0.7,
                    vy: 0.0,
                    mode: Mode::Roll,
                    t: 0,
                    ..s
                };
            }
            Some(Weapon::Digger) if matches!(h, Hit::Ground(..)) => {
                let sp = sqrt(s.vx * s.vx + s.vy * s.vy).max(1.0);
                let (ux, uy) = (s.vx / sp, s.vy / sp);
                let mut end = (cx, cy);
                for d in 0..DIG_LEN {
                    let x = (px + ux * d as f32) as i32;
                    let y = (py + uy * d as f32) as i32;
                    if y < 0 {
                        break;
                    }
                    self.crater(x, y, DIG_R);
                    end = (x, y);
                }
                self.explode(end.0, end.1, Weapon::Digger as u8, out);
            }
            _ => self.explode(cx, cy, s.kind, out),
        }
    }

    /// A roller's physics step: along the ground, downhill faster.
    fn roll(&mut self, k: usize, out: &mut SalvoOut) {
        let mut s = self.shells[k];
        let g = self.g();
        let col = (s.x / M_PER_PX) as i32;
        let slope = (f32::from(self.height(col + 1)) - f32::from(self.height(col - 1))) / 2.0;
        let a = -g * slope / (1.0 + slope * slope);
        let friction = 0.08 * g;
        s.vx += a * DT;
        s.vx = if s.vx > 0.0 { (s.vx - friction * DT).max(0.0) } else { (s.vx + friction * DT).min(0.0) };
        let nx = s.x + s.vx * DT;
        let ncol = (nx / M_PER_PX) as i32;
        let w = i32::from(self.width());
        s.t = s.t.saturating_add(1);
        let here = i32::from(self.height(col));
        let wall = ncol != col && i32::from(self.height(ncol)) > here + 3;
        let stopped = fabs(s.vx) < 0.3 && fabs(slope) < 0.6 && s.t > 20;
        let under = i32::from(self.water) > here;
        let tank = self.tanks().iter().any(|t| {
            t.alive()
                && (i32::from(t.x) - ncol).abs() <= i32::from(TANK_HALF) + 1
                && (i32::from(t.y_px()) - here).abs() <= 6
        });
        if nx < 0.0 || ncol >= w {
            self.shells[k].mode = Mode::Off;
            return;
        }
        if wall || stopped || under || tank || s.t > 4 * PHYS_HZ as u16 {
            self.shells[k] = s;
            let y = f32::from(self.height(col).max(self.water)) + 1.0;
            self.land_roller(k, s.x / M_PER_PX, y, out);
            return;
        }
        s.x = nx;
        s.y = f32::from(self.height(ncol)) * M_PER_PX;
        self.shells[k] = s;
    }

    fn land_roller(&mut self, k: usize, px: f32, py: f32, out: &mut SalvoOut) {
        self.shells[k].mode = Mode::Off;
        self.explode(px as i32, py as i32, Weapon::Roller as u8, out);
    }

    /// Cut a disc out of the ground; the dirt above falls into the hole.
    fn crater(&mut self, cx: i32, cy: i32, r: i32) {
        let w = i32::from(self.width());
        for x in (cx - r).max(0)..=(cx + r).min(w - 1) {
            let dx = x - cx;
            let dy = isqrt((r * r - dx * dx).max(0) as u32) as i32;
            let lo = (cy - dy).max(0);
            let hi = cy + dy;
            let h = i32::from(self.heights[x as usize]);
            let removed = h.clamp(lo, hi.max(lo)) - lo;
            self.heights[x as usize] = u8::try_from((h - removed).clamp(0, i32::from(TOP))).unwrap_or(0);
        }
    }

    /// A disc of dirt that falls onto the ground.
    fn add_dirt(&mut self, cx: i32, cy: i32, r: i32) {
        let w = i32::from(self.width());
        for x in (cx - r).max(0)..=(cx + r).min(w - 1) {
            let dx = x - cx;
            let dy = isqrt((r * r - dx * dx).max(0) as u32) as i32;
            let h = i32::from(self.heights[x as usize]);
            let top = cy + dy;
            let add = top - (cy - dy).max(h);
            if add > 0 {
                self.heights[x as usize] = u8::try_from((h + add).clamp(0, i32::from(TOP))).unwrap_or(TOP);
            }
        }
        // Tanks under it come up on top.
        for i in 0..usize::from(self.cfg.players) {
            let ground = i16::from(self.height(i32::from(self.tanks[i].x))) * 16;
            let t = &mut self.tanks[i];
            if t.alive() && t.y < ground {
                t.y = ground;
            }
        }
    }

    /// An explosion of `kind`'s blast at (cx, cy): the crater, the damage.
    fn explode(&mut self, cx: i32, cy: i32, kind: u8, out: &mut SalvoOut) {
        let sp = SPECS[usize::from(kind.min(7))];
        let r = i32::from(sp.radius);
        self.crater(cx, cy, r);
        self.blast = Blast { x: cx as i16, y: cy as i16, r: sp.radius, age: 0 };
        out.exploded = true;
        let reach = r + i32::from(TANK_HALF);
        let shooter = usize::from(self.turn);
        for i in 0..usize::from(self.cfg.players) {
            let t = self.tanks[i];
            if !t.alive() {
                continue;
            }
            let dx = i32::from(t.x) - cx;
            let dy = i32::from(t.y_px()) + i32::from(TANK_H) / 2 - cy;
            let d = isqrt((dx * dx + dy * dy) as u32) as i32;
            if d >= reach {
                continue;
            }
            let dmg = i32::from(sp.damage) * (reach - d) / reach;
            self.hurt(i, dmg, Some(shooter));
        }
    }

    /// Damage to tank `i` (by `by`): points to the shooter.
    fn hurt(&mut self, i: usize, dmg: i32, by: Option<usize>) {
        let t = self.tanks[i];
        let dmg = dmg.clamp(0, i32::from(t.health));
        if dmg == 0 {
            return;
        }
        self.tanks[i].health = t.health - dmg as u8;
        if let Some(b) = by.filter(|&b| b != i) {
            let mut pts = dmg as i16;
            if self.tanks[i].health == 0 {
                pts += KILL_POINTS;
            }
            self.award(b, pts);
        }
    }

    fn award(&mut self, i: usize, pts: i16) {
        let t = &mut self.tanks[i];
        t.score = t.score.saturating_add(pts);
        t.credits = t.credits.saturating_add(pts.max(0) as u16);
    }

    /// One logic step of the tanks: falling, sliding, sinking. True if
    /// any moved.
    fn settle_tanks(&mut self) -> bool {
        let mut moved = false;
        let w = i32::from(self.width());
        for i in 0..usize::from(self.cfg.players) {
            let mut t = self.tanks[i];
            if !t.alive() {
                continue;
            }
            let ground = i16::from(self.height(i32::from(t.x))) * 16;
            if t.y > ground {
                t.vy = t.vy.saturating_add(3).min(96);
                t.y = (t.y - i16::from(t.vy)).max(ground);
                moved = true;
                if t.y == ground {
                    // From rest at 3/16 px per step²: v² = 2·a·h.
                    let v = i32::from(t.vy);
                    let fell = (v * v / 6 / 16) as i16;
                    t.vy = 0;
                    self.tanks[i] = t;
                    if self.cfg.fall_damage && fell > FALL_FREE {
                        self.hurt(i, i32::from((fell - FALL_FREE) * FALL_HURT), None);
                    }
                    continue;
                }
            } else {
                t.y = ground;
                t.vy = 0;
                // Slide down a steep slope, a pixel every few steps.
                let x = i32::from(t.x);
                let l = i32::from(self.height(x - 2));
                let r = i32::from(self.height(x + 2));
                let me = i32::from(self.height(x));
                let dir = if r < me - 3 && r <= l && x + 2 < w - 1 {
                    1
                } else if l < me - 3 && l < r && x - 2 > 0 {
                    -1
                } else {
                    0
                };
                if dir != 0 {
                    moved = true;
                    if t.slide == 0 {
                        t.x += dir as i16;
                        t.y = t.y.max(i16::from(self.height(i32::from(t.x))) * 16);
                        t.slide = 3;
                    } else {
                        t.slide -= 1;
                    }
                } else {
                    t.slide = 0;
                }
            }
            // In the water: sunk.
            if self.water > 0 && t.y_px() < i16::from(self.water) && t.y == ground {
                t.health = 0;
                moved = true;
            }
            self.tanks[i] = t;
        }
        moved
    }

    fn end_turn(&mut self) {
        let shooter = usize::from(self.turn);
        if self.cfg.kinds[shooter].is_computer() && self.impact_x != i16::MIN {
            let k = self.cfg.kinds[shooter].level();
            let tank = self.tanks[shooter];
            let dir = if i32::from(self.search.aim_x) >= i32::from(tank.x) { 1 } else { -1 };
            let miss = (i32::from(self.impact_x) - i32::from(self.search.aim_x)) * dir;
            let t = &mut self.tanks[shooter];
            if t.ai_target == self.search.target {
                let off = i32::from(t.ai_off) - miss * dir * AI_GAIN[k] / 10;
                t.ai_off = i8::try_from(off.clamp(-60, 60)).unwrap_or(0);
            }
        }
        let alive = self.tanks().iter().filter(|t| t.alive()).count();
        if alive <= 1 {
            if let Some(w) = self.round_winner() {
                self.award(usize::from(w), WIN_POINTS);
            }
            for t in &mut self.tanks[..usize::from(self.cfg.players)] {
                t.credits = t.credits.saturating_add(ROUND_CREDITS);
            }
            self.phase = Phase::RoundOver;
            self.t = 0;
            return;
        }
        match self.next_alive(self.turn) {
            Some(p) => self.begin_turn(p),
            None => self.phase = Phase::RoundOver,
        }
    }

    /// After the round's banner: the shop, the next round, or the end.
    fn after_round(&mut self) {
        if self.round + 1 >= self.cfg.rounds {
            self.phase = Phase::Over;
            return;
        }
        if self.cfg.shop {
            // Computers buy at once: heavy shells, then clusters.
            for i in 0..usize::from(self.cfg.players) {
                if self.cfg.kinds[i].is_computer() {
                    for w in [Weapon::Heavy, Weapon::Cluster, Weapon::Heavy] {
                        self.buy(i, w as u8);
                    }
                }
            }
            if let Some(p) = self.next_shopper(0) {
                self.phase = Phase::Shop;
                self.shop_player = p;
                self.shop_row = 1;
                return;
            }
        }
        self.start_round(self.round + 1);
    }

    fn next_shopper(&self, from: u8) -> Option<u8> {
        (from..self.cfg.players).find(|&p| !self.cfg.kinds[usize::from(p)].is_computer())
    }

    /// Buy one of weapon `w` for player `i`; false if not enough credits.
    pub fn buy(&mut self, i: usize, w: u8) -> bool {
        let Some(sp) = SPECS.get(usize::from(w)).filter(|_| (1..WEAPONS as u8).contains(&w)) else {
            return false;
        };
        let t = &mut self.tanks[i];
        if t.credits < sp.price || t.rounds(w) >= 99 {
            return false;
        }
        t.credits -= sp.price;
        if let Some(n) = t.rounds_mut(w) {
            *n += 1;
        }
        true
    }

    // --------------------------------------------------------- computer

    fn think_start(&mut self) {
        let me = usize::from(self.turn);
        let k = self.cfg.kinds[me];
        let lv = k.level();
        let mx = self.tanks[me].x;
        // The target: the nearest living tank (Novice: any).
        let mut cands = [0u8; MAX_PLAYERS];
        let mut n = 0;
        for (i, t) in self.tanks().iter().enumerate() {
            if i != me && t.alive() {
                cands[n] = i as u8;
                n += 1;
            }
        }
        let target = if n == 0 {
            me as u8
        } else if k == Kind::Novice {
            cands[self.rng.below(n as u32) as usize]
        } else {
            *cands[..n]
                .iter()
                .min_by_key(|&&c| (self.tanks[usize::from(c)].x - mx).abs())
                .unwrap_or(&cands[0])
        };
        let t = &mut self.tanks[me];
        if t.ai_target != target {
            t.ai_target = target;
            t.ai_off = 0;
        }
        let noise = AI_WIND_NOISE[lv] * i32::from(self.cfg.wind) / 100;
        let nv = self.rng.below(2 * noise.unsigned_abs() + 1) as i32 - noise;
        let wind = f32::from(self.wind) / 10.0 + nv as f32 / 10.0 + f32::from(self.tanks[me].ai_bias) / 4.0;
        let tx = self.tanks[usize::from(target)].x;
        let aim = i32::from(tx) + i32::from(self.tanks[me].ai_off);
        // Weapon: the heavy shell for a healthy target, else the shell.
        let th = self.tanks[usize::from(target)].health;
        let weapon = if self.available(me, Weapon::Heavy as u8) && th > 40 { Weapon::Heavy as u8 } else { 0 };
        self.search = Search {
            target,
            cand: 0,
            iter: 0,
            sims: 0,
            lo: 0,
            hi: POWER_MAX,
            best_angle: 45,
            best_power: 500,
            best_err: u16::MAX,
            aim_x: i16::try_from(aim.clamp(0, i32::from(self.width()) - 1)).unwrap_or(0),
            wind,
            done: n == 0,
            weapon,
        };
    }

    /// One simulated shot of the search.
    fn think_step(&mut self) {
        let me = usize::from(self.turn);
        let lv = self.cfg.kinds[me].level();
        let mut s = self.search;
        if s.done {
            return;
        }
        let mx = self.tanks[me].x;
        let dir: i32 = if i32::from(s.aim_x) >= i32::from(mx) { 1 } else { -1 };
        let elev = ANGLES[usize::from(s.cand)];
        let angle = if dir > 0 { elev } else { 180 - elev };
        let power = (s.lo + s.hi) / 2;
        let kind = s.weapon;
        let shell = self.launch(me, angle, power, kind);
        // Expert with gusts: the gusts the flight will draw (the turn's
        // generator); everyone else a steady wind.
        let gusts = (lv == 3 && self.cfg.gusts).then_some(self.rng);
        let mut last = 0.0f32;
        let hit = self.simulate(shell, me, s.wind, gusts, |_, sh| {
            last = sh.px().0;
            true
        });
        let e = match hit {
            Hit::Ground(x, _) | Hit::Tank(_, x, _) => (x - f32::from(s.aim_x)) * dir as f32,
            // Off the side (beyond the target or behind the tank), or
            // still flying: where it was last.
            Hit::Out => (last - f32::from(s.aim_x)) * dir as f32,
        };
        // A shot that would hurt itself is never the best.
        let near_self = match hit {
            Hit::Ground(x, y) | Hit::Tank(_, x, y) => {
                let r = f32::from(SPECS[usize::from(kind)].radius) + 6.0;
                let (sx, sy) = self.pivot(me);
                fabs(x - sx) < r && fabs(y - sy) < r
            }
            Hit::Out => false,
        };
        let err = (fabs(e) * 4.0).min(60_000.0) as u16;
        if err < s.best_err && !near_self {
            s.best_err = err;
            s.best_angle = angle;
            s.best_power = power;
        }
        if e < 0.0 {
            s.lo = power;
        } else {
            s.hi = power;
        }
        s.iter += 1;
        s.sims = s.sims.saturating_add(1);
        if s.best_err <= 4 {
            s.done = true;
        } else if s.iter >= AI_ITERS[lv] || s.hi - s.lo <= 1 {
            s.cand += 1;
            s.iter = 0;
            s.lo = 0;
            s.hi = POWER_MAX;
            if s.cand >= AI_ANGLES[lv] {
                s.done = true;
            }
        }
        self.search = s;
    }

    /// The search is over: the aim with the difficulty's noise.
    fn think_done(&mut self) {
        let me = usize::from(self.turn);
        let lv = self.cfg.kinds[me].level();
        let s = self.search;
        let (an, pn) = (AI_ANGLE_NOISE[lv], AI_POWER_NOISE[lv]);
        let mut angle = i32::from(s.best_angle);
        let mut power = i32::from(s.best_power);
        if an > 0 {
            angle += self.rng.below(2 * an + 1) as i32 - an as i32;
        }
        if pn > 0 {
            power += self.rng.below(2 * pn + 1) as i32 - pn as i32;
        }
        let t = &mut self.tanks[me];
        t.weapon = s.weapon;
        self.search.best_angle = u8::try_from(angle.clamp(0, 180)).unwrap_or(90);
        self.search.best_power = u16::try_from(power.clamp(0, i32::from(POWER_MAX))).unwrap_or(500);
        self.phase = Phase::Turn;
        self.t = 0;
    }

    /// The barrel turned toward the chosen shot by a step; true when
    /// there.
    fn turn_barrel(&mut self) -> bool {
        let me = usize::from(self.turn);
        let (ga, gp) = (self.search.best_angle, self.search.best_power);
        let t = &mut self.tanks[me];
        let step = if self.fast { 6 } else { 2 };
        t.angle = if t.angle < ga { (t.angle + step).min(ga) } else { t.angle.saturating_sub(step).max(ga) };
        let ps = if self.fast { 60 } else { 20 };
        t.power = if t.power < gp { (t.power + ps).min(gp) } else { t.power.saturating_sub(ps).max(gp) };
        t.angle == ga && t.power == gp
    }

    /// Run the computer's whole search at once (tests, tools); returns
    /// the simulated shots.
    pub fn think_all(&mut self) -> u8 {
        while self.phase == Phase::Think && !self.search.done {
            self.think_step();
        }
        self.search.sims
    }

    // --------------------------------------------------------- input

    fn aim_by(&mut self, b: Button, repeat: u16) {
        let me = usize::from(self.turn);
        let t = &mut self.tanks[me];
        match b {
            Button::Up => t.angle = (t.angle + 1).min(180),
            Button::Down => t.angle = t.angle.saturating_sub(1),
            Button::Left | Button::Right => {
                let d = match repeat {
                    0..60 => 1,
                    60..150 => 5,
                    _ => 20,
                };
                t.power =
                    if b == Button::Right { (t.power + d).min(POWER_MAX) } else { t.power.saturating_sub(d) };
            }
            _ => {}
        }
    }

    /// A key.
    pub fn input(&mut self, i: Input) -> SalvoOut {
        let mut out = SalvoOut::default();
        if let Input::Long(Button::Ok) = i {
            self.ok_down = false;
            self.overview = !self.overview;
            return out;
        }
        if let Input::Release(b) = i
            && self.held == Some(b)
        {
            self.held = None;
        }
        if let Some(cur) = self.menu {
            match i {
                Input::Press(Button::Up) => self.menu = Some(self.menu_move(cur, -1)),
                Input::Press(Button::Down) => self.menu = Some(self.menu_move(cur, 1)),
                Input::Press(Button::Ok) => {
                    self.tanks[usize::from(self.turn)].weapon = cur;
                    self.menu = None;
                }
                Input::Press(Button::Menu) => self.menu = None,
                _ => {}
            }
            return out;
        }
        match self.phase {
            Phase::Pass => {
                if i == Input::Press(Button::Ok) {
                    self.phase = Phase::Aim;
                    self.timer = u16::from(self.cfg.timer) * HZ;
                }
            }
            Phase::Aim => match i {
                Input::Press(b @ (Button::Up | Button::Down | Button::Left | Button::Right)) => {
                    self.aim_by(b, 0);
                    self.held = Some(b);
                    self.held_t = 0;
                }
                Input::Press(Button::Ok) => self.ok_down = true,
                Input::Release(Button::Ok) if self.ok_down => {
                    self.ok_down = false;
                    out = self.fire();
                }
                Input::Press(Button::Menu) => self.menu = Some(self.tanks[usize::from(self.turn)].weapon),
                _ => {}
            },
            Phase::Think | Phase::Turn | Phase::Flight | Phase::Settle => match i {
                Input::Press(Button::Ok) => self.fast = !self.fast,
                Input::Press(b @ (Button::Left | Button::Right)) => {
                    self.held = Some(b);
                    self.held_t = 0;
                    self.follow = false;
                }
                _ => {}
            },
            Phase::RoundOver => {
                if i == Input::Press(Button::Ok) {
                    self.after_round();
                }
            }
            Phase::Shop => self.shop_input(i),
            Phase::Over => {}
        }
        out
    }

    /// The weapon list's cursor moved by one, over weapons in stock.
    fn menu_move(&self, cur: u8, d: i8) -> u8 {
        let me = usize::from(self.turn);
        let mut c = cur;
        for _ in 0..WEAPONS {
            c = (i16::from(c) + i16::from(d)).rem_euclid(WEAPONS as i16) as u8;
            if self.available(me, c) {
                return c;
            }
        }
        cur
    }

    /// Close the weapon list (Back); true if it was open.
    pub fn close_menu(&mut self) -> bool {
        self.menu.take().is_some()
    }

    fn shop_input(&mut self, i: Input) {
        let p = usize::from(self.shop_player);
        match i {
            Input::Press(Button::Up) => {
                self.shop_row = if self.shop_row <= 1 { 7 } else { self.shop_row - 1 }
            }
            Input::Press(Button::Down) => {
                self.shop_row = if self.shop_row >= 7 { 1 } else { self.shop_row + 1 }
            }
            Input::Press(Button::Ok) => {
                if self.shop_row < 7 {
                    self.buy(p, self.shop_row);
                } else {
                    match self.next_shopper(self.shop_player + 1) {
                        Some(q) => {
                            self.shop_player = q;
                            self.shop_row = 1;
                        }
                        None => self.start_round(self.round + 1),
                    }
                }
            }
            _ => {}
        }
    }

    /// Let go of every key (a pause).
    pub fn release_all(&mut self) {
        self.held = None;
        self.ok_down = false;
    }

    /// Finish what is in the air at once (before a pause is saved): the
    /// flight, the blast, the tanks settling, up to the next turn.
    pub fn settle_now(&mut self) {
        for _ in 0..20_000 {
            if !matches!(self.phase, Phase::Flight | Phase::Settle) {
                break;
            }
            self.step();
        }
    }

    // --------------------------------------------------------- step

    /// One logic step ([`crate::STEP_HZ`]).
    pub fn step(&mut self) -> SalvoOut {
        let mut out = SalvoOut::default();
        self.t = self.t.saturating_add(1);
        if self.blast.age < BLAST_STEPS {
            self.blast.age += 1;
        }
        self.drift_puffs();
        match self.phase {
            Phase::Pass | Phase::Shop | Phase::Over => {}
            Phase::Aim => {
                if let Some(b) = self.held {
                    self.held_t = self.held_t.saturating_add(1);
                    let t = self.held_t;
                    let every =
                        if matches!(b, Button::Up | Button::Down) { if t < 60 { 3 } else { 1 } } else { 2 };
                    if t > 15 && t % every == 0 {
                        self.aim_by(b, t);
                    }
                }
                if self.cfg.timer > 0 && self.menu.is_none() {
                    self.timer = self.timer.saturating_sub(1);
                    if self.timer == 0 {
                        out = self.fire();
                    }
                }
            }
            Phase::Think => {
                if !self.search.done {
                    self.think_step();
                }
                if self.search.done && (self.t >= THINK_MIN || self.fast) {
                    self.think_done();
                }
            }
            Phase::Turn => {
                if self.turn_barrel() && self.t > 6 {
                    out = self.fire();
                }
            }
            Phase::Flight => self.fly_step(&mut out),
            Phase::Settle => {
                let moving = self.settle_tanks();
                if !moving && !self.blast.shown() {
                    self.end_turn();
                }
            }
            Phase::RoundOver => {
                if self.cfg.humans() == 0 && self.t > 2 * HZ {
                    self.after_round();
                }
            }
        }
        self.pan_and_follow();
        out.ended = self.ended();
        out
    }

    fn drift_puffs(&mut self) {
        let (w, g) = self.wind();
        // Air movement in a logic step (0.1 s of game time), 1/64 px.
        let dx = ((w + g) * (SUB as f32 * DT) / M_PER_PX * 64.0) as i32;
        let wpx = i32::from(self.width()) * 64;
        for p in &mut self.puffs {
            if p.age == 0 {
                continue;
            }
            p.age += 1;
            p.x = p.x.saturating_add(i16::try_from(dx).unwrap_or(0));
            p.y = p.y.saturating_add(1).min(WORLD_H * 16);
            if p.age > PUFF_LIFE || p.x < 0 || i32::from(p.x) >= wpx {
                *p = Puff::default();
            }
        }
    }

    fn pan_and_follow(&mut self) {
        let w = i16::try_from(self.width()).unwrap_or(VIEW_W);
        let max = (w - VIEW_W).max(0);
        if let Some(b @ (Button::Left | Button::Right)) = self.held
            && self.phase != Phase::Aim
        {
            self.cam = (self.cam + if b == Button::Right { 3 } else { -3 }).clamp(0, max);
            return;
        }
        if !self.follow {
            return;
        }
        let target = match self.shells.iter().find(|s| s.active()) {
            Some(s) => s.px().0 as i16,
            None => self.tanks[usize::from(self.turn)].x,
        };
        let want = (target - VIEW_W / 2).clamp(0, max);
        let d = want - self.cam;
        self.cam += if d.abs() <= 4 { d } else { d.signum() * (d.abs() / 4).max(4) };
        self.cam = self.cam.clamp(0, max);
    }

    // --------------------------------------------------------- tests

    /// Make it player `p`'s turn now (tests and the simulator's
    /// scripts).
    pub fn force_turn(&mut self, p: u8) {
        if p < self.cfg.players && self.tanks[usize::from(p)].alive() {
            self.shells = [Shell::default(); SHELLS];
            self.begin_turn(p);
            if self.phase == Phase::Pass {
                self.phase = Phase::Aim;
            }
        }
    }

    /// Replace the terrain (tests, scripts): `h` per column, clamped to
    /// `0..=TOP`. Tanks it covers come up on top; tanks above it fall when
    /// the next shot settles.
    pub fn set_heights(&mut self, h: &[u8]) {
        let w = usize::from(self.width());
        for x in 0..MAX_W {
            self.heights[x] = if x < w { h.get(x).copied().unwrap_or(0).min(TOP) } else { 0 };
        }
        for i in 0..usize::from(self.cfg.players) {
            let g = i16::from(self.height(i32::from(self.tanks[i].x))) * 16;
            if self.tanks[i].y < g {
                self.tanks[i].y = g;
            }
        }
    }

    /// An explosion of `weapon`'s blast at (x, y) px, scored for the
    /// player whose turn it is (tests, scripts).
    pub fn explode_at(&mut self, x: i32, y: i32, weapon: u8) {
        let mut out = SalvoOut::default();
        self.explode(x, y, weapon.min(BOMBLET), &mut out);
    }

    /// Put tank `i` at column `x`, on the ground (tests, scripts).
    pub fn place(&mut self, i: usize, x: i16) {
        let w = i16::try_from(self.width()).unwrap_or(VIEW_W);
        let g = i16::from(self.height(i32::from(x.clamp(0, w - 1)))) * 16;
        if let Some(t) = self.tanks.get_mut(i) {
            t.x = x.clamp(0, w - 1);
            t.y = g;
            t.vy = 0;
        }
    }

    /// Set tank `i`'s aim (tests, scripts).
    pub fn set_aim(&mut self, i: usize, angle: u8, power: u16) {
        if let Some(t) = self.tanks.get_mut(i) {
            t.angle = angle.min(180);
            t.power = power.min(POWER_MAX);
        }
    }

    /// Set tank `i`'s health (tests, scripts; 0 destroys it).
    pub fn set_health(&mut self, i: usize, h: u8) {
        if let Some(t) = self.tanks.get_mut(i) {
            t.health = h.min(self.cfg.health);
        }
    }

    /// Set the turn's wind (tenths of m/s; tests, scripts).
    pub fn set_wind(&mut self, tenths: i16) {
        let m = i16::from(self.cfg.wind) * 10;
        self.wind = tenths.clamp(-m, m);
    }

    /// The invariants (tests, the fuzz target).
    pub fn check(&self) -> Result<(), Broken> {
        let w = usize::from(self.width());
        if self.heights[..w].iter().any(|&h| h > TOP) {
            return Err(Broken::TerrainAboveTop);
        }
        if self.heights[w..].iter().any(|&h| h != 0) {
            return Err(Broken::TerrainBeyondWorld);
        }
        if self.turn >= self.cfg.players {
            return Err(Broken::TurnOfNoPlayer);
        }
        for t in self.tanks() {
            if t.health > self.cfg.health {
                return Err(Broken::HealthAboveSetting);
            }
            if t.alive() && (t.x < 0 || t.x >= w as i16 || t.y < 0 || t.y > WORLD_H * 16) {
                return Err(Broken::TankOutsideWorld);
            }
            if t.angle > 180 || t.power > POWER_MAX {
                return Err(Broken::AimOutOfRange);
            }
            if t.alive() && t.y < i16::from(self.height(i32::from(t.x))) * 16 {
                return Err(Broken::TankInGround);
            }
        }
        for s in self.shells.iter().filter(|s| s.active()) {
            if !s.finite() {
                return Err(Broken::ShellNotFinite);
            }
            if self.phase != Phase::Flight {
                return Err(Broken::ShellOutsideFlight);
            }
            let (px, py) = s.px();
            if s.mode == Mode::Fly && px >= 0.0 && py < self.surface(px as i32) {
                return Err(Broken::ShellInGround);
            }
        }
        if !f32::from_bits(self.gust).is_finite() || !self.search.wind.is_finite() {
            return Err(Broken::WindNotFinite);
        }
        if self.cam < 0 || self.cam > (w as i16 - VIEW_W).max(0) {
            return Err(Broken::ViewOutsideWorld);
        }
        Ok(())
    }

    // --------------------------------------------------------- save

    /// The save file's payload. A game saved during a shot comes back
    /// settling (what was in the air is lost), one saved while a computer
    /// thinks comes back thinking (the search starts again).
    pub fn encode(&self, w: &mut Writer<'_>) {
        self.cfg.encode(w);
        w.u64(self.rng.state());
        let phase = match self.phase {
            Phase::Flight | Phase::Settle => Phase::Settle,
            Phase::Turn | Phase::Think => Phase::Think,
            p => p,
        };
        for v in [phase as u8, self.turn, self.round, self.water, self.shop_player, self.shop_row] {
            w.u8(v);
        }
        w.u16(self.wind as u16);
        w.u16(self.timer);
        // Heights, seven bits each (TOP < 128).
        let n = usize::from(self.width());
        let (mut acc, mut bits) = (0u32, 0u32);
        for &h in &self.heights[..n] {
            acc |= u32::from(h & 0x7F) << bits;
            bits += 7;
            while bits >= 8 {
                w.u8(acc as u8);
                acc >>= 8;
                bits -= 8;
            }
        }
        if bits > 0 {
            w.u8(acc as u8);
        }
        for t in self.tanks() {
            w.u16(t.x as u16);
            w.u16(t.y as u16);
            w.u8(t.angle);
            w.u16(t.power);
            w.u8(t.health);
            w.u8(t.weapon);
            w.bytes(&t.ammo);
            w.u16(t.score as u16);
            w.u16(t.credits);
            w.u8(t.ai_target);
            w.i8(t.ai_off);
            w.i8(t.ai_bias);
            w.u8(t.last_angle);
            w.u16(t.last_power);
            w.u8(t.last_weapon);
            w.i8(t.last_wind);
        }
    }

    #[must_use]
    pub fn decode(r: &mut Reader<'_>) -> Option<Salvo> {
        let mut s = Salvo::blank(Config::DEFAULT, 0);
        s.decode_from(r)?;
        Some(s)
    }

    /// [`Salvo::decode`] into this game, in place (the firmware's UI task
    /// has no room for a second game on its stack). `None`: damaged (the
    /// game is then a blank one).
    pub fn decode_from(&mut self, r: &mut Reader<'_>) -> Option<()> {
        let cfg = Config::decode(r)?;
        let seed = r.u64()?;
        self.reset(cfg, seed);
        let s = self;
        let [phase, turn, round, water, shop_player, shop_row] = r.array::<6>()?;
        s.phase = *Phase::ALL.get(usize::from(phase))?;
        s.turn = turn;
        s.round = round;
        s.water = water;
        s.shop_player = shop_player;
        s.shop_row = shop_row;
        s.wind = r.u16()? as i16;
        s.timer = r.u16()?;
        let n = usize::from(s.width());
        let (mut acc, mut bits) = (0u32, 0u32);
        for i in 0..n {
            while bits < 7 {
                acc |= u32::from(r.u8()?) << bits;
                bits += 8;
            }
            s.heights[i] = (acc & 0x7F) as u8;
            acc >>= 7;
            bits -= 7;
        }
        for i in 0..usize::from(cfg.players) {
            let t = &mut s.tanks[i];
            t.x = r.u16()? as i16;
            t.y = r.u16()? as i16;
            t.angle = r.u8()?;
            t.power = r.u16()?;
            t.health = r.u8()?;
            t.weapon = r.u8()?;
            let a = r.array::<6>()?;
            t.ammo = a;
            t.score = r.u16()? as i16;
            t.credits = r.u16()?;
            t.ai_target = r.u8()?;
            t.ai_off = r.i8()?;
            t.ai_bias = r.i8()?;
            t.last_angle = r.u8()?;
            t.last_power = r.u16()?;
            t.last_weapon = r.u8()?;
            t.last_wind = r.i8()?;
        }
        let m = i16::from(cfg.wind) * 10;
        let ok = turn < cfg.players
            && round < cfg.rounds
            && shop_player < cfg.players
            && (1..=7).contains(&shop_row)
            && i32::from(water) <= i32::from(TOP)
            && (-m..=m).contains(&s.wind)
            && s.timer <= u16::from(cfg.timer) * HZ
            && s.tanks().iter().all(|t| {
                usize::from(t.weapon) < WEAPONS
                    && (t.last_weapon == 0xFF || usize::from(t.last_weapon) < WEAPONS)
            });
        if !ok {
            return None;
        }
        if s.phase == Phase::Think {
            if !cfg.kinds[usize::from(turn)].is_computer() {
                return None;
            }
            s.think_start();
        }
        s.cam = 0;
        for _ in 0..64 {
            s.pan_and_follow();
        }
        s.check().ok()
    }
}
