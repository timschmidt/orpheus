//! Replays every `traces/*.csv` (the `acc trace` format,
//! `orpheus_steps::trace`) labelled with `# truth=<steps>`: the count must
//! be within `# tolerance=<pct>` (default 5) of the truth, and exactly 0
//! for a trace whose truth is 0. Captures from the player go here with the
//! truth the walker counted; the committed `synthetic-*` files were made
//! by `tests/common` (they keep the parser and this path tested until real
//! captures exist).

use std::path::Path;

use orpheus_steps::trace::{header_value, parse_line};
use orpheus_steps::{Detector, Sensitivity};

#[test]
fn labelled_traces() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("traces");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "csv"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no traces in {}", dir.display());
    for p in files {
        let text = std::fs::read_to_string(&p).unwrap();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let Some(truth) = header_value(&text, "truth").and_then(|v| v.parse::<u32>().ok()) else {
            continue;
        };
        let rate = header_value(&text, "rate").and_then(|v| v.parse().ok()).unwrap_or(25);
        let tol: u32 = header_value(&text, "tolerance").and_then(|v| v.parse().ok()).unwrap_or(5);
        let mut d = Detector::new(rate, Sensitivity::Medium);
        let mut n = 0;
        let mut got = 0;
        for s in text.lines().filter_map(parse_line) {
            got += d.feed(s.x, s.y, s.z);
            n += 1;
        }
        assert!(n > 0, "{name}: no samples");
        if truth == 0 {
            assert_eq!(got, 0, "{name}: false steps");
        } else {
            assert!(
                u64::from(got.abs_diff(truth)) * 100 <= u64::from(truth) * u64::from(tol),
                "{name}: counted {got}, truth {truth} (± {tol} %)"
            );
        }
    }
}
