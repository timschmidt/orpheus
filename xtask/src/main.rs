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
mod hot;
mod linkgraph;
mod linkmap;
mod metrics;
mod ofw;
mod pagelink;
mod profile;
mod resident;
mod rules;
mod sanitize;
mod savedlink;
mod selftest;
mod site;
mod size;
mod stack;
mod test;
mod thumb;
mod tts;
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
  stack [--check] [--top N] [--root PATTERN] [--elf PATH]
                                   static worst-case MSP depth per task, handler and
                                   in total with interrupt nesting (stack.toml);
                                   --check enforces budgets.toml's margin and the
                                   line budgets of hot.toml clusters with `from`
  hot [--name NAME] [--from PATTERN]… [--stop PATTERN]… [--lines N] [--fits-with A,B]
      [--top N] [--write]          the paged code reachable from PATTERN (static call
                                   graph): size by crate and cluster, the largest
                                   functions, a cluster of it within N lines (default
                                   8); --write puts it in hot.toml (07 §5.6)
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
  prof DIR [--update-hot NAME [--lines N] [--ro-lines N]] [--window-ms N] [--top N]
                                   report a capture of `dev prof` again (T-8, 07 §5.6;
                                   no player): against the current layout and hot.toml
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
  tts-lexicon [--check] [--budget BYTES]
                                   the speech engine's exception lexicon from CMUdict
                                   and Gutenberg word counts (TTS-1): src/lexicon.bin,
                                   data/eval.tsv, data/lexicon-report.txt in
                                   crates/orpheus-tts; --check fails if not current
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
        "stack" => stack::cmd(&mut args),
        "hot" => hot::cmd(&mut args),
        "test" => test::cmd(&mut args),
        "gate" => gate::cmd(&mut args),
        "cover" => cover::cmd(&mut args),
        "metrics" => metrics::cmd(&mut args),
        "dev" => dev::cmd(&mut args),
        "prof" => dev::prof::report_cmd(&mut args),
        "drain" => drain::cmd(&mut args),
        "brand" => brand::cmd(&mut args),
        "site" => site::cmd(&mut args),
        "wasm" => wasm::cmd(&mut args),
        "tts-lexicon" => tts::cmd(&mut args),
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

#[cfg(test)]
mod manifest_tests {
    /// 2026-11-25: `tests/settings/effects.toml` held unescaped quotes in
    /// three Salvo oracles and was not TOML (only searched as text then);
    /// the VER-13 gate parses it, so it must parse.
    #[test]
    fn regress_settings_manifest_is_valid_toml() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/settings/effects.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        let v: toml::Value = toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(v.as_table().is_some_and(|t| !t.is_empty()));
    }
}
