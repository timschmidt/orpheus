//! `cargo xtask settings-matrix [--check] [--strict] [--manifest PATH]`
//! (VER-13, 07 §5.12): setting → effect coverage.
//!
//! The settings come from the code (`orpheus_apps::settings::all()` with
//! every feature on: the groups, the apps' items, the games' and the
//! reading settings, packed fields and their storage words); the manifest
//! `tests/settings/effects.toml` says for each one where it shows (scope,
//! values, contexts, the contexts it must not touch, oracle) and which test
//! or on-device check shows it in each context (`cover`). The tool checks:
//!
//! * every setting of the tree has exactly one entry, and no entry names a
//!   setting the tree does not have;
//! * each entry has scope, values, default, contexts, oracle and tests;
//!   the values fit the setting's kind (all choices, a range's two ends)
//!   and the default is the code's;
//! * every context is declared in `[contexts]` (or a group of `[groups]`);
//!   a global setting accounts for every declared context (affected,
//!   unaffected, or `na` with a reason);
//! * every context, affected or unaffected, has a test or device check in
//!   `cover`, unless it is listed in `uncovered` with a reason; there is at
//!   least one covered "unaffected" context per setting (the effect is
//!   shown absent somewhere);
//! * every test named resolves (`path/file.rs::module::…::name`) to a
//!   `#[test]` function in that file that is not `#[ignore]`d; every
//!   `suite:<journey>` check is a journey of `tests/suite/journeys.toml`;
//! * persistence, defaults, clamping and migration have tests for every
//!   kind of setting the tree has (`[kinds]`).
//!
//! Effects only the player shows carry `device = true` and are counted
//! separately. A whole setting without an effect to show (not wired yet,
//! one value only) carries `gap = "…"`; it and the `uncovered` contexts are
//! reviewed exclusions: listed, counted against the coverage, accepted by
//! `--check`, refused by `--strict` (the M11 gate).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::args::Args;
use crate::util::root;

pub fn cmd(args: &mut Args) -> Result<String> {
    let check = args.flag("check");
    let strict = args.flag("strict");
    let manifest =
        args.value("manifest").map_or_else(|| root().join("tests/settings/effects.toml"), PathBuf::from);
    let m = load(&manifest)?;
    let r = analyse(&m, &tree(), &root());
    print!("{}", r.table());
    let summary = r.summary();
    if !r.errors.is_empty() {
        for e in &r.errors {
            eprintln!("error: {e}");
        }
        if check || strict {
            bail!("{} manifest error(s); {summary}", r.errors.len());
        }
    }
    if strict && r.exclusions() > 0 {
        bail!("{} reviewed exclusion(s) left (--strict wants 100 %); {summary}", r.exclusions());
    }
    Ok(summary)
}

// ------------------------------------------------------------ the tree

/// What the tool needs of a setting of the tree.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeItem {
    pub id: u16,
    pub kind: ItemKind,
    pub default: i16,
    /// A field packed into another stored word.
    pub packed: bool,
    /// A stored word other settings are packed into.
    pub word: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ItemKind {
    Toggle,
    Choice(usize),
    Range { min: i16, max: i16 },
    Language,
    HomeApps,
}

impl ItemKind {
    fn name(&self) -> &'static str {
        match self {
            ItemKind::Toggle => "toggle",
            ItemKind::Choice(_) => "choice",
            ItemKind::Range { .. } => "range",
            ItemKind::Language => "language",
            ItemKind::HomeApps => "homeapps",
        }
    }
}

/// Every setting with a value (actions and information rows have none),
/// from the code.
pub fn tree() -> Vec<TreeItem> {
    use orpheus_apps::settings::{Kind, all};
    let words: BTreeSet<u16> = all().filter_map(|s| s.pack.map(|p| p.0.0)).collect();
    all()
        .filter_map(|s| {
            let kind = match s.kind {
                Kind::Toggle => ItemKind::Toggle,
                Kind::Choice(c) => ItemKind::Choice(c.len()),
                Kind::Range { min, max, .. } => ItemKind::Range { min, max },
                Kind::Language => ItemKind::Language,
                Kind::HomeApps => ItemKind::HomeApps,
                Kind::Action(_) | Kind::Info(_) => return None,
            };
            Some(TreeItem {
                id: s.id.0,
                kind,
                default: s.default,
                packed: s.pack.is_some(),
                word: words.contains(&s.id.0),
            })
        })
        .collect()
}

// ------------------------------------------------------------ manifest

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(default)]
    pub contexts: BTreeMap<String, String>,
    #[serde(default)]
    pub groups: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub kinds: BTreeMap<String, KindTests>,
    #[serde(default)]
    pub setting: Vec<Entry>,
}

