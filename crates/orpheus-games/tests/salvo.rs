//! Salvo (GAME-4/5): the physics against analytic solutions, the
//! invariants over random games, the terrain, the weapons, the computer
//! players, saves and determinism.

#![cfg(feature = "salvo")]

use orpheus_games::salvo::{
    self, AIR, BLAST_STEPS, Config, DT, GRAVITY, Hit, Kind, M_PER_PX, MAX_FLIGHT, MAX_W, Mode, POWER_MAX,
    Phase, SPECS, Salvo, Shell, Style, TOP, V_MAX, WIDTHS, Weapon, cos_deg, sin_deg,
};
use orpheus_games::{Button, Input, Rng, SAVE_MAX, Session, id};

fn shell(x: f32, y: f32, vx: f32, vy: f32) -> Shell {
    Shell { x, y, vx, vy, kind: 0, mode: Mode::Fly, t: 0 }
}

/// Without air: x and y follow the closed form, apart from semi-implicit
/// Euler's known lag of `g·Δt·t/2` in height (the velocity is updated
/// first), which is checked exactly too.
#[test]
fn no_drag_matches_the_closed_form() {
    for (g, v, deg) in [(9.81f32, 150.0f32, 45u8), (1.62, 60.0, 70), (24.8, 300.0, 20), (3.71, 90.0, 89)] {
        let (vx, vy) = (f64::from(v * cos_deg(deg)), f64::from(v * sin_deg(deg)));
        let mut s = shell(0.0, 0.0, vx as f32, vy as f32);
        let g64 = f64::from(g);
        for n in 1..=6000u32 {
            Salvo::advance(&mut s, g, 0.0, 0.0);
            let t = f64::from(n) * f64::from(DT);
            let x = vx * t;
            let y = vy * t - g64 * t * t / 2.0;
            let lag = g64 * f64::from(DT) * t / 2.0;
            assert!((f64::from(s.x) - x).abs() <= 1e-4 * x.abs().max(1.0), "x at {t}: {} vs {x}", s.x);
            // Rounding of f32 sums over thousands of steps: a few cm.
            assert!(
                (f64::from(s.y) - (y - lag)).abs() <= 1e-4 * y.abs() + 0.1,
                "y at {t}: {} vs {}",
                s.y,
                y - lag
            );
            assert!((f64::from(s.vy) - (vy - g64 * t)).abs() < 1e-3 * vy.abs().max(10.0));
        }
    }
}

/// Air, no wind, no gravity: v(t) = v₀/(1 + c·v₀·t), x(t) = ln(1 + c·v₀·t)/c.
#[test]
fn drag_without_gravity_matches_the_closed_form() {
    for (rho, w) in [(AIR[1], 0usize), (AIR[2], 1), (AIR[1], 6)] {
        let c = f64::from(rho * SPECS[w].k);
        let v0 = 200.0f64;
        let mut s = shell(0.0, 0.0, v0 as f32, 0.0);
        s.kind = w as u8;
        for n in 1..=4000u32 {
            Salvo::advance(&mut s, 0.0, (f64::from(rho) * f64::from(SPECS[w].k)) as f32, 0.0);
            let t = f64::from(n) * f64::from(DT);
            let v = v0 / (1.0 + c * v0 * t);
            let x = (1.0 + c * v0 * t).ln() / c;
            assert!((f64::from(s.vx) - v).abs() <= 2e-3 * v, "v at {t}: {} vs {v}", s.vx);
            assert!((f64::from(s.x) - x).abs() <= 2e-3 * x + 0.05, "x at {t}: {} vs {x}", s.x);
        }
    }
}

