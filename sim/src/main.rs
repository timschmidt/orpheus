//! `yoton-sim [--demo] [--script "ok down*3 shot:x"] [--out DIR] [--window]`
//!
//! Runs the `yoton-apps` screens, or with `--demo` the toolkit's widget demo.
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
use yoton_sim::{AppSim, DemoSim, Host, Sim};

fn main() -> Result<()> {
    let mut script = String::new();
    let mut out = PathBuf::from("target/yoton/shots");
    let mut window = false;
    let mut demo = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--script" | "-s" => script = args.next().context("--script needs a value")?,
            "--out" | "-o" => out = args.next().context("--out needs a value")?.into(),
            "--window" | "-w" => window = true,
            "--demo" => demo = true,
            "--help" | "-h" => {
                println!("yoton-sim [--script SCRIPT] [--out DIR] [--window]");
                return Ok(());
            }
            other => bail!("unknown argument `{other}`"),
        }
    }
    if demo {
        run(DemoSim::new_dev_font(), &script, &out, window)
    } else {
        run(AppSim::new_dev_font(), &script, &out, window)
    }
}

fn run<H: Host>(mut sim: Sim<H>, script: &str, out: &std::path::Path, window: bool) -> Result<()> {
    let shots = sim.run_script(script)?;
    if !shots.is_empty() {
        std::fs::create_dir_all(out)?;
        for s in &shots {
            let p = out.join(format!("{}.png", s.name));
            std::fs::write(&p, s.png()?)?;
            println!("{}", p.display());
        }
    }
    println!("screen: {}  frames: {}  dirty pixels: {}", sim.screen(), sim.frames, sim.dirty_pixels);
    if window {
        run_window(sim, out)?;
    }
    Ok(())
}

#[cfg(feature = "window")]
fn run_window<H: Host>(sim: Sim<H>, out: &std::path::Path) -> Result<()> {
    yoton_sim::window::run(sim, out)
}

#[cfg(not(feature = "window"))]
fn run_window<H: Host>(_sim: Sim<H>, _out: &std::path::Path) -> Result<()> {
    bail!("built without the `window` feature (cargo run -p yoton-sim --features window -- --window)")
}
