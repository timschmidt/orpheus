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
}

impl Rules {
    pub fn load(path: &Path, bin: &str) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&text, bin).with_context(|| format!("parse {}", path.display()))
    }

    pub fn parse(text: &str, bin: &str) -> Result<Self> {
        let f: RulesFile = toml::from_str(text)?;
        let mut r = Self { roots: f.roots, vector_skip: f.vector_skip, resident: f.resident, paged: f.paged };
        if let Some(b) = f.bin.get(bin) {
            r.roots.extend(b.roots.iter().cloned());
            r.resident.extend(&b.resident);
            r.paged.extend(&b.paged);
        }
        Ok(r)
    }
}

#[derive(Clone, Debug, Deserialize)]
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
        assert!(
            Rules::parse("roots = []\n", "other")
                .unwrap()
                .paged
                .select(&sec(".t", "a", &["planted"]))
                .is_none()
        );
    }

    #[test]
    fn cluster_rank_follows_pattern_order() {
        let c = Cluster {
            name: "c".into(),
            bins: vec![],
            symbols: vec!["a::second".into(), "a::first*".into()],
            sections: vec![".text.zero".into()],
        };
        assert_eq!(c.rank(&sec(".text.zero", "f", &[])), Some(0));
        assert_eq!(c.rank(&sec(".text.q", "f", &["a::second"])), Some(1));
        assert_eq!(c.rank(&sec(".text.r", "f", &["a::first_one"])), Some(2));
        assert_eq!(c.rank(&sec(".text.s", "f", &["b::x"])), None);
    }
}