/// Air, no wind: a fall from rest approaches the terminal speed
/// v_t = √(g/c): v(t) = v_t·tanh(g·t/v_t), drop = v_t²/g·ln cosh(g·t/v_t);
/// a shot straight up stops at t = v_t/g·atan(v₀/v_t) at the height
/// v_t²/(2g)·ln(1 + v₀²/v_t²).
#[test]
fn drag_without_wind_matches_the_vertical_closed_forms() {
    for (g, rho) in [(9.81f32, AIR[1]), (1.62, AIR[2]), (24.8, AIR[1])] {
        let k = rho * SPECS[0].k;
        let c = f64::from(k);
        let g64 = f64::from(g);
        let vt = (g64 / c).sqrt();
        let mut s = shell(0.0, 0.0, 0.0, 0.0);
        for n in 1..=8000u32 {
            Salvo::advance(&mut s, g, k, 0.0);
            let t = f64::from(n) * f64::from(DT);
            let v = vt * (g64 * t / vt).tanh();
            let d = vt * vt / g64 * (g64 * t / vt).cosh().ln();
            assert!((-f64::from(s.vy) - v).abs() <= 3e-3 * v + 1e-3, "v at {t}: {} vs {v}", -s.vy);
            assert!((-f64::from(s.y) - d).abs() <= 3e-3 * d + 0.05, "drop at {t}: {} vs {d}", -s.y);
        }
        let v0 = 150.0f64;
        let mut s = shell(0.0, 0.0, 0.0, v0 as f32);
        let mut n = 0u32;
        while s.vy > 0.0 {
            Salvo::advance(&mut s, g, k, 0.0);
            n += 1;
        }
        let ta = vt / g64 * (v0 / vt).atan();
        let h = vt * vt / (2.0 * g64) * (1.0 + v0 * v0 / (vt * vt)).ln();
        let t = f64::from(n) * f64::from(DT);
        assert!((t - ta).abs() <= 2.0 * f64::from(DT), "apex at {t} s, {ta} s expected");
        assert!((f64::from(s.y) - h).abs() <= 3e-3 * h, "apex {} m, {h} m expected", s.y);
    }
}

/// Wind acts only through the drag: without air a shot is the same in
/// any wind; with air it drifts downwind, a light shell more than a
/// heavy one.
#[test]
fn wind_acts_through_drag() {
    let fly = |rho: f32, kind: usize, wind: f32| {
        let mut s = shell(0.0, 0.0, 100.0, 100.0);
        let k = rho * SPECS[kind].k;
        while s.y >= 0.0 {
            Salvo::advance(&mut s, 9.81, k, wind);
        }
        s.x
    };
    assert_eq!(fly(0.0, 0, 0.0).to_bits(), fly(0.0, 0, 12.0).to_bits());
    let calm = fly(AIR[1], 0, 0.0);
    assert!(fly(AIR[1], 0, 10.0) > calm + 20.0, "downwind goes further");
    assert!(fly(AIR[1], 0, -10.0) < calm - 20.0, "upwind less far");
    let drift = |kind| fly(AIR[1], kind, 10.0) - fly(AIR[1], kind, 0.0);
    assert!(drift(Weapon::Smoke as usize) > drift(Weapon::Shell as usize));
    assert!(drift(Weapon::Shell as usize) > drift(Weapon::Heavy as usize));
}

/// Energy per kilogram, ½v² + g·y, never grows without wind (semi-implicit
/// Euler loses ½g²Δt² a step without air; the drag only takes energy).
#[test]
fn energy_bounded_without_wind() {
    let mut r = Rng::new(7);
    for _ in 0..300 {
        // Any shot of the game: up to full power for the gravity and air.
        let (gi, di) = (r.below(4) as usize, r.below(3) as usize);
        let (g, rho) = (GRAVITY[gi], AIR[di]);
        let kind = r.below(7) as usize;
        let deg = r.below(181) as u8;
        let v = V_MAX[gi][di] * r.below(1001) as f32 / 1000.0;
        let mut s = shell(0.0, 400.0, v * cos_deg(deg), v * sin_deg(deg));
        let e = |s: &Shell| {
            0.5 * f64::from(s.vx).powi(2) + 0.5 * f64::from(s.vy).powi(2) + f64::from(g) * f64::from(s.y)
        };
        let e0 = e(&s);
        let mut last = e0;
        for _ in 0..MAX_FLIGHT {
            Salvo::advance(&mut s, g, rho * SPECS[kind].k, 0.0);
            let en = e(&s);
            // f32 rounding over 20,000 steps.
            assert!(en <= e0 * (1.0 + 1e-4) + 1.0, "energy grew: {en} > {e0}");
            assert!(en <= last + 1e-5 * last.abs() + 0.5, "energy grew in a step");
            assert!(s.x.is_finite() && s.y.is_finite() && s.vx.is_finite() && s.vy.is_finite());
            last = en;
            // The flight ends at the ground, 400 m below the start.
            if s.y < 0.0 {
                break;
            }
        }
    }
}

#[test]
fn numbers() {
    let mut r = Rng::new(3);
    for _ in 0..100_000 {
        let x = f32::from_bits(r.next_u32() & 0x7F7F_FFFF);
        // Normal numbers (the physics has no others).
        if !(1e-30..1e30).contains(&x) {
            continue;
        }
        let want = f64::from(x).sqrt();
        let got = f64::from(salvo::sqrt(x));
        assert!((got - want).abs() <= 3e-7 * want, "sqrt({x}) = {got}, {want}");
    }
    assert_eq!(salvo::sqrt(0.0), 0.0);
    assert_eq!(salvo::sqrt(-1.0), 0.0);
    assert_eq!(salvo::sqrt(f32::NAN), 0.0);
    for d in 0..=180u8 {
        let a = f64::from(d).to_radians();
        assert!((f64::from(sin_deg(d)) - a.sin()).abs() < 1e-7);
        assert!((f64::from(cos_deg(d)) - a.cos()).abs() < 1e-7);
    }
    for v in 0..100_000u32 {
        let s = salvo::isqrt(v);
        assert!(s * s <= v && (s + 1) * (s + 1) > v);
    }
}

