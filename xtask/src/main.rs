//! `cargo xtask …` — build, size, test and device commands
//! (docs/plan/07-tooling-testing.md §2).
//!
//! Every command is non-interactive, exits non-zero on failure and prints a
//! one-line verdict last.

mod args;
mod brand;
mod build;
mod cover;
mod dev;
mod drain;
mod gate;
mod linkgraph;
mod linkmap;
mod metrics;
mod ofw;
mod pagelink;
mod resident;
mod rules;
mod sanitize;
mod savedlink;
mod selftest;
mod site;
mod size;
mod test;
mod thumb;
mod util;
mod wasm;

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
  test fuzz-seeds                  rewrite the committed seed inputs of every fuzz
                                   target (tests/fuzz-corpus/<target>/seed-*)
  test asan [--fuzz-only] [--lean] host tests and every fuzz target's corpus under
                                   AddressSanitizer + LeakSanitizer (nightly)
  test miri [--crate NAME]         Miri on the crates listed in xtask/src/sanitize.rs
  cover [--crate NAME] [--fuzz|--fuzz-only] [--html] [--clean] [--report-only] [--add]
                                   LLVM source-based coverage (line, region, branch)
                                   of crates/, sim/, xtask/: target/orpheus/coverage/
  metrics [--elf PATH]             append this commit's coverage and firmware
                                   size/stack rows to metrics/*.csv
  gate [--no-dev] [--coverage] [--m11]
                                   the milestone gate (07 §6); --coverage adds the
                                   coverage report, --m11 enforces 100 % + ASan + Miri
  dev <subcommand>                 talk to the player (see `cargo xtask dev help`)
  drain report FILE.csv [FILE…]    battery-logger files (PWR-5): duration, mV drop,
                                   mV/h (end points and fit), time per clock level
                                   and playing, a comparison table
  brand [--check]                  palette contrast (web and RGB565) and the generated
                                   brand assets in docs/brand/generated/ (BRAND-1);
                                   --check fails if they are not current
  site [--check]                   the website into target/orpheus/site/ from site/ and
                                   the Markdown in docs/user, docs/dev (WEB-1); --check:
                                   internal links, screenshots, page limits
  site shots [--check]             docs/shots/: the golden images the documentation
                                   references, scaled x2 (DOC-2)
  wasm [--demo] [--record] [--test [--browser chromium|firefox]]
                                   Orpheus in the browser (WEB-2) into
                                   target/orpheus/site/try/pkg/ (pinned wasm-bindgen
                                   and wasm-opt), size budget; --demo regenerates
                                   sim/web/demo/ (ffmpeg), --record adds the sizes to
                                   metrics/static.csv, --test runs the page's self-test
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
        "drain" => drain::cmd(&mut args),
        "brand" => brand::cmd(&mut args),
        "site" => site::cmd(&mut args),
        "wasm" => wasm::cmd(&mut args),
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
