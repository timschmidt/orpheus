//! Interactive 4× window (feature `window`; X11 through `minifb`, loaded at
//! run time). Real time drives the simulated clock.

use std::path::Path;
use std::time::{Duration, Instant as StdInstant};

use anyhow::{Context, Result};
use minifb::{Key as MKey, KeyRepeat, Window, WindowOptions};
use yoton_ui::Key;

use crate::Sim;

const SCALE: usize = 4;

fn map(k: MKey) -> Option<Key> {
    Some(match k {
        MKey::Up => Key::Up,
        MKey::Down => Key::Down,
        MKey::Left => Key::Prev,
        MKey::Right => Key::Next,
        MKey::Enter | MKey::Space => Key::Ok,
        MKey::Backspace | MKey::Escape => Key::Back,
        MKey::M => Key::Menu,
        MKey::Equal | MKey::NumPadPlus => Key::VolUp,
        MKey::Minus | MKey::NumPadMinus => Key::VolDown,
        MKey::P => Key::Power,
        _ => return None,
    })
}

pub fn run(mut sim: Sim, out: &Path) -> Result<()> {
    let (w, h) = (128 * SCALE, 128 * SCALE);
    let mut win = Window::new("yoton-sim", w, h, WindowOptions::default()).context("open window")?;
    win.set_target_fps(60);
    let mut buf = vec![0u32; w * h];
    let start = StdInstant::now();
    let base = sim.now.0;
    let mut shots = 0;
    while win.is_open() {
        let now = base + u64::try_from(start.elapsed().as_millis()).unwrap_or(0);
        for k in win.get_keys_pressed(KeyRepeat::No) {
            if k == MKey::S {
                shots += 1;
                std::fs::create_dir_all(out)?;
                let p = out.join(format!("window-{shots}.png"));
                std::fs::write(&p, crate::png_bytes(&sim.fb)?)?;
                println!("{}", p.display());
            } else if let Some(k) = map(k) {
                sim.press(k);
            }
        }
        for k in win.get_keys_released() {
            if let Some(k) = map(k) {
                sim.release(k);
            }
        }
        if now > sim.now.0 {
            sim.advance(now - sim.now.0);
        }
        for (i, px) in sim.fb.iter().enumerate() {
            let [r, g, b] = yoton_ui::color::to_rgb888(*px);
            let c = (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
            let (x, y) = (i % 128, i / 128);
            for dy in 0..SCALE {
                let row = (y * SCALE + dy) * w + x * SCALE;
                buf[row..row + SCALE].fill(c);
            }
        }
        win.update_with_buffer(&buf, w, h)?;
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}