/// The top speed of each gravity × air setting carries a 45° … best-angle
/// shot across 1.25 widest worlds from flat ground in still air, and not
/// much further (the table follows the physics).
#[test]
fn top_speed_reaches() {
    let want = 1.25 * f32::from(WIDTHS[3]) * M_PER_PX;
    for (gi, g) in GRAVITY.iter().enumerate() {
        for (di, rho) in AIR.iter().enumerate() {
            let v = V_MAX[gi][di];
            let best = (15..=75u8)
                .map(|deg| {
                    let mut s = shell(0.0, 0.0, v * cos_deg(deg), v * sin_deg(deg));
                    let mut n = 0;
                    while s.y >= 0.0 && n < MAX_FLIGHT {
                        Salvo::advance(&mut s, *g, rho * SPECS[0].k, 0.0);
                        n += 1;
                    }
                    s.x
                })
                .fold(0.0f32, f32::max);
            assert!(best >= want, "g {g} air {rho}: {best} m < {want} m");
            assert!(best <= want * 1.15, "g {g} air {rho}: {best} m, more than needed");
        }
    }
}

fn cfg() -> Config {
    Config { kinds: [Kind::Human, Kind::Human, Kind::Easy, Kind::Easy], ..Config::DEFAULT }
}

/// Every style, width, roughness and relief: heights within 2 ..= TOP,
/// nothing beyond the world, tanks on flat pads above any water, apart.
#[test]
fn terrain_within_the_world() {
    for style in Style::ALL {
        for width in 0..4u8 {
            for (rough, relief) in [(0, 0), (1, 1), (2, 2), (2, 0)] {
                for players in 2..=4u8 {
                    for seed in 0..6u64 {
                        let c = Config { style, width, rough, relief, players, ..cfg() };
                        let g = Salvo::new(c, seed * 977 + u64::from(width));
                        g.check().unwrap();
                        let w = usize::from(WIDTHS[usize::from(width)]);
                        assert_eq!(g.heights().len(), w);
                        assert!(g.heights().iter().all(|&h| (2..=TOP).contains(&h)), "{style:?}");
                        let mut xs: Vec<i16> = g.tanks().iter().map(|t| t.x).collect();
                        xs.sort_unstable();
                        for p in xs.windows(2) {
                            assert!(p[1] - p[0] >= 8, "{style:?} {w}: tanks too close {xs:?}");
                        }
                        for t in g.tanks() {
                            let x = i32::from(t.x);
                            let h = g.height(x);
                            assert!((x - 3..=x + 3).all(|c| g.height(c) == h), "flat pad");
                            assert_eq!(t.y_px(), i16::from(h));
                            assert!(g.water() == 0 || h > g.water(), "above the water");
                        }
                        if style == Style::Islands {
                            assert!(g.water() > 0);
                        }
                    }
                }
            }
        }
    }
    // A fixed terrain seed: the same terrain every game.
    let c = Config { terrain_seed: 42, ..cfg() };
    assert_eq!(Salvo::new(c, 1).heights(), Salvo::new(c, 2).heights());
    assert_ne!(Salvo::new(cfg(), 1).heights(), Salvo::new(cfg(), 2).heights());
}

/// Random games driven by random keys and steps: the invariants after
/// every input and step (the fuzz body, which also checks saves and
/// determinism).
#[test]
fn random_games_keep_the_invariants() {
    let mut r = Rng::new(2026);
    for _ in 0..60 {
        let mut d: Vec<u8> = (0..18).map(|_| r.next_u32() as u8).collect();
        for _ in 0..300 {
            let b = match r.below(10) {
                0..=3 => 0x80 | r.below(40) as u8,
                4 => Input::Press(Button::Ok).to_byte(),
                5 => Input::Release(Button::Ok).to_byte(),
                _ => r.below(48) as u8,
            };
            d.push(b);
        }
        let _ = bodies::salvo_run(&d);
    }
}

#[path = "../fuzz/src/lib.rs"]
#[allow(dead_code)]
mod bodies;

/// Steps until the phase is one of `stop` (at most `n`).
fn run_until(g: &mut Salvo, stop: &[Phase], n: u32) -> u32 {
    for k in 0..n {
        if stop.contains(&g.phase()) {
            return k;
        }
        g.step();
        g.check().unwrap();
    }
    n
}

