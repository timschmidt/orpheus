//! `cargo xtask tts-lexicon [--check] [--budget BYTES]` (TTS-1, 04 §8): the
//! text-to-speech exception lexicon.
//!
//! Downloads CMUdict (pinned commit, SHA-256 checked) and the Project
//! Gutenberg books whose word counts rank the words into
//! `target/orpheus/tts-data/` (once), measures the letter-to-sound rules
//! against CMUdict, keeps the most useful exceptions within the budget
//! (default 32 KiB) and writes, in `crates/orpheus-tts/`:
//!
//! * `src/lexicon.bin` — the lexicon the crate embeds;
//! * `data/eval.tsv` — the accuracy tests' fixture (a frequency-stratified
//!   sample of CMUdict with counts, and the forced vocabulary);
//! * `data/lexicon-report.txt` — the measurement: rules alone, rules +
//!   lexicon, and the accuracy/size curve.
//!
//! The work is `orpheus_tts::lexgen` (feature `build`). `--check`
//! regenerates in memory and fails if a committed file differs (after a
//! rule change the lexicon must be rebuilt).

use anyhow::{Result, anyhow, bail};

use crate::args::Args;
use crate::util::{log, out_dir, root};

pub fn cmd(args: &mut Args) -> Result<String> {
    let check = args.flag("check");
    let budget = match args.value("budget") {
        Some(b) => b.parse::<usize>().map_err(|e| anyhow!("--budget {b}: {e}"))?,
        None => orpheus_tts::lexgen::BUDGET,
    };
    let cache = out_dir().join("tts-data");
    log(&format!("tts-lexicon: data in {}", cache.display()));
    orpheus_tts::lexgen::fetch(&cache).map_err(|e| anyhow!(e))?;
    let ui = [root().join("crates/orpheus-apps/src"), root().join("crates/orpheus-ui/src")];
    let out = orpheus_tts::lexgen::run(&cache, &ui, budget).map_err(|e| anyhow!(e))?;
    let dir = root().join("crates/orpheus-tts");
    let files: [(&str, &[u8]); 3] = [
        ("src/lexicon.bin", &out.bin),
        ("data/eval.tsv", out.eval.as_bytes()),
        ("data/lexicon-report.txt", out.report.as_bytes()),
    ];
    print!("{}", out.report);
    if check {
        let stale: Vec<&str> = files
            .iter()
            .filter(|(p, d)| std::fs::read(dir.join(p)).ok().as_deref() != Some(*d))
            .map(|(p, _)| *p)
            .collect();
        if !stale.is_empty() {
            bail!("not current (run `cargo xtask tts-lexicon`): {}", stale.join(", "));
        }
        return Ok(format!("lexicon current ({} bytes)", out.bin.len()));
    }
    std::fs::create_dir_all(dir.join("data"))?;
    for (p, d) in files {
        std::fs::write(dir.join(p), d)?;
    }
    Ok(format!(
        "wrote crates/orpheus-tts/src/lexicon.bin ({} bytes), data/eval.tsv, data/lexicon-report.txt",
        out.bin.len()
    ))
}
