//! Key sequences in the format of the `ui_keys` fuzz corpus (07 §5.7,
//! `sim/fuzz/src/lib.rs`): a selector byte, then 3-byte operations
//! `[op, a, b]`. A sequence recorded in the browser is a corpus input: drop
//! it into `tests/regress/ui_keys/` (replayed by every `cargo test`) or
//! replay it on the player; a corpus input replays here.
//!
//! The recorder writes taps (`op 0`), holds (`op 6`, 200 + 20·b ms), key
//! edges (`op 7`/`op 8`, when keys overlap) and waits (`op 9`, 37·b + 1
//! ms). The replayer decodes every operation with the fuzz target's own
//! decoder (`command`), included from `sim/fuzz`.

/// The fuzz target's key order (`KEYS` in `sim/fuzz/src/lib.rs`).
pub const KEYS: [&str; 10] = ["up", "down", "prev", "next", "ok", "back", "menu", "vol+", "vol-", "power"];

/// Operations the fuzz target runs per input, and its time limit.
pub use crate::fuzz::{MAX_MS, MAX_OPS};

/// A key edge at a simulated time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub ms: u64,
    pub press: bool,
    pub key: u8,
}

/// Presses shorter than this are taps (the target's tap holds 60 ms).
const TAP_MS: u64 = 300;
/// Waits shorter than this are dropped (the shortest wait but one).
const MIN_WAIT: u64 = 38;

/// Index of a key name in [`KEYS`].
#[must_use]
pub fn key_index(name: &str) -> Option<u8> {
    KEYS.iter().position(|k| *k == name).and_then(|i| u8::try_from(i).ok())
}

fn wait(out: &mut Vec<u8>, mut ms: u64) {
    while ms >= MIN_WAIT {
        let b = ((ms - 1) / 37).min(255);
        out.extend([9, 0, b as u8]);
        ms -= b * 37 + 1;
    }
}

/// Encode edges recorded from `start` as a corpus input (selector 0: the
/// target compares a full redraw after every operation).
#[must_use]
pub fn encode(edges: &[Edge], start: u64) -> Vec<u8> {
    let mut out = vec![0u8];
    let mut t = start;
    let mut i = 0;
    while i < edges.len() {
        let e = edges[i];
        wait(&mut out, e.ms.saturating_sub(t));
        let next = edges.get(i + 1);
        match next {
            Some(r) if e.press && !r.press && r.key == e.key => {
                let d = r.ms - e.ms;
                if d < TAP_MS {
                    out.extend([0, e.key, 0]);
                    t = e.ms.max(t) + 100;
                } else {
                    let b = (d.saturating_sub(200) / 20).min(255);
                    out.extend([6, e.key, b as u8]);
                    t = e.ms.max(t) + 200 + b * 20 + 40;
                }
                i += 2;
            }
            _ => {
                out.extend([if e.press { 7 } else { 8 }, e.key, 0]);
                t = e.ms.max(t);
                i += 1;
            }
        }
    }
    out
}

/// Script commands of a corpus input, as the fuzz target runs them (at
/// most [`MAX_OPS`]).
#[must_use]
pub fn decode(data: &[u8]) -> Vec<String> {
    let Some((_, ops)) = data.split_first() else { return Vec::new() };
    ops.chunks_exact(3).take(MAX_OPS).map(|op| crate::fuzz::command(op[0], op[1], op[2])).collect()
}

/// Number of operations in an encoded input.
#[must_use]
pub fn ops(data: &[u8]) -> usize {
    data.len().saturating_sub(1) / 3
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(ms: u64, press: bool, key: &str) -> Edge {
        Edge { ms, press, key: key_index(key).unwrap() }
    }

    #[test]
    fn taps_holds_overlaps_and_waits() {
        let edges = [
            e(1000, true, "down"),
            e(1080, false, "down"),
            e(2000, true, "menu"),
            e(2800, false, "menu"),
            e(3000, true, "vol+"),
            e(3100, true, "ok"),
            e(3150, false, "vol+"),
            e(3200, false, "ok"),
        ];
        let data = encode(&edges, 0);
        let cmds = decode(&data);
        assert_eq!(
            cmds,
            [
                "wait:1000",
                "down",
                "wait:889",
                "hold:menu:800",
                "wait:149",
                "press:vol+",
                "wait:75",
                "press:ok",
                "wait:38",
                "release:vol+",
                "wait:38",
                "release:ok",
            ]
        );
    }

    #[test]
    fn long_waits_are_split() {
        let data = encode(&[e(20_000, true, "ok"), e(20_010, false, "ok")], 0);
        let cmds = decode(&data);
        let total: u64 =
            cmds.iter().filter_map(|c| c.strip_prefix("wait:")).map(|v| v.parse::<u64>().unwrap()).sum();
        assert!((19_980..=20_000).contains(&total), "{total}");
        assert_eq!(cmds.last().unwrap(), "ok");
    }

    #[test]
    fn a_corpus_input_replays() {
        let cmds = decode(&[3, 0, 4, 0, 6, 6, 25, 9, 0, 10, 14, 1, 0]);
        assert_eq!(cmds, ["ok", "hold:menu:700", "wait:371", "goto:Music"]);
    }
}