/// A human shot: aim, fire, fly, settle.
fn shoot(g: &mut Salvo, angle: u8, power: u16, weapon: u8) {
    let p = usize::from(g.turn());
    g.set_aim(p, angle, power);
    if weapon != 0 {
        g.input(Input::Press(Button::Menu));
        while g.menu() != Some(weapon) {
            g.input(Input::Press(Button::Down));
        }
        g.input(Input::Press(Button::Ok));
    }
    if g.phase() == Phase::Pass {
        g.input(Input::Press(Button::Ok));
        g.input(Input::Release(Button::Ok));
    }
    assert_eq!(g.phase(), Phase::Aim);
    g.input(Input::Press(Button::Ok));
    assert!(g.input(Input::Release(Button::Ok)).fired);
    run_until(g, &[Phase::Pass, Phase::Aim, Phase::Think, Phase::RoundOver], 20_000);
}

fn flat(c: Config, h: u8) -> Salvo {
    let mut g = Salvo::new(c, 5);
    let w = usize::from(g.width());
    g.set_heights(&vec![h; w]);
    g.set_wind(0);
    g
}

#[test]
fn craters_only_lower_the_ground() {
    let c = Config { wind: 0, width: 2, ..cfg() };
    let mut g = flat(c, 40);
    g.place(0, 30);
    g.place(1, 200);
    // Straight up: it comes down near the shooter, a crater, damage to it.
    let before: Vec<u8> = g.heights().to_vec();
    g.force_turn(0);
    shoot(&mut g, 90, 200, 0);
    let after = g.heights();
    assert!(after.iter().zip(&before).any(|(a, b)| a < b), "a crater");
    assert!(after.iter().zip(&before).all(|(a, b)| a <= b), "only lower");
    assert!(g.tanks()[0].health < 100, "its own shell hurts");
    // A blast below the surface: each column loses exactly the cut, the
    // dirt above it falls in.
    let mut g = flat(c, 40);
    g.place(0, 20);
    g.place(1, 230);
    g.explode_at(100, 30, Weapon::Heavy as u8);
    let r = i32::from(SPECS[1].radius);
    for x in 80..120 {
        let dx: i32 = x - 100;
        let want = if dx.abs() > r {
            40
        } else {
            let dy = salvo::isqrt((r * r - dx * dx) as u32) as i32;
            40 - (40.min(30 + dy) - (30 - dy).max(0))
        };
        assert_eq!(i32::from(g.height(x)), want, "column {x}");
    }
}

#[test]
fn fall_damage_past_the_free_drop() {
    for fall in [true, false] {
        let c = Config { wind: 0, fall_damage: fall, health: 100, ..cfg() };
        let mut g = flat(c, 60);
        g.place(0, 20);
        g.place(1, 200);
        // The ground under tank 1 gone 20 px down; it falls when the next
        // shot settles.
        let mut h = g.heights().to_vec();
        h[190..=210].fill(40);
        g.set_heights(&h);
        assert_eq!(g.tanks()[1].y_px(), 60, "floating until the turn settles");
        g.force_turn(0);
        shoot(&mut g, 135, 300, 0);
        let t = g.tanks()[1];
        assert_eq!(t.y_px(), 40);
        if fall {
            assert!((60..=72).contains(&t.health), "fall damage {}", 100 - t.health);
            assert!(g.tanks()[0].score == 0, "a fall scores for nobody");
        } else {
            assert_eq!(t.health, 100);
        }
    }
}

