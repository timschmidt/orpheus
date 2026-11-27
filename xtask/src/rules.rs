//! `resident.toml` and `hot.toml` (docs/plan/02-architecture.md §3.3–3.5).
//!
//! Patterns are globs (`*` any run of characters, `?` one character) over
//! demangled symbol names without hash (`atj2157_hal::rt::*`; a leading
//! `<` of a trait-method path is ignored, so `embassy_executor::raw::*`
//! matches `<embassy_executor::raw::Executor>::poll`), over section names
//! (`.text.resident.*`) or over input file names (`*libcompiler_builtins-*`).
//! A section matches a symbol pattern if any symbol defined in it matches.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::linkgraph::demangle;
use crate::linkmap::InputSection;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patterns {
    #[serde(default)]
    pub symbols: Vec<String>,
    #[serde(default)]
    pub sections: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
    /// Symbols that never match, whatever else does.
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl Patterns {
    fn extend(&mut self, o: &Self) {
        self.symbols.extend(o.symbols.iter().cloned());
        self.sections.extend(o.sections.iter().cloned());
        self.files.extend(o.files.iter().cloned());
        self.exclude.extend(o.exclude.iter().cloned());
    }

    /// Index of the first symbol pattern that selects `s` (excludes
    /// apply), if any.
    pub fn symbol_rank(&self, s: &InputSection) -> Option<usize> {
        s.symbols
            .iter()
            .filter_map(|sym| {
                let name = demangle(&sym.name);
                if self.exclude.iter().any(|p| symbol_matches(p, &name)) {
                    return None;
                }
                self.symbols.iter().position(|p| symbol_matches(p, &name))
            })
            .min()
    }

    /// The first pattern that selects `s`, as `kind:pattern`.
    pub fn select(&self, s: &InputSection) -> Option<String> {
        if let Some(p) = self.sections.iter().find(|p| glob(p, &s.name)) {
            return Some(format!("section:{p}"));
        }
        if let Some(p) = self.files.iter().find(|p| glob(p, &s.file)) {
            return Some(format!("file:{p}"));
        }
        for sym in &s.symbols {
            let name = demangle(&sym.name);
            if self.exclude.iter().any(|p| symbol_matches(p, &name)) {
                continue;
            }
            if let Some(p) = self.symbols.iter().find(|p| symbol_matches(p, &name)) {
                return Some(format!("symbol:{p}"));
            }
        }
        None
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BinRules {
    #[serde(default)]
    roots: Vec<String>,
    #[serde(default)]
    resident: Patterns,
    #[serde(default)]
    paged: Patterns,
    #[serde(default)]
    cold: Patterns,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RulesFile {
    #[serde(default)]
    roots: Vec<String>,
    #[serde(default)]
    vector_skip: Vec<usize>,
    #[serde(default)]
    resident: Patterns,
    #[serde(default)]
    paged: Patterns,
    #[serde(default)]
    cold: Patterns,
    #[serde(default)]
    bin: BTreeMap<String, BinRules>,
}

/// Rules for one binary (global entries plus its `[bin.NAME]` table).
#[derive(Clone, Debug, Default)]
pub struct Rules {
    /// Hard roots: `@vectors` (every vector-table entry except
    /// `vector_skip`) or symbol patterns.
    pub roots: Vec<String>,
    pub vector_skip: Vec<usize>,
    pub resident: Patterns,
    /// Forced into pages even when reachable from a root (check-resident
    /// then reports it).
    pub paged: Patterns,
    /// Paged code placed in the rodata window (cold code, PG-2: 02 §3.6)
    /// instead of the code window, unless a `hot.toml` cluster claims it or
    /// it is larger than a rodata group.
    pub cold: Patterns,
}

impl Rules {
    pub fn load(path: &Path, bin: &str) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&text, bin).with_context(|| format!("parse {}", path.display()))
    }

    pub fn parse(text: &str, bin: &str) -> Result<Self> {
        let f: RulesFile = toml::from_str(text)?;
        let mut r = Self {
            roots: f.roots,
            vector_skip: f.vector_skip,
            resident: f.resident,
            paged: f.paged,
            cold: f.cold,
        };
        if let Some(b) = f.bin.get(bin) {
            r.roots.extend(b.roots.iter().cloned());
            r.resident.extend(&b.resident);
            r.paged.extend(&b.paged);
            r.cold.extend(&b.cold);
        }
        Ok(r)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cluster {
    pub name: String,
    /// Binaries the cluster applies to (all when empty).
    #[serde(default)]
    pub bins: Vec<String>,
    #[serde(default)]
    pub symbols: Vec<String>,
    #[serde(default)]
    pub sections: Vec<String>,
    /// Clusters that run together with this one (the same wake-up): none
    /// of them may share a physical line of the direct-mapped code cache
    /// with this one or with each other (`pagelink` warns).
    #[serde(default)]
    pub fits_with: Vec<String>,
    /// Start on the physical code-cache line the named cluster (placed
    /// before this one) starts on: `pagelink` leaves the code window empty
    /// up to there (at most 31 lines). For a cluster that must share
    /// lines with that one only (one decoder's frame code over another's).
    #[serde(default)]
    pub starts_over: Option<String>,
    /// Entry points of the code the cluster is for (`xtask hot`, symbol
    /// patterns): with them, `xtask stack --check` checks that the paged
    /// code reachable from them (not entering `stop`) and not in a
    /// `fits_with` cluster stays within `budget_lines`.
    #[serde(default)]
    pub from: Vec<String>,
    #[serde(default)]
    pub stop: Vec<String>,
    #[serde(default)]
    pub budget_lines: Option<u32>,
    /// Constants of at most this many bytes that the cluster's code reads
    /// directly (relocation edges; plain data, no pointers, not a merged
    /// section) are resident instead of paged (PG-2b): small per-call
    /// constants on the physical lines of a big table the cluster streams
    /// through (the synth's wavetable takes 14 of the 16 rodata lines)
    /// would be reloaded on every call.
    #[serde(default)]
    pub resident_constants: Option<u32>,
}

impl Cluster {
    /// Position of the first pattern selecting `s` (packing order).
    pub fn rank(&self, s: &InputSection) -> Option<usize> {
        let by_section = self.sections.iter().position(|p| glob(p, &s.name));
        let by_symbol = s
            .symbols
            .iter()
            .filter_map(|y| {
                let n = demangle(&y.name);
                self.symbols.iter().position(|p| symbol_matches(p, &n))
            })
            .min()
            .map(|i| i + self.sections.len());
        by_section.or(by_symbol)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct HotFile {
    #[serde(default)]
    cluster: Vec<Cluster>,
}

/// The clusters of `hot.toml` that apply to `bin`, in file order.
pub fn load_hot(path: &Path, bin: &str) -> Result<Vec<Cluster>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let f: HotFile = toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    Ok(f.cluster.into_iter().filter(|c| c.bins.is_empty() || c.bins.iter().any(|b| b == bin)).collect())
}

/// Every cluster of a `hot.toml` text, in file order.
pub fn parse_hot(text: &str) -> Result<Vec<Cluster>> {
    let f: HotFile = toml::from_str(text)?;
    Ok(f.cluster)
}

/// A pattern of `hot.toml` (`stop`) or `stack.toml` (`targets`) that may
/// match nothing: written with a leading `?` (the code of a firmware
/// feature that a build may leave out, PG-2). Returns whether it may, and
/// the pattern without the mark.
#[must_use]
pub fn optional_pattern(p: &str) -> (bool, &str) {
    match p.strip_prefix('?') {
        Some(rest) => (true, rest),
        None => (false, p),
    }
}

pub fn symbol_matches(pat: &str, demangled: &str) -> bool {
    glob(pat, demangled) || glob(pat, demangled.trim_start_matches('<'))
}

/// Glob match: `*` any run (also empty), `?` one character.
pub fn glob(pat: &str, s: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pat.chars().collect(), s.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linkmap::MapSymbol;

    fn sec(name: &str, file: &str, syms: &[&str]) -> InputSection {
        InputSection {
            out: ".text".into(),
            file: file.into(),
            name: name.into(),
            addr: 0x1000,
            size: 4,
            align: 2,
            symbols: syms.iter().map(|s| MapSymbol { addr: 0x1001, size: 4, name: (*s).into() }).collect(),
        }
    }

    #[test]
    fn globbing() {
        assert!(glob("a*c", "abc"));
        assert!(glob("a*c", "ac"));
        assert!(glob("*", ""));
        assert!(glob("a?c", "abc"));
        assert!(!glob("a?c", "ac"));
        assert!(glob("orpheus_pager::*", "orpheus_pager::PagerCore::fault"));
        assert!(!glob("orpheus_pager::*", "orpheus_pagers::x"));
        assert!(glob("*::raw::*::poll", "embassy_executor::raw::TaskStorage<F>::poll"));
        assert!(symbol_matches("embassy_executor::raw::*", "<embassy_executor::raw::Executor>::poll"));
    }

    #[test]
    fn rules_select_by_symbol_section_file_with_excludes() {
        let r = Rules::parse(
            r#"
roots = ["@vectors"]
vector_skip = [0, 1]
[resident]
symbols = ["embassy_executor::raw::*", "memcpy"]
sections = [".text.resident.*"]
files = ["*libcompiler_builtins-*"]
exclude = ["*::TaskStorage<*>::poll"]
[bin.toy]
roots = ["toy_memmanage"]
[bin.toy.paged]
symbols = ["*planted*"]
[bin.toy.cold]
symbols = ["toy::settings::*"]
exclude = ["toy::settings::hot"]
"#,
            "toy",
        )
        .unwrap();
        assert_eq!(r.roots, ["@vectors", "toy_memmanage"]);
        assert_eq!(r.vector_skip, [0, 1]);
        let pick = |s: &InputSection| r.resident.select(s);
        assert_eq!(pick(&sec(".text.x", "a.o", &["memcpy"])).as_deref(), Some("symbol:memcpy"));
        assert_eq!(pick(&sec(".text.resident.f", "a.o", &[])).as_deref(), Some("section:.text.resident.*"));
        assert_eq!(
            pick(&sec(".text.y", "/x/libcompiler_builtins-1234.rlib(m.o)", &[])).as_deref(),
            Some("file:*libcompiler_builtins-*")
        );
        assert!(
            pick(&sec(".text.z", "a.o", &["<embassy_executor::raw::TaskStorage<x::F>>::poll"])).is_none()
        );
        assert!(pick(&sec(".text.z", "a.o", &["<embassy_executor::raw::SyncExecutor>::poll"])).is_some());
        assert!(r.paged.select(&sec(".text.p", "a.o", &["toy::planted_violation"])).is_some());
        assert!(r.cold.select(&sec(".text.c", "a.o", &["toy::settings::draw"])).is_some());
        assert!(r.cold.select(&sec(".text.c", "a.o", &["toy::settings::hot"])).is_none());
        assert!(
            Rules::parse("roots = []\n", "other")
                .unwrap()
                .paged
                .select(&sec(".t", "a", &["planted"]))
                .is_none()
        );
    }

    #[test]
    fn optional_patterns() {
        assert_eq!(optional_pattern("?*orpheus_tts::*"), (true, "*orpheus_tts::*"));
        assert_eq!(optional_pattern("*orpheus_tts::*"), (false, "*orpheus_tts::*"));
    }

    /// PG-2b, 2026-11-27: a firmware update from a PG-2 image (cold code in
    /// the rodata window) took 8–9 min to write 1.3 MB (37.9 s before) and
    /// the player reset during the read-back: SHA-256's round loop ran cold
    /// and straddled the physical line of its round constants (two line
    /// loads per round), and a 16 KiB read-back step outlasted the
    /// watchdog. The update's per-byte and per-block code must never be
    /// cold in the real `resident.toml`; its screens and checks may.
    #[test]
    fn regress_update_data_path_ran_from_the_rodata_window() {
        let r = Rules::load(&crate::util::root().join("resident.toml"), "firmware").unwrap();
        let cold = |sym: &str| r.cold.select(&sec(".text.x", "f.o", &[sym])).is_some();
        for hot in [
            "<orpheus_update::install::Installer>::write::<firmware::update::Nor>",
            "<orpheus_update::install::Installer>::take::<firmware::update::Nor>",
            "<orpheus_update::install::Installer>::same::<firmware::update::Nor>",
            "<orpheus_update::install::Installer>::program::<firmware::update::Nor>",
            "<orpheus_update::install::Installer>::erase::<firmware::update::Nor>",
            "<orpheus_update::install::Installer>::verify_step::<firmware::update::Nor>",
            "<orpheus_update::install::Installer>::guard",
            "<orpheus_update::slots::Target>::allows",
            "<firmware::update::Nor as orpheus_update::install::Flash>::read",
            "firmware::update::usb_out",
            "firmware::update::poll",
            "firmware::update::install_file::{closure#0}",
            "<sha2::Sha256 as digest::Update>::update",
            "sha2::sha256::compress256",
        ] {
            assert!(!cold(hot), "{hot} is cold: the update's data path must stay in the code window");
        }
        for rare in [
            "orpheus_update::ofw::check",
            "<orpheus_update::install::Installer>::commit::<firmware::update::Nor>",
            "<orpheus_apps::screens::update::UpdateScreen>::notice",
            "sha2::sha512::compress512",
            "sha1::block_api::compress",
        ] {
            assert!(cold(rare), "{rare} should stay cold");
        }
    }

    /// PG-2b, 2026-11-27: at an idle Home the PG-2 image loaded ≈ 200
    /// lines/s (≈ 120 before): `Fw::setting` (physical code lines 1–2)
    /// and `SettingsCache::get` (line 2) evicted each other on every one
    /// of the UI loop's ≈ 40 setting reads. The real `hot.toml` keeps a
    /// setting's read path in one cluster (never conflicting with itself).
    #[test]
    fn regress_setting_reads_evicted_each_other() {
        let hot = load_hot(&crate::util::root().join("hot.toml"), "firmware").unwrap();
        let c = hot.iter().find(|c| c.name == "settings-get").expect("cluster settings-get");
        for sym in [
            "<firmware::services::Fw as orpheus_apps::services::SettingsService>::setting",
            "<orpheus_apps::settings::SettingsCache>::get",
            "orpheus_apps::settings::index",
            "orpheus_apps::settings::def",
        ] {
            assert!(c.rank(&sec(".text.x", "f.o", &[sym])).is_some(), "{sym} not in settings-get");
        }
    }

    #[test]
    fn cluster_rank_follows_pattern_order() {
        let c = Cluster {
            name: "c".into(),
            bins: vec![],
            symbols: vec!["a::second".into(), "a::first*".into()],
            sections: vec![".text.zero".into()],
            ..Cluster::default()
        };
        assert_eq!(c.rank(&sec(".text.zero", "f", &[])), Some(0));
        assert_eq!(c.rank(&sec(".text.q", "f", &["a::second"])), Some(1));
        assert_eq!(c.rank(&sec(".text.r", "f", &["a::first_one"])), Some(2));
        assert_eq!(c.rank(&sec(".text.s", "f", &["b::x"])), None);
    }
}
