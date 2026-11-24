//! `cargo xtask …` — build, size, test and device commands
//! (docs/plan/07-tooling-testing.md §2).
//!
//! Every command is non-interactive, exits non-zero on failure and prints a
//! one-line verdict last.

mod args;
mod build;
mod cover;
mod dev;
mod gate;
mod linkgraph;
mod linkmap;
mod metrics;
mod pagelink;
mod resident;
mod rules;
mod savedlink;
mod selftest;
mod size;
mod test;
mod thumb;
mod util;

use std::process::ExitCode;

use args::Args;

const USAGE: &str = "\
usage: cargo xtask <command> [options]

  build [--debug] [--bin NAME] [--layout dual|full] [--features F]
                                   build the firmware (fw/) and pagelink it:
                                   target/orpheus/firmware.yfw, resident.bin,
                                   pages.bin, layout.json
  pagelink [--bin NAME] [--no-build] [--paged PATTERN,..]
                                   the two-pass paged link alone
  check-resident [--bin NAME]      prove nothing reachable from the roots is paged
  bringup NAME [--debug]           build a RAM-only bring-up image (fw/bringup/NAME)
  size [--check] [--bin NAME] [--elf PATH]
                                   size report; --check enforces budgets.toml
  test [host|golden|pagelink|codecs|fuzz-smoke|codec-size|all]
                                   host test suites (pagelink: the paged toy end to end)
  cover [--crate NAME] [--fuzz] [--html] [--clean] [--report-only]
                                   LLVM source-based coverage (line, region, branch)
                                   of crates/, sim/, xtask/: target/orpheus/coverage/
  metrics [--elf PATH]             append this commit's coverage and firmware
                                   size/stack rows to metrics/*.csv
  gate [--no-dev] [--coverage] [--m11]
                                   the milestone gate (07 §6); --coverage adds the
                                   coverage report, --m11 enforces 100 %
  dev <subcommand>                 talk to the player (see `cargo xtask dev help`)
";

fn main() -> ExitCode {
    let mut args = Args::from_env();
    let Some(cmd) = args.next_positional() else {
        eprint!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let r = match cmd.as_str() {
        "build" => build::cmd_build(&mut args),
        "bringup" => build::cmd_bringup(&mut args),
        "pagelink" => pagelink::cmd(&mut args),
        "check-resident" => resident::cmd(&mut args),
        "size" => size::cmd(&mut args),
        "test" => test::cmd(&mut args),
        "gate" => gate::cmd(&mut args),
        "cover" => cover::cmd(&mut args),
        "metrics" => metrics::cmd(&mut args),
        "dev" => dev::cmd(&mut args),
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        other => Err(anyhow::anyhow!("unknown command `{other}`\n{USAGE}")),
    };
    match r {
        Ok(verdict) => {
            println!("xtask {cmd}: {verdict}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("xtask {cmd}: FAILED: {e:#}");
            ExitCode::FAILURE
        }
    }
}