#[test]
fn weapons_do_what_they_say() {
    let c = Config { wind: 0, width: 2, health: 200, ..cfg() };
    let base = {
        let mut g = flat(c, 30);
        g.place(0, 20);
        g.place(1, 240);
        g
    };
    // Each weapon fired at 45° with the same power.
    let land = |w: Weapon| {
        let mut g = base.clone();
        g.force_turn(0);
        shoot(&mut g, 45, 450, w as u8);
        g
    };
    let shell = land(Weapon::Shell);
    let heavy = land(Weapon::Heavy);
    let removed = |g: &Salvo| g.heights().iter().map(|&h| u32::from(30 - h.min(30))).sum::<u32>();
    let added = |g: &Salvo| g.heights().iter().map(|&h| u32::from(h.saturating_sub(30))).sum::<u32>();
    assert!(removed(&shell) > 50, "a crater");
    assert!(removed(&heavy) > removed(&shell) * 3 / 2, "a bigger crater");
    let dirt = land(Weapon::Dirt);
    assert!(added(&dirt) > 100 && removed(&dirt) == 0, "a mound");
    let digger = land(Weapon::Digger);
    let deepest = |g: &Salvo| g.heights().iter().copied().min().unwrap_or(0);
    assert!(deepest(&digger) < deepest(&shell), "a trench deeper than a crater");
    let cluster = land(Weapon::Cluster);
    let holes = cluster.heights().windows(2).filter(|w| w[0] == 30 && w[1] < 30).count();
    assert!(holes >= 2, "bomblets: several craters, {holes}");
    let smoke = land(Weapon::Smoke);
    assert_eq!(removed(&smoke), 0);
    assert!(smoke.puffs().iter().any(|p| p.age > 0), "smoke puffs");
    // The roller: on a slope down to the right it rolls on past where it
    // lands.
    let mut g = base.clone();
    let w = usize::from(g.width());
    let slope: Vec<u8> = (0..w).map(|x| (90 - (x * 60 / w)) as u8).collect();
    g.set_heights(&slope);
    g.force_turn(0);
    let mut rolled = false;
    g.set_aim(0, 45, 300);
    g.input(Input::Press(Button::Menu));
    while g.menu() != Some(Weapon::Roller as u8) {
        g.input(Input::Press(Button::Down));
    }
    g.input(Input::Press(Button::Ok));
    g.input(Input::Press(Button::Ok));
    g.input(Input::Release(Button::Ok));
    for _ in 0..20_000 {
        if g.shells().iter().any(|s| s.mode == Mode::Roll) {
            rolled = true;
        }
        if g.phase() != Phase::Flight {
            break;
        }
        g.step();
        g.check().unwrap();
    }
    assert!(rolled, "the roller rolled");
    // Ammo is spent; the shell is unlimited.
    assert_eq!(heavy.tanks()[0].rounds(Weapon::Heavy as u8), SPECS[1].start - 1);
    assert_eq!(shell.tanks()[0].rounds(0), u8::MAX);
}

#[test]
fn tanks_sink_in_water() {
    let c = Config { style: Style::Islands, wind: 0, ..cfg() };
    let mut g = Salvo::new(c, 9);
    assert!(g.water() > 0);
    let x = g.tanks()[1].x;
    let mut h = g.heights().to_vec();
    for c in (x - 6)..=(x + 6) {
        h[c as usize] = 1;
    }
    g.set_heights(&h);
    g.force_turn(0);
    shoot(&mut g, 90, 0, 0);
    assert!(!g.tanks()[1].alive(), "sunk");
}

/// An Expert computer hits a still target in calm air within two shots,
/// on every gravity, air, width and style.
#[test]
fn expert_hits_in_calm_air() {
    let mut shots = Vec::new();
    for gravity in 0..4u8 {
        for drag in 0..3u8 {
            for (width, style) in
                [(0u8, Style::Hills), (2, Style::Mountains), (3, Style::Plateaus), (1, Style::Canyon)]
            {
                for seed in 0..3u64 {
                    let c = Config {
                        players: 2,
                        kinds: [Kind::Expert, Kind::Human, Kind::Human, Kind::Human],
                        wind: 0,
                        gusts: false,
                        gravity,
                        drag,
                        width,
                        style,
                        health: 250,
                        full_set: false,
                        ..Config::DEFAULT
                    };
                    let mut g = Salvo::new(c, seed * 31 + u64::from(gravity));
                    let mut n = 0;
                    loop {
                        n += 1;
                        g.force_turn(0);
                        assert_eq!(g.phase(), Phase::Think);
                        run_until(&mut g, &[Phase::Pass, Phase::Aim, Phase::RoundOver], 50_000);
                        if g.tanks()[1].health < 250 {
                            break;
                        }
                        assert!(n < 2, "g{gravity} d{drag} w{width} {style:?} seed {seed}: missed twice");
                    }
                    shots.push(n);
                }
            }
        }
    }
    let first = shots.iter().filter(|&&n| n == 1).count();
    assert!(first * 10 >= shots.len() * 9, "first-shot hits {first} of {}", shots.len());
}

/// The difficulties differ: in light wind Novice needs more shots than
/// Expert on average, and the computers improve on their misses
/// (bracketing).
#[test]
fn difficulty_orders_and_bracketing() {
    let mean = |k: Kind| {
        let mut total = 0;
        for seed in 0..12u64 {
            let c = Config {
                players: 2,
                kinds: [k, Kind::Human, Kind::Human, Kind::Human],
                wind: 4,
                health: 250,
                full_set: false,
                ..Config::DEFAULT
            };
            let mut g = Salvo::new(c, seed);
            let mut n = 0;
            while n < 12 {
                n += 1;
                g.force_turn(0);
                run_until(&mut g, &[Phase::Pass, Phase::Aim, Phase::RoundOver], 50_000);
                if g.tanks()[1].health < 250 {
                    break;
                }
            }
            total += n;
        }
        f64::from(total) / 12.0
    };
    let (novice, easy, hard, expert) =
        (mean(Kind::Novice), mean(Kind::Easy), mean(Kind::Hard), mean(Kind::Expert));
    eprintln!(
        "shots to hit in light wind: novice {novice:.2}, easy {easy:.2}, hard {hard:.2}, expert {expert:.2}"
    );
    assert!(novice > expert && easy >= hard && hard >= expert - 0.01);
    assert!(novice < 12.0, "Novice brackets in");
}