/// Persistence, defaults, clamping and migration of one kind of setting.
#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct KindTests {
    #[serde(default)]
    pub persist: Vec<String>,
    #[serde(default)]
    pub default: Vec<String>,
    #[serde(default)]
    pub clamp: Vec<String>,
    #[serde(default)]
    pub migrate: Vec<String>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub values: Vec<toml::Value>,
    pub default: Option<toml::Value>,
    #[serde(default)]
    pub contexts: Vec<String>,
    /// Contexts the setting must not change (groups minus `contexts`).
    #[serde(default)]
    pub unaffected: Vec<String>,
    /// Contexts a global setting cannot show in, with the reason.
    #[serde(default)]
    pub na: BTreeMap<String, String>,
    #[serde(default)]
    pub oracle: String,
    #[serde(default)]
    pub tests: Vec<String>,
    /// Context (or group) → the tests and checks that show it.
    #[serde(default)]
    pub cover: BTreeMap<String, Vec<String>>,
    /// Effects only the player shows.
    #[serde(default)]
    pub device: bool,
    /// On-device checks: `suite:<journey>`.
    #[serde(default)]
    pub checks: Vec<String>,
    /// Declared contexts without a test yet, with the reason (reviewed
    /// exclusions).
    #[serde(default)]
    pub uncovered: BTreeMap<String, String>,
    /// The setting has no effect to show yet (reviewed exclusion).
    pub gap: Option<String>,
}

pub fn load(path: &Path) -> Result<Manifest> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    parse(&text).with_context(|| format!("parse {}", path.display()))
}

pub fn parse(text: &str) -> Result<Manifest> {
    Ok(toml::from_str(text)?)
}

fn parse_id(s: &str) -> Option<u16> {
    let h = s.strip_prefix("0x")?;
    (h.len() == 4 && h.chars().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()))
        .then(|| u16::from_str_radix(h, 16).ok())
        .flatten()
}

// ------------------------------------------------------------ analysis

/// One setting's line of the table.
#[derive(Debug, Default)]
pub struct Row {
    pub id: String,
    pub name: String,
    pub scope: String,
    pub device: bool,
    pub gap: bool,
    /// Matrix cells (contexts, affected and unaffected) and those covered.
    pub cells: usize,
    pub covered: usize,
}

#[derive(Debug, Default)]
pub struct Report {
    pub rows: Vec<Row>,
    pub errors: Vec<String>,
    /// (setting, context or "*", reason).
    pub excluded: Vec<(String, String, String)>,
}

impl Report {
    #[must_use]
    pub fn exclusions(&self) -> usize {
        self.excluded.len()
    }

    #[must_use]
    pub fn table(&self) -> String {
        let mut t = String::new();
        let _ = writeln!(t, "{:<7} {:<6} {:<4} {:>9}  name", "id", "scope", "dev", "cells");
        for r in &self.rows {
            let cells = if r.gap { "gap".to_string() } else { format!("{}/{}", r.covered, r.cells) };
            let _ = writeln!(
                t,
                "{:<7} {:<6} {:<4} {:>9}  {}",
                r.id,
                r.scope,
                if r.device { "yes" } else { "" },
                cells,
                r.name
            );
        }
        let _ =
            writeln!(t, "\nby scope (settings fully covered / with exclusions / gap; matrix cells covered):");
        for scope in ["global", "local", "storage"] {
            let rs: Vec<&Row> = self.rows.iter().filter(|r| r.scope == scope).collect();
            if rs.is_empty() {
                continue;
            }
            let full = rs.iter().filter(|r| !r.gap && r.covered == r.cells).count();
            let gap = rs.iter().filter(|r| r.gap).count();
            let (c, n) = rs.iter().fold((0, 0), |(c, n), r| (c + r.covered, n + r.cells));
            let _ = writeln!(
                t,
                "  {scope:<8} {} settings: {full} full, {} partial, {gap} gap; cells {c}/{n}",
                rs.len(),
                rs.len() - full - gap
            );
        }
        let dev: Vec<&Row> = self.rows.iter().filter(|r| r.device).collect();
        let _ = writeln!(
            t,
            "  device   {} settings with effects only the player shows ({} fully covered)",
            dev.len(),
            dev.iter().filter(|r| !r.gap && r.covered == r.cells).count()
        );
        if !self.excluded.is_empty() {
            let _ = writeln!(t, "\nreviewed exclusions ({}):", self.excluded.len());
            for (id, ctx, why) in &self.excluded {
                let _ = writeln!(t, "  {id} {ctx}: {why}");
            }
        }
        t
    }

    #[must_use]
    pub fn summary(&self) -> String {
        let (c, n) = self.rows.iter().fold((0, 0), |(c, n), r| (c + r.covered, n + r.cells));
        let gaps = self.rows.iter().filter(|r| r.gap).count();
        let pct = if n == 0 { 100.0 } else { 100.0 * c as f64 / n as f64 };
        format!(
            "{} settings, matrix {c}/{n} cells covered ({pct:.1} %), {gaps} setting(s) without an effect to show, {} exclusion(s), {} error(s)",
            self.rows.len(),
            self.exclusions(),
            self.errors.len()
        )
    }
}

