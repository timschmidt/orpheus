//! The milestone gate (07 §6). Each step reports passed / FAILED / not run.
//!
//! `--coverage` adds `xtask cover` as a report-only step (the numbers, never
//! a failure for being below 100 %). `--m11` (the M11 gate, VER-1/VER-5)
//! adds coverage with the fuzz corpora and fails below 100 % of the
//! non-excluded regions, plus `test asan` and `test miri`. Without these
//! flags the gate is unchanged.

use anyhow::{Result, bail};

use crate::args::Args;
use crate::util::{cargo, log, root, run};

pub fn cmd(args: &mut Args) -> Result<String> {
    let skip_dev = args.flag("no-dev");
    let m11 = args.flag("m11");
    let coverage = args.flag("coverage") || m11;
    let mut results: Vec<(&str, String)> = Vec::new();
    let mut failed = false;
    let mut step = |name: &'static str, r: Result<String>| match r {
        Ok(s) => results.push((name, format!("passed ({s})"))),
        Err(e) => {
            failed = true;
            results.push((name, format!("FAILED: {e:#}")));
        }
    };
    step("fmt", run(cargo().current_dir(root()).args(["fmt", "--all", "--check"])).map(|()| String::new()));
    step(
        "fmt (fw)",
        run(cargo().current_dir(root().join("fw")).args(["fmt", "--all", "--check"])).map(|()| String::new()),
    );
    step(
        "clippy",
        run(cargo().current_dir(root()).args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--quiet",
            "--",
            "-D",
            "warnings",
        ]))
        .map(|()| String::new()),
    );
    step(
        "clippy (fw)",
        run(cargo().current_dir(root().join("fw")).args([
            "clippy",
            "--workspace",
            "--quiet",
            "--",
            "-D",
            "warnings",
        ]))
        .map(|()| String::new()),
    );
    step("test all", crate::test::cmd(&mut Args::from_vec(vec!["all".into()])));
    step("brand --check", crate::brand::cmd(&mut Args::from_vec(vec!["--check".into()])));
    step("site --check", crate::site::cmd(&mut Args::from_vec(vec!["--check".into()])));
    step("build --release", crate::build::cmd_build(&mut Args::from_vec(vec![])));
    step("size --check", crate::size::cmd(&mut Args::from_vec(vec!["--check".into()])));
    step("check-resident", crate::resident::cmd(&mut Args::from_vec(vec![])));
    if coverage {
        step("coverage", crate::cover::gate_step(m11));
    }
    if m11 {
        step("test asan", crate::sanitize::asan(false, false));
        step("test miri", crate::sanitize::miri(None));
    }
    if skip_dev {
        results.push(("dev hil smoke", "not run (--no-dev)".into()));
    } else {
        match crate::dev::cmd(&mut Args::from_vec(vec!["hil".into(), "smoke".into()])) {
            Ok(s) => results.push(("dev hil smoke", format!("passed ({s})"))),
            Err(e)
                if format!("{e:#}").contains("did not reach ADFU") || format!("{e:#}").contains("absent") =>
            {
                results.push(("dev hil smoke", format!("not run (player not reachable: {e:#})")));
            }
            Err(e) => {
                failed = true;
                results.push(("dev hil smoke", format!("FAILED: {e:#}")));
            }
        }
    }
    log("gate summary:");
    for (n, r) in &results {
        log(&format!("  {n:<16} {r}"));
    }
    if failed {
        bail!("gate failed");
    }
    Ok("gate passed (steps marked `not run` did not run)".into())
}