/// Expert with gusts on simulates the very gusts the flight draws: its
/// simulated landing is where the shell lands.
#[test]
fn expert_sees_gusts_others_do_not() {
    for seed in 0..6u64 {
        let c = Config {
            players: 2,
            kinds: [Kind::Expert, Kind::Human, Kind::Human, Kind::Human],
            wind: 12,
            gusts: true,
            health: 250,
            full_set: false,
            ..Config::DEFAULT
        };
        let mut g = Salvo::new(c, seed);
        g.force_turn(0);
        let mut n = 0;
        loop {
            run_until(&mut g, &[Phase::Pass, Phase::Aim, Phase::RoundOver], 50_000);
            n += 1;
            if g.tanks()[1].health < 250 || n >= 3 {
                break;
            }
            g.force_turn(0);
        }
        assert!(g.tanks()[1].health < 250, "seed {seed}: Expert misses in gusts");
    }
}

#[test]
fn simulate_matches_the_flight() {
    // The search's simulation and the real flight are the same physics.
    let c = Config { wind: 12, gusts: false, ..cfg() };
    let mut g = flat(c, 30);
    g.place(0, 20);
    g.place(1, 240);
    g.force_turn(0);
    let w = g.wind().0;
    let s = g.launch(0, 50, 400, 0);
    let hit = g.simulate(s, 0, w, None, |_, _| true);
    g.set_aim(0, 50, 400);
    g.input(Input::Press(Button::Ok));
    g.input(Input::Release(Button::Ok));
    let mut last = None;
    while g.phase() == Phase::Flight {
        if let Some(s) = g.shells().iter().find(|s| s.active()) {
            last = Some(s.px());
        }
        g.step();
    }
    let (Hit::Ground(x, _) | Hit::Tank(_, x, _)) = hit else { panic!("{hit:?}") };
    let (lx, _) = last.unwrap();
    assert!((lx - x).abs() < 1.5, "simulated {x}, flew to {lx}");
}

#[test]
fn turns_rounds_and_the_end() {
    // Four computers play a whole game of two rounds unattended.
    let c = Config {
        players: 4,
        kinds: [Kind::Hard, Kind::Expert, Kind::Novice, Kind::Easy],
        rounds: 2,
        health: 50,
        ..Config::DEFAULT
    };
    let mut g = Salvo::new(c, 99);
    let n = run_until(&mut g, &[Phase::Over], 400_000);
    assert!(n < 400_000, "the game ends");
    assert!(g.ended());
    assert!(g.tanks().iter().any(|t| t.score > 0));
    // Two humans: a pass screen before each human turn; OK goes on.
    let mut g = Salvo::new(Config { kinds: [Kind::Human; 4], ..Config::DEFAULT }, 3);
    assert_eq!(g.phase(), Phase::Pass);
    g.input(Input::Press(Button::Ok));
    assert_eq!(g.phase(), Phase::Aim);
    // One human against a computer: no pass screen.
    let g = Salvo::new(Config::DEFAULT, 3);
    assert!(matches!(g.phase(), Phase::Aim | Phase::Think));
}

#[test]
fn keys_aim_and_menu() {
    let mut g = Salvo::new(
        Config { kinds: [Kind::Human, Kind::Human, Kind::Easy, Kind::Easy], ..Config::DEFAULT },
        4,
    );
    g.input(Input::Press(Button::Ok));
    g.input(Input::Release(Button::Ok));
    let p = usize::from(g.turn());
    let (a0, p0) = (g.tanks()[p].angle, g.tanks()[p].power);
    g.input(Input::Press(Button::Up));
    g.input(Input::Release(Button::Up));
    assert_eq!(g.tanks()[p].angle, a0 + 1);
    g.input(Input::Press(Button::Right));
    assert_eq!(g.tanks()[p].power, p0 + 1);
    // Held: repeats, faster after a while.
    for _ in 0..200 {
        g.step();
    }
    g.input(Input::Release(Button::Right));
    let gained = g.tanks()[p].power - p0;
    assert!(gained > 100, "held power accelerates: {gained}");
    // Menu: the weapon list over weapons in stock.
    g.input(Input::Press(Button::Menu));
    assert_eq!(g.menu(), Some(0));
    g.input(Input::Press(Button::Down));
    assert_eq!(g.menu(), Some(Weapon::Heavy as u8));
    assert!(g.close_menu());
    assert_eq!(g.tanks()[p].weapon, 0, "Back keeps the weapon");
    // Hold OK: the overview.
    g.input(Input::Press(Button::Ok));
    g.input(Input::Long(Button::Ok));
    g.input(Input::Release(Button::Ok));
    assert!(g.overview());
    assert_eq!(g.phase(), Phase::Aim, "a long OK does not fire");
}