/// Check the manifest against the tree and the sources under `root`.
#[must_use]
pub fn analyse(m: &Manifest, tree: &[TreeItem], root: &Path) -> Report {
    let mut r = Report::default();
    let mut err = |e: String| r.errors.push(e);
    let mut files = Sources::new(root);
    let journeys = journeys(root);

    // Contexts and groups.
    for (g, members) in &m.groups {
        if m.contexts.contains_key(g) {
            err(format!("group `{g}` is also a context"));
        }
        for c in members {
            if !m.contexts.contains_key(c) {
                err(format!("group `{g}`: `{c}` is not a declared context"));
            }
        }
    }
    let expand = |names: &[String], errs: &mut Vec<String>, what: &str| -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for n in names {
            if let Some(g) = n.strip_prefix('@') {
                match m.groups.get(g) {
                    Some(members) => out.extend(members.iter().cloned()),
                    None => errs.push(format!("{what}: unknown group `@{g}`")),
                }
            } else if m.contexts.contains_key(n) {
                out.insert(n.clone());
            } else {
                errs.push(format!("{what}: unknown context `{n}` (declare it in [contexts])"));
            }
        }
        out
    };

    // Kinds: persistence, defaults, clamping, migration.
    let kinds: BTreeSet<&'static str> = tree
        .iter()
        .map(|t| if t.word { "packed" } else { t.kind.name() })
        .chain(tree.iter().any(|t| t.packed).then_some("packed"))
        .collect();
    for k in &kinds {
        match m.kinds.get(*k) {
            None => r
                .errors
                .push(format!("[kinds.{k}] missing: persistence, defaults, clamping and migration tests")),
            Some(kt) => {
                for (what, tests) in [
                    ("persist", &kt.persist),
                    ("default", &kt.default),
                    ("clamp", &kt.clamp),
                    ("migrate", &kt.migrate),
                ] {
                    if tests.is_empty() {
                        r.errors.push(format!("[kinds.{k}] has no `{what}` test"));
                    }
                    for t in tests {
                        if let Err(e) = files.resolve(t) {
                            r.errors.push(format!("[kinds.{k}] {what}: {e}"));
                        }
                    }
                }
            }
        }
    }
    for k in m.kinds.keys() {
        if !kinds.contains(k.as_str()) {
            r.errors.push(format!("[kinds.{k}]: the tree has no setting of this kind"));
        }
    }

    // Entries.
    let by_id: BTreeMap<u16, &TreeItem> = tree.iter().map(|t| (t.id, t)).collect();
    let mut seen = BTreeSet::new();
    for e in &m.setting {
        let what = format!("{} ({})", e.id, e.name);
        let Some(id) = parse_id(&e.id) else {
            r.errors.push(format!("{what}: id must be \"0x\" and four upper-case hex digits"));
            continue;
        };
        if !seen.insert(id) {
            r.errors.push(format!("{what}: two entries"));
            continue;
        }
        let Some(item) = by_id.get(&id) else {
            r.errors.push(format!("{what}: no such setting in the tree (removed? renumbered?)"));
            continue;
        };
        let mut errs = Vec::new();
        let mut row = Row {
            id: e.id.clone(),
            name: e.name.clone(),
            scope: if item.word { "storage".into() } else { e.scope.clone() },
            device: e.device,
            gap: e.gap.is_some(),
            ..Row::default()
        };
        if e.name.is_empty() {
            errs.push("no name".into());
        }
        if e.oracle.trim().is_empty() {
            errs.push("no oracle".into());
        }
        match (item.word, e.scope.as_str()) {
            (_, "global" | "local") => {}
            _ => errs.push(format!("scope `{}` (global or local)", e.scope)),
        }
        check_values(item, e, &mut errs);
        // Tests and checks.
        if e.tests.is_empty() && e.checks.is_empty() {
            errs.push("no tests".into());
        }
        for t in &e.tests {
            if let Err(x) = files.resolve(t) {
                errs.push(x.to_string());
            }
        }
        if e.device && e.checks.is_empty() && e.gap.is_none() && e.uncovered.is_empty() {
            errs.push("device = true needs `checks` (suite:<journey>) or the `uncovered` reason".into());
        }
        if !e.device && !e.checks.is_empty() {
            errs.push("`checks` without device = true".into());
        }
        for c in &e.checks {
            match c.strip_prefix("suite:") {
                Some(j) if journeys.contains(j) => {}
                Some(j) => errs.push(format!("check `{c}`: no journey `{j}` in tests/suite/journeys.toml")),
                None => errs.push(format!("check `{c}`: expected suite:<journey>")),
            }
        }
        if let Some(why) = &e.gap {
            if why.trim().is_empty() {
                errs.push("gap without a reason".into());
            }
            r.excluded.push((e.id.clone(), "*".into(), why.clone()));
            for x in errs {
                r.errors.push(format!("{what}: {x}"));
            }
            r.rows.push(row);
            continue;
        }
        // Contexts.
        let own = expand(&e.contexts, &mut errs, "contexts");
        if own.is_empty() {
            errs.push("no contexts".into());
        }
        let not: BTreeSet<String> =
            expand(&e.unaffected, &mut errs, "unaffected").difference(&own).cloned().collect();
        let na = expand(&e.na.keys().cloned().collect::<Vec<_>>(), &mut errs, "na");
        for c in na.intersection(&own) {
            errs.push(format!("`{c}` both affected and not applicable"));
        }
        if e.scope == "global" {
            let missing: Vec<&String> = m
                .contexts
                .keys()
                .filter(|c| !own.contains(*c) && !not.contains(*c) && !na.contains(*c))
                .collect();
            if !missing.is_empty() {
                errs.push(format!(
                    "global: contexts neither affected, unaffected nor na: {}",
                    missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                ));
            }
        }
        if not.is_empty() && !item.word {
            errs.push("no `unaffected` context (where the effect must be absent)".into());
        }
        // Cover.
        let listed: BTreeSet<&String> = e.tests.iter().chain(&e.checks).collect();
        let mut covered: BTreeSet<String> = BTreeSet::new();
        // Covered by name (a group's cover — the matrix over @screens —
        // gives way to an `uncovered` reason: it cannot show everything).
        let mut by_name: BTreeSet<String> = BTreeSet::new();
        for (ctx, by) in &e.cover {
            if !ctx.starts_with('@') {
                by_name.insert(ctx.clone());
            }
            let cs = expand(std::slice::from_ref(ctx), &mut errs, "cover");
            if by.is_empty() {
                errs.push(format!("cover `{ctx}`: no test"));
                continue;
            }
            for t in by {
                if !listed.contains(t) {
                    errs.push(format!("cover `{ctx}`: `{t}` is not in tests or checks"));
                }
            }
            for c in cs {
                if !own.contains(&c) && !not.contains(&c) {
                    errs.push(format!("cover `{c}`: neither a context nor unaffected"));
                }
                covered.insert(c);
            }
        }
        let unc = expand(&e.uncovered.keys().cloned().collect::<Vec<_>>(), &mut errs, "uncovered");
        for (c, why) in &e.uncovered {
            if why.trim().is_empty() {
                errs.push(format!("uncovered `{c}` without a reason"));
            }
        }
        for c in &unc {
            if !by_name.contains(c) {
                covered.remove(c);
            }
        }
        let cells: BTreeSet<&String> = own.iter().chain(&not).collect();
        for c in &cells {
            match (covered.contains(*c), unc.contains(*c)) {
                (true, true) => errs.push(format!("`{c}` is covered by name and listed as uncovered")),
                (false, false) => {
                    errs.push(format!("context `{c}` has no test (cover) and no `uncovered` reason"));
                }
                (false, true) => {
                    let why = e.uncovered.get(*c).cloned().unwrap_or_else(|| "(group)".into());
                    r.excluded.push((e.id.clone(), (*c).clone(), why));
                }
                (true, false) => {}
            }
        }
        if !item.word && !not.is_empty() && !not.iter().any(|c| covered.contains(c)) {
            errs.push("no covered `unaffected` context: the effect is never shown absent".into());
        }
        row.cells = cells.len();
        row.covered = cells.iter().filter(|c| covered.contains(**c)).count();
        for x in errs {
            r.errors.push(format!("{what}: {x}"));
        }
        r.rows.push(row);
    }
    for t in tree {
        if !seen.contains(&t.id) {
            r.errors.push(
                format!("{:#06X}: setting of the tree without a manifest entry", t.id).replace("0X", "0x"),
            );
        }
    }
    r
}

