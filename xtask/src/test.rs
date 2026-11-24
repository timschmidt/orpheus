//! `xtask test [host|golden|pagelink|all]` (07 §3).

use anyhow::Result;

use crate::args::Args;
use crate::util::{cargo, root, run};

pub fn cmd(args: &mut Args) -> Result<String> {
    let which = args.next_positional().unwrap_or_else(|| "all".into());
    let bless = args.flag("bless");
    match which.as_str() {
        "host" => host(),
        "golden" => golden(bless),
        "pagelink" => crate::selftest::run(),
        "all" => {
            host()?;
            golden(false)?;
            crate::selftest::run()?;
            Ok("host, golden and pagelink tests passed".into())
        }
        other => anyhow::bail!("unknown test suite `{other}` (host, golden, pagelink, all)"),
    }
}

fn host() -> Result<String> {
    run(cargo().current_dir(root()).args(["test", "--workspace", "--quiet"]))?;
    Ok("host tests passed".into())
}

fn golden(bless: bool) -> Result<String> {
    // Golden images are ordinary tests of the simulator crate; `--bless`
    // asks them to rewrite their reference PNGs.
    let sim = root().join("sim");
    if !sim.join("Cargo.toml").exists() {
        return Ok("no simulator yet (golden tests not run)".into());
    }
    let mut c = cargo();
    c.current_dir(root()).args(["test", "--quiet", "--package", "yoton-sim", "--test", "golden"]);
    if bless {
        c.env("YOTON_BLESS", "1");
    }
    run(&mut c)?;
    Ok(if bless { "golden images re-blessed".into() } else { "golden tests passed".into() })
}