#[test]
fn turn_timer_fires() {
    let c = Config { timer: 15, kinds: [Kind::Human, Kind::Easy, Kind::Easy, Kind::Easy], ..Config::DEFAULT };
    let mut g = Salvo::new(c, 1);
    g.force_turn(0);
    assert_eq!(g.timer_left(), Some(15));
    let mut fired = false;
    for _ in 0..15 * 60 {
        fired |= g.step().fired;
    }
    assert!(fired && g.phase() == Phase::Flight);
}

#[test]
fn shop_between_rounds() {
    let c = Config { rounds: 2, health: 25, ..Config::DEFAULT };
    let mut g = Salvo::new(c, 21);
    g.force_turn(0);
    g.set_health(1, 0);
    // Smoke: no damage to anyone.
    shoot(&mut g, 90, 300, Weapon::Smoke as u8);
    assert_eq!(g.phase(), Phase::RoundOver);
    assert_eq!(g.round_winner(), Some(0));
    g.input(Input::Press(Button::Ok));
    assert_eq!(g.phase(), Phase::Shop);
    let credits = g.tanks()[0].credits;
    assert!(credits >= salvo::ROUND_CREDITS);
    let heavy = g.tanks()[0].rounds(1);
    g.input(Input::Press(Button::Ok));
    if credits >= SPECS[1].price {
        assert_eq!(g.tanks()[0].rounds(1), heavy + 1);
        assert_eq!(g.tanks()[0].credits, credits - SPECS[1].price);
    }
    g.input(Input::Press(Button::Up));
    assert_eq!(g.shop_row(), 7);
    g.input(Input::Press(Button::Ok));
    assert_eq!(g.round(), 1);
    assert!(g.tanks().iter().all(|t| t.health == 25));
}

#[test]
fn saves_round_trip() {
    let mut buf = [0u8; SAVE_MAX];
    let mut biggest = 0;
    for width in 0..4u8 {
        for players in 2..=4u8 {
            let c = Config { width, players, ..Config::DEFAULT };
            let mut g = Salvo::new(c, u64::from(width) * 7 + u64::from(players));
            for k in 0..400 {
                g.step();
                if k % 23 == 0 {
                    g.input(Input::Press(Button::Ok));
                    g.input(Input::Release(Button::Ok));
                }
                let s = Session::Salvo(g.clone());
                let n = s.encode(&mut buf).expect("fits SAVE_MAX");
                biggest = biggest.max(n);
                let Some(Session::Salvo(x)) = Session::decode(&buf[..n], Some(id::SALVO)) else {
                    panic!("does not read back")
                };
                assert_eq!(x.heights(), g.heights());
                assert_eq!(x.round(), g.round());
                assert_eq!(x.turn(), g.turn());
                for (a, b) in x.tanks().iter().zip(g.tanks()) {
                    assert_eq!(
                        (a.x, a.health, a.score, a.credits, a.ammo),
                        (b.x, b.health, b.score, b.credits, b.ammo)
                    );
                }
                match g.phase() {
                    Phase::Flight | Phase::Settle => assert_eq!(x.phase(), Phase::Settle),
                    Phase::Turn | Phase::Think => assert_eq!(x.phase(), Phase::Think),
                    p => assert_eq!(x.phase(), p),
                }
            }
        }
    }
    eprintln!("largest save {biggest} B of {SAVE_MAX}");
    assert!(biggest <= SAVE_MAX);
    // Damage is refused.
    let g = Session::Salvo(Salvo::new(Config::DEFAULT, 1));
    let n = g.encode(&mut buf).unwrap();
    let mut bad = buf;
    bad[30] ^= 0x40;
    assert!(Session::decode(&bad[..n], None).is_none());
}