fn check_values(item: &TreeItem, e: &Entry, errs: &mut Vec<String>) {
    if e.values.is_empty() {
        errs.push("no values".into());
        return;
    }
    let Some(default) = &e.default else {
        errs.push("no default".into());
        return;
    };
    if item.word {
        if default.as_integer() != Some(i64::from(item.default)) {
            errs.push(format!("default {default} (the code's is {})", item.default));
        }
        return;
    }
    let ints: Vec<i64> = e.values.iter().filter_map(toml::Value::as_integer).collect();
    match item.kind {
        ItemKind::Toggle => {
            if e.values.len() != 2 {
                errs.push("a toggle has two values".into());
            }
        }
        ItemKind::Choice(n) => {
            if e.values.len() != n {
                errs.push(format!("{} values for {n} choices", e.values.len()));
            }
        }
        ItemKind::Range { min, max } => {
            if ints.len() == e.values.len() {
                if !ints.contains(&i64::from(min)) || !ints.contains(&i64::from(max)) {
                    errs.push(format!("range values must include both ends ({min}, {max})"));
                }
                if ints.iter().any(|v| *v < i64::from(min) || *v > i64::from(max)) {
                    errs.push(format!("a value outside {min}..={max}"));
                }
            } else if e.values.len() < 2 {
                errs.push("a range needs its two ends".into());
            }
        }
        ItemKind::Language | ItemKind::HomeApps => {}
    }
    // Choice and toggle defaults are indices; range defaults are values.
    if default.as_integer() != Some(i64::from(item.default)) {
        errs.push(format!("default {default} (the code's is {})", item.default));
    }
}

