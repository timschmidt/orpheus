//! `cargo xtask …` — build, size, test and device commands
//! (docs/plan/07-tooling-testing.md §2).
//!
//! Every command is non-interactive, exits non-zero on failure and prints a
//! one-line verdict last.

mod args;
mod build;
mod dev;
mod gate;
mod size;
mod test;
mod util;

use std::process::ExitCode;

use args::Args;

const USAGE: &str = "\
usage: cargo xtask <command> [options]

  build [--debug] [--bin NAME]     build the firmware (fw/) and its flat image
  bringup NAME [--debug]           build a RAM-only bring-up image (fw/bringup/NAME)
  size [--check] [--elf PATH]      size report; --check enforces budgets.toml
  test [host|golden|all]           host test suites
  gate                             the milestone gate (07 §6)
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
        "size" => size::cmd(&mut args),
        "test" => test::cmd(&mut args),
        "gate" => gate::cmd(&mut args),
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