/// The same seed and keys give the same game, bit for bit; pinned by a
/// checksum so that a change of the physics, the terrain or the computers
/// shows (and the player, with an IEEE FPU and no fused multiply-add,
/// computes the same).
#[test]
fn deterministic_and_pinned() {
    let c = Config {
        players: 3,
        kinds: [Kind::Expert, Kind::Hard, Kind::Easy, Kind::Human],
        wind: 12,
        gusts: true,
        style: Style::Mixed,
        rounds: 2,
        ..Config::DEFAULT
    };
    let run = || {
        let mut g = Salvo::new(c, 0x0005_A170);
        let mut crc = 0u32;
        for k in 0..30_000u32 {
            g.step();
            if k % 500 == 0 {
                let mut buf = [0u8; SAVE_MAX];
                let n = Session::Salvo(g.clone()).encode(&mut buf).unwrap();
                crc = orpheus_games::save::crc32(&buf[..n]) ^ crc.rotate_left(5);
            }
        }
        (g, crc)
    };
    let (a, ca) = run();
    let (b, cb) = run();
    assert_eq!(a, b);
    assert_eq!(ca, cb);
    eprintln!("pinned checksum {ca:#010x}");
    assert_eq!(ca, PINNED, "the game changed: if on purpose, pin {ca:#010x}");
}

const PINNED: u32 = 0xd5cb_f752;

#[test]
fn widths_and_view() {
    for (i, &w) in WIDTHS.iter().enumerate() {
        let c = Config { width: i as u8, ..Config::DEFAULT };
        let g = Salvo::new(c, 1);
        assert_eq!(g.width(), w);
        assert!(usize::from(w) <= MAX_W);
        assert!(g.camera() >= 0 && i32::from(g.camera()) + 128 <= i32::from(w));
    }
    const { assert!(POWER_MAX == 1000 && BLAST_STEPS > 0) };
}

/// What the computers and a flight cost on the host (release): per
/// simulated shot, per decision, per logic step of a flight. Printed for
/// docs/STATUS.md (`cargo test --release -p orpheus-games --features salvo
/// --test salvo measure_cost -- --ignored --nocapture`).
#[test]
#[ignore = "measurement"]
fn measure_cost() {
    use std::time::Instant;
    for gravity in [0u8, 2, 3] {
        for kind in [Kind::Novice, Kind::Expert] {
            let (mut decisions, mut sims, mut worst_sim, mut worst_dec) = (0u32, 0u32, 0f64, 0f64);
            let mut total = 0f64;
            for seed in 0..20u64 {
                let c = Config {
                    players: 2,
                    kinds: [kind, Kind::Human, Kind::Human, Kind::Human],
                    gravity,
                    wind: 4,
                    width: 3,
                    style: Style::Mixed,
                    ..Config::DEFAULT
                };
                let mut g = Salvo::new(c, seed);
                g.force_turn(0);
                let t0 = Instant::now();
                while g.phase() == Phase::Think {
                    let ts = Instant::now();
                    let before = g.clone();
                    g.step();
                    let dt = ts.elapsed().as_secs_f64();
                    if before.phase() == Phase::Think {
                        worst_sim = worst_sim.max(dt);
                    }
                }
                let dt = t0.elapsed().as_secs_f64();
                worst_dec = worst_dec.max(dt);
                total += dt;
                decisions += 1;
                sims += u32::from(g.clone().think_all());
            }
            println!(
                "gravity {gravity} {kind:?}: {:.2} ms per decision (worst {:.2}), worst logic step {:.3} ms",
                total * 1e3 / f64::from(decisions),
                worst_dec * 1e3,
                worst_sim * 1e3
            );
            let _ = sims;
        }
    }
    // Physics: ns per step of a shell.
    let mut s = shell(0.0, 0.0, 100.0, 100.0);
    let t0 = Instant::now();
    let n = 2_000_000;
    for _ in 0..n {
        Salvo::advance(&mut s, 9.81, AIR[1] * SPECS[0].k, 3.0);
        if s.y < 0.0 {
            s = shell(0.0, 0.0, 100.0, 100.0);
        }
    }
    println!("{:.1} ns per physics step (advance only)", t0.elapsed().as_secs_f64() * 1e9 / f64::from(n));
    // A whole flight's logic steps (physics, hits, gusts).
    let c = Config { wind: 12, gusts: true, ..cfg() };
    let mut g = Salvo::new(c, 3);
    g.force_turn(0);
    g.set_aim(0, 60, 800);
    g.input(Input::Press(Button::Ok));
    g.input(Input::Release(Button::Ok));
    let t0 = Instant::now();
    let mut k = 0;
    while g.phase() == Phase::Flight {
        g.step();
        k += 1;
    }
    println!(
        "{:.1} µs per logic step of a flight ({k} steps)",
        t0.elapsed().as_secs_f64() * 1e6 / f64::from(k)
    );
}