fn journeys(root: &Path) -> BTreeSet<String> {
    #[derive(Deserialize)]
    struct F {
        #[serde(default)]
        journey: Vec<J>,
    }
    #[derive(Deserialize)]
    struct J {
        name: String,
    }
    std::fs::read_to_string(root.join("tests/suite/journeys.toml"))
        .ok()
        .and_then(|t| toml::from_str::<F>(&t).ok())
        .map(|f| f.journey.into_iter().map(|j| j.name).collect())
        .unwrap_or_default()
}

// ------------------------------------------------------------ tests named

/// Test functions found in source files (cached per file).
pub struct Sources {
    root: PathBuf,
    files: BTreeMap<String, Result<Vec<TestFn>, String>>,
}

/// A function: its module path within the file, its name, whether it is a
/// test and whether it is ignored.
#[derive(Clone, Debug, PartialEq)]
pub struct TestFn {
    pub path: Vec<String>,
    pub name: String,
    pub test: bool,
    pub ignored: bool,
}

impl Sources {
    #[must_use]
    pub fn new(root: &Path) -> Sources {
        Sources { root: root.to_path_buf(), files: BTreeMap::new() }
    }

    /// `file.rs::mod::name` → ok if a non-ignored `#[test] fn name` is in
    /// module `mod` of `file.rs`.
    pub fn resolve(&mut self, spec: &str) -> Result<()> {
        let Some(at) = spec.find(".rs::") else { bail!("test `{spec}`: expected path/file.rs::…::name") };
        let (file, rest) = (&spec[..at + 3], &spec[at + 5..]);
        let mut parts: Vec<String> = rest.split("::").map(str::to_string).collect();
        let name = parts.pop().unwrap_or_default();
        let root = self.root.clone();
        let fns = self.files.entry(file.to_string()).or_insert_with(|| {
            std::fs::read_to_string(root.join(file)).map(|t| functions(&t)).map_err(|e| format!("{e}"))
        });
        let fns = match fns {
            Ok(f) => f,
            Err(e) => bail!("test `{spec}`: cannot read {file}: {e}"),
        };
        match fns.iter().find(|f| f.path == parts && f.name == name) {
            None => bail!("test `{spec}`: no fn `{name}` in module `{}` of {file}", parts.join("::")),
            Some(f) if !f.test => bail!("test `{spec}`: `{name}` is not a #[test]"),
            Some(f) if f.ignored => bail!("test `{spec}`: `{name}` is #[ignore]d"),
            Some(_) => Ok(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Punct(char),
}

/// Rust source to identifiers and punctuation, without comments, strings
/// and character literals.
fn tokens(src: &str) -> Vec<Tok> {
    let c: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let at = |i: usize| c.get(i).copied().unwrap_or('\0');
    while i < c.len() {
        let ch = c[i];
        if ch == '/' && at(i + 1) == '/' {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && at(i + 1) == '*' {
            let mut depth = 0;
            while i < c.len() {
                if c[i] == '/' && at(i + 1) == '*' {
                    depth += 1;
                    i += 2;
                } else if c[i] == '*' && at(i + 1) == '/' {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if ch == '"' {
            i = skip_string(&c, i + 1);
        } else if (ch == 'r' || (ch == 'b' && at(i + 1) == 'r')) && {
            let mut j = i + if ch == 'b' { 2 } else { 1 };
            while at(j) == '#' {
                j += 1;
            }
            at(j) == '"'
        } {
            let mut j = i + if ch == 'b' { 2 } else { 1 };
            let mut hashes = 0;
            while at(j) == '#' {
                hashes += 1;
                j += 1;
            }
            j += 1;
            loop {
                if j >= c.len() {
                    break;
                }
                if c[j] == '"' && (1..=hashes).all(|k| at(j + k) == '#') {
                    j += 1 + hashes;
                    break;
                }
                j += 1;
            }
            i = j;
        } else if ch == 'b' && at(i + 1) == '"' {
            i = skip_string(&c, i + 2);
        } else if ch == 'b' && at(i + 1) == '\'' {
            i = skip_char(&c, i + 1);
        } else if ch == '\'' {
            i = skip_char(&c, i);
        } else if ch.is_alphabetic() || ch == '_' {
            let s = i;
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
            out.push(Tok::Ident(c[s..i].iter().collect()));
        } else if ch.is_ascii_digit() {
            while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_') {
                i += 1;
            }
        } else {
            if !ch.is_whitespace() {
                out.push(Tok::Punct(ch));
            }
            i += 1;
        }
    }
    out
}

/// Past the closing quote of a string whose body starts at `i`.
fn skip_string(c: &[char], mut i: usize) -> usize {
    while i < c.len() {
        match c[i] {
            '\\' => i += 2,
            '"' => return i + 1,
            _ => i += 1,
        }
    }
    i
}

/// At a `'`: past a character literal, or past the quote of a lifetime.
fn skip_char(c: &[char], i: usize) -> usize {
    let at = |k: usize| c.get(k).copied().unwrap_or('\0');
    if at(i + 1) == '\\' {
        let mut j = i + 2;
        while j < c.len() && c[j] != '\'' {
            j += 1;
        }
        j + 1
    } else if at(i + 2) == '\'' {
        i + 3
    } else {
        i + 1
    }
}

/// Every `fn` of a source file with its module path and attributes.
#[must_use]
pub fn functions(src: &str) -> Vec<TestFn> {
    let t = tokens(src);
    let mut out = Vec::new();
    // (module name, brace depth inside it)
    let mut mods: Vec<(String, usize)> = Vec::new();
    let mut depth = 0usize;
    let mut attrs: Vec<String> = Vec::new();
    let mut i = 0;
    let ident = |k: usize| match t.get(k) {
        Some(Tok::Ident(s)) => Some(s.as_str()),
        _ => None,
    };
    let punct = |k: usize, p: char| t.get(k) == Some(&Tok::Punct(p));
    while i < t.len() {
        if punct(i, '#') && (punct(i + 1, '[') || (punct(i + 1, '!') && punct(i + 2, '['))) {
            let mut j = i + if punct(i + 1, '!') { 3 } else { 2 };
            let mut d = 1;
            let mut a = String::new();
            while j < t.len() && d > 0 {
                match &t[j] {
                    Tok::Punct('[') => d += 1,
                    Tok::Punct(']') => d -= 1,
                    _ => {}
                }
                if d > 0 {
                    match &t[j] {
                        Tok::Ident(s) => {
                            a.push_str(s);
                            a.push(' ');
                        }
                        Tok::Punct(p) => a.push(*p),
                    }
                }
                j += 1;
            }
            attrs.push(a);
            i = j;
            continue;
        }
        match (&t[i], ident(i + 1)) {
            (Tok::Ident(k), Some(name)) if k == "mod" => {
                if punct(i + 2, '{') {
                    depth += 1;
                    mods.push((name.to_string(), depth));
                    attrs.clear();
                    i += 3;
                    continue;
                }
            }
            (Tok::Ident(k), Some(name)) if k == "fn" => {
                let is_test = attrs.iter().any(|a| {
                    let w: Vec<&str> =
                        a.split([' ', ':', '(', ')', ',', '=']).filter(|s| !s.is_empty()).collect();
                    w.last() == Some(&"test") || w.first() == Some(&"test")
                });
                let ignored = attrs.iter().any(|a| a.split([' ', '(', ')', ',', '=']).any(|w| w == "ignore"));
                out.push(TestFn {
                    path: mods.iter().map(|m| m.0.clone()).collect(),
                    name: name.to_string(),
                    test: is_test,
                    ignored,
                });
                attrs.clear();
                i += 2;
                continue;
            }
            _ => {}
        }
        match t[i] {
            Tok::Punct('{') => {
                depth += 1;
                attrs.clear();
            }
            Tok::Punct('}') => {
                if mods.last().is_some_and(|m| m.1 == depth) {
                    mods.pop();
                }
                depth = depth.saturating_sub(1);
                attrs.clear();
            }
            Tok::Punct(';') => attrs.clear(),
            _ => {}
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r##"
//! fn not_this() {}
use x;
#[test]
fn top() { let s = "fn fake() { }"; let c = '{'; let r = r#"}"#; }
fn helper<'a>(x: &'a str) -> &'a str { x }
mod inner {
    /// doc: fn nope()
    #[test]
    #[ignore = "slow"]
    fn slow() {}
    #[cfg(test)]
    mod deeper {
        #[tokio::test]
        async fn deep() { /* } nested /* } */ */ }
        #[test] fn after_deep() {}
    }
    #[test]
    fn after() { let b = b'}'; }
}
#[test]
fn last() {}
"##;

    #[test]
    fn finds_tests_by_module_path() {
        let f = functions(SRC);
        let get = |p: &[&str], n: &str| f.iter().find(|x| x.path == p && x.name == n).cloned();
        assert!(get(&[], "top").is_some_and(|x| x.test && !x.ignored));
        assert!(get(&[], "helper").is_some_and(|x| !x.test));
        assert!(get(&["inner"], "slow").is_some_and(|x| x.test && x.ignored));
        assert!(get(&["inner", "deeper"], "deep").is_some_and(|x| x.test));
        assert!(get(&["inner", "deeper"], "after_deep").is_some_and(|x| x.test));
        assert!(get(&["inner"], "after").is_some_and(|x| x.test));
        assert!(get(&[], "last").is_some_and(|x| x.test));
        assert!(
            get(&[], "fake").is_none() && get(&[], "not_this").is_none() && get(&["inner"], "nope").is_none()
        );
    }

    fn tree1() -> Vec<TreeItem> {
        vec![
            TreeItem { id: 0x0108, kind: ItemKind::Toggle, default: 0, packed: false, word: false },
            TreeItem {
                id: 0x0103,
                kind: ItemKind::Range { min: -6, max: 6 },
                default: 0,
                packed: false,
                word: false,
            },
        ]
    }

    /// A scratch root with a test file and a journeys file.
    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("orpheus-matrix-{name}-{}", std::process::id()));
        std::fs::create_dir_all(d.join("t")).unwrap();
        std::fs::create_dir_all(d.join("tests/suite")).unwrap();
        std::fs::write(
            d.join("t/a.rs"),
            "mod m {\n#[test]\nfn click() {}\n#[test]\nfn bass() {}\n#[test]\n#[ignore]\nfn off() {}\n}\n",
        )
        .unwrap();
        std::fs::write(d.join("tests/suite/journeys.toml"), "[[journey]]\nname = \"snd\"\n").unwrap();
        d
    }

    const HEAD: &str = r#"
[contexts]
Home = "home"
Settings = "settings"
audio = "sound"
[groups]
screens = ["Home", "Settings"]
[kinds.toggle]
persist = ["t/a.rs::m::click"]
default = ["t/a.rs::m::click"]
clamp = ["t/a.rs::m::click"]
migrate = ["t/a.rs::m::click"]
[kinds.range]
persist = ["t/a.rs::m::bass"]
default = ["t/a.rs::m::bass"]
clamp = ["t/a.rs::m::bass"]
migrate = ["t/a.rs::m::bass"]
"#;

    const CLICK: &str = r#"
[[setting]]
id = "0x0108"
name = "Key click"
scope = "global"
values = ["off", "on"]
default = 0
contexts = ["audio"]
unaffected = ["@screens"]
oracle = "a click"
tests = ["t/a.rs::m::click"]
cover = { audio = ["t/a.rs::m::click"], "@screens" = ["t/a.rs::m::click"] }
"#;

    const BASS: &str = r#"
[[setting]]
id = "0x0103"
name = "Bass"
scope = "local"
values = [-6, 0, 6]
default = 0
contexts = ["audio"]
unaffected = ["Home"]
oracle = "low shelf"
tests = ["t/a.rs::m::bass"]
device = true
checks = ["suite:snd"]
cover = { audio = ["suite:snd"], Home = ["t/a.rs::m::bass"] }
"#;

    #[test]
    fn a_complete_manifest_passes() {
        let root = scratch("ok");
        let m = parse(&format!("{HEAD}{CLICK}{BASS}")).unwrap();
        let r = analyse(&m, &tree1(), &root);
        assert!(r.errors.is_empty(), "{:#?}", r.errors);
        assert_eq!(r.exclusions(), 0);
        let s = r.summary();
        assert!(s.contains("matrix 5/5 cells"), "{s}");
        assert!(r.table().contains("device   1 settings"));
    }

    /// 2026-11-25 (VER-13): the gate must fail on a setting of the tree
    /// without an entry and on a test name that does not resolve (renamed,
    /// moved or ignored tests kept the manifest "green" before, when it
    /// was only searched as text).
    #[test]
    fn regress_settings_matrix_missing_entry_and_dangling_test() {
        let root = scratch("bad");
        let dangling = CLICK.replace("m::click\"]\ncover", "m::clack\"]\ncover").replace(
            "cover = { audio = [\"t/a.rs::m::click\"], \"@screens\" = [\"t/a.rs::m::click\"] }",
            "cover = { audio = [\"t/a.rs::m::clack\"], \"@screens\" = [\"t/a.rs::m::clack\"] }",
        );
        let m = parse(&format!("{HEAD}{dangling}")).unwrap();
        let r = analyse(&m, &tree1(), &root);
        let all = r.errors.join("\n");
        assert!(all.contains("0x0103: setting of the tree without a manifest entry"), "{all}");
        assert!(all.contains("no fn `clack` in module `m`"), "{all}");
        // An ignored test does not count either.
        let ignored = CLICK.replace("m::click", "m::off");
        let r = analyse(&parse(&format!("{HEAD}{ignored}{BASS}")).unwrap(), &tree1(), &root);
        assert!(r.errors.join("\n").contains("`off` is #[ignore]d"), "{:#?}", r.errors);
    }

    #[test]
    fn uncovered_contexts_and_missing_absence_fail() {
        let root = scratch("cover");
        // Audio without a test, and no unaffected context covered.
        let c = CLICK.replace(
            "cover = { audio = [\"t/a.rs::m::click\"], \"@screens\" = [\"t/a.rs::m::click\"] }",
            "cover = {}",
        );
        let r = analyse(&parse(&format!("{HEAD}{c}{BASS}")).unwrap(), &tree1(), &root);
        let all = r.errors.join("\n");
        assert!(all.contains("context `audio` has no test"), "{all}");
        assert!(all.contains("never shown absent"), "{all}");
        // A reviewed exclusion is accepted and counted.
        let c = CLICK.replace(
            "cover = { audio = [\"t/a.rs::m::click\"], \"@screens\" = [\"t/a.rs::m::click\"] }",
            "cover = { \"@screens\" = [\"t/a.rs::m::click\"] }\nuncovered = { audio = \"no click sound yet\" }",
        );
        let r = analyse(&parse(&format!("{HEAD}{c}{BASS}")).unwrap(), &tree1(), &root);
        assert!(r.errors.is_empty(), "{:#?}", r.errors);
        assert_eq!(r.exclusions(), 1);
        assert!(r.summary().contains("4/5 cells"), "{}", r.summary());
    }

    #[test]
    fn global_settings_account_for_every_context() {
        let root = scratch("global");
        let c = CLICK.replace("unaffected = [\"@screens\"]", "unaffected = [\"Home\"]").replace(
            "cover = { audio = [\"t/a.rs::m::click\"], \"@screens\" = [\"t/a.rs::m::click\"] }",
            "cover = { audio = [\"t/a.rs::m::click\"], Home = [\"t/a.rs::m::click\"] }",
        );
        let r = analyse(&parse(&format!("{HEAD}{c}{BASS}")).unwrap(), &tree1(), &root);
        assert!(
            r.errors.join("\n").contains("neither affected, unaffected nor na: Settings"),
            "{:#?}",
            r.errors
        );
    }

    #[test]
    fn values_defaults_kinds_and_checks_are_checked() {
        let root = scratch("values");
        let b = BASS
            .replace("values = [-6, 0, 6]", "values = [-5, 0, 6]")
            .replace("default = 0", "default = 1")
            .replace("suite:snd", "suite:nope");
        let m = parse(&format!("{HEAD}{CLICK}{b}")).unwrap();
        let r = analyse(&m, &tree1(), &root);
        let all = r.errors.join("\n");
        assert!(all.contains("both ends"), "{all}");
        assert!(all.contains("default 1 (the code's is 0)"), "{all}");
        assert!(all.contains("no journey `nope`"), "{all}");
        // A kind without its persistence tests.
        let head = HEAD.replace("[kinds.range]", "[kinds.unused]");
        let r = analyse(&parse(&format!("{head}{CLICK}{BASS}")).unwrap(), &tree1(), &root);
        let all = r.errors.join("\n");
        assert!(
            all.contains("[kinds.range] missing") && all.contains("[kinds.unused]: the tree has no"),
            "{all}"
        );
        // Unknown fields are refused (a typo must not hide a requirement).
        assert!(parse(&format!("{HEAD}{CLICK}covr = 1\n")).is_err());
    }

    #[test]
    fn a_gap_is_an_exclusion_and_strict_refuses_it() {
        let root = scratch("gap");
        let c = "\n[[setting]]\nid = \"0x0108\"\nname = \"Key click\"\nscope = \"global\"\nvalues = [\"off\", \"on\"]\ndefault = 0\noracle = \"none yet\"\ntests = [\"t/a.rs::m::click\"]\ngap = \"not wired\"\n";
        let r = analyse(&parse(&format!("{HEAD}{c}{BASS}")).unwrap(), &tree1(), &root);
        assert!(r.errors.is_empty(), "{:#?}", r.errors);
        assert_eq!(r.exclusions(), 1);
        assert!(r.table().contains("gap"));
    }

    /// The real manifest against the real tree: no errors (what
    /// `xtask settings-matrix --check` enforces).
    #[test]
    fn the_manifest_covers_the_tree() {
        let m = load(&root().join("tests/settings/effects.toml")).unwrap();
        let r = analyse(&m, &tree(), &root());
        assert!(r.errors.is_empty(), "{}", r.errors.join("\n"));
    }
}
