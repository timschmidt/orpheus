//! `yoton-sim [--script "ok down*3 shot:x"] [--out DIR] [--window]`
//!
//! Headless by default: runs the script and writes each `shot:` as
//! `DIR/<name>.png` (default `target/yoton/shots`). `--window` (needs the
//! `window` feature) opens a 4× window after the script: arrows = Up/Down/
//! Prev/Next, Enter = OK, Backspace/Esc = Back, M = Menu, +/- = volume,
//! P = Power, S = screenshot. `YOTON_FALLBACK_FON=<file>@<offset>` uses a
//! 33-byte-record font file (e.g. the vendor UNICODE.FON in a NOR dump) as
//! the fallback font instead of the Unifont subset.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use yoton_sim::Sim;

fn main() -> Result<()> {
    let mut script = String::new();
    let mut out = PathBuf::from("target/yoton/shots");
    let mut window = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--script" | "-s" => script = args.next().context("--script needs a value")?,
            "--out" | "-o" => out = args.next().context("--out needs a value")?.into(),
            "--window" | "-w" => window = true,
            "--help" | "-h" => {
                println!("yoton-sim [--script SCRIPT] [--out DIR] [--window]");
                return Ok(());
            }
            other => bail!("unknown argument `{other}`"),
        }
    }
    let mut sim = Sim::new_dev_font();
    let shots = sim.run_script(&script)?;
    if !shots.is_empty() {
        std::fs::create_dir_all(&out)?;
        for s in &shots {
            let p = out.join(format!("{}.png", s.name));
            std::fs::write(&p, s.png()?)?;
            println!("{}", p.display());
        }
    }
    println!("screen: {}  frames: {}  dirty pixels: {}", sim.screen(), sim.frames, sim.dirty_pixels);
    if window {
        run_window(sim, &out)?;
    }
    Ok(())
}

#[cfg(feature = "window")]
fn run_window(sim: Sim, out: &std::path::Path) -> Result<()> {
    yoton_sim::window::run(sim, out)
}

#[cfg(not(feature = "window"))]
fn run_window(_sim: Sim, _out: &std::path::Path) -> Result<()> {
    bail!("built without the `window` feature (cargo run -p yoton-sim --features window -- --window)")
}
