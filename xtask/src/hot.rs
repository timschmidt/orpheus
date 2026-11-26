//! `xtask hot` (T-8, 07 §5.6): a `hot.toml` cluster from the static call
//! graph, and the line budget check of such clusters.
//!
//! A fault capture shows what ran; a chore that runs while music plays is
//! drowned in the playback's and the UI's faults (a game switch: the games
//! card pieces, 156–223 line loads each, were a small share of the
//! capture), so `dev prof --update-hot` would pick the wrong sections.
//! The working set of such a chore is known statically instead: the paged
//! code reachable from its entry points.
//!
//! * **Reach**: the call graph of `xtask stack` (direct branches between
//!   code sections, and the indirect targets `stack.toml` names globally),
//!   breadth-first from the roots (`--from`, symbol globs), not entering
//!   `--stop` functions (what the chore never runs: a cold branch kept out
//!   of line, a fallback). The report: functions, paged and resident bytes,
//!   by crate and by owning cluster, the largest paged functions.
//! * **Selection** ([`select`]): the reachable paged functions closest to
//!   the roots first (then those called from more reached functions), as
//!   literal patterns of their names, greedily within `--lines` code lines.
//!   Functions another cluster owns are left to it (pagelink gives a
//!   section to the first matching cluster), as are names that also select
//!   another cluster's section.
//! * **Check** ([`working_set`]): a cluster with `from` in `hot.toml` has a
//!   `budget_lines`; the reachable paged code that is neither in the
//!   cluster nor in a cluster of its `fits_with` (in the cache with it by
//!   design) costs at least a line per function. The cluster's lines plus
//!   those must stay within the budget (`xtask stack --check`).

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;

use anyhow::{Context, Result, bail};
use orpheus_pager::LINE_SIZE;

use crate::args::Args;
use crate::linkgraph::{Kind, crate_of};
use crate::profile::{literal_pattern, packed, toml_str, update_hot};
use crate::rules::{self, Cluster};
use crate::stack::{self, Analysis, IndirectCfg, Resolve};
use crate::util::{log, root};

// ---------------------------------------------------------------------------
// The selection (host-tested on synthetic graphs).

/// One function of the call graph as the selection sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Func {
    /// The demangled name (what `hot.toml` patterns match).
    pub label: String,
    pub bytes: u32,
    pub paged: bool,
    /// The `hot.toml` cluster that owns it (first match in file order).
    pub owner: Option<String>,
}

/// Breadth-first distances from `roots` over `edges`; `stop` nodes are not
/// entered (a root always is).
#[must_use]
pub fn distances(edges: &[Vec<usize>], roots: &[usize], stop: &[bool]) -> Vec<Option<u32>> {
    let mut d = vec![None; edges.len()];
    let mut q = VecDeque::new();
    for &r in roots {
        if d[r].is_none() {
            d[r] = Some(0);
            q.push_back(r);
        }
    }
    while let Some(n) = q.pop_front() {
        let next = d[n].unwrap_or(0) + 1;
        for &m in &edges[n] {
            if d[m].is_none() && !stop.get(m).copied().unwrap_or(false) {
                d[m] = Some(next);
                q.push_back(m);
            }
        }
    }
    d
}

/// One pattern of the generated cluster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    pub pattern: String,
    /// Paged sections it selects (a name shared by several takes them all).
    pub sections: usize,
    pub bytes: u32,
    /// Calls from the nearest root.
    pub dist: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub picks: Vec<Pick>,
    /// Packed bytes of the picks.
    pub bytes: u32,
    /// Reachable paged functions left out, with the reason.
    pub skipped: Vec<(String, String)>,
}

/// The cluster `name` from the reachable paged functions: closest to the
/// roots first, then those called from more reached functions, then by
/// name; greedily within `lines` code lines.
#[must_use]
pub fn select(
    funcs: &[Func],
    edges: &[Vec<usize>],
    dist: &[Option<u32>],
    name: &str,
    lines: u32,
) -> Selection {
    let mut callers = vec![0u32; funcs.len()];
    for (n, es) in edges.iter().enumerate() {
        if dist[n].is_some() {
            for &m in es {
                callers[m] += 1;
            }
        }
    }
    let mut by_label: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, f) in funcs.iter().enumerate().filter(|(_, f)| f.paged) {
        by_label.entry(f.label.as_str()).or_default().push(i);
    }
    let mut order: Vec<usize> = (0..funcs.len()).filter(|&i| funcs[i].paged && dist[i].is_some()).collect();
    order.sort_by(|&a, &b| {
        dist[a].cmp(&dist[b]).then(callers[b].cmp(&callers[a])).then(funcs[a].label.cmp(&funcs[b].label))
    });
    let cap = lines * LINE_SIZE;
    let mut s = Selection::default();
    let mut seen: Vec<&str> = Vec::new();
    for i in order {
        let f = &funcs[i];
        if seen.contains(&f.label.as_str()) {
            continue;
        }
        seen.push(&f.label);
        if let Some(o) = f.owner.as_deref().filter(|o| *o != name) {
            s.skipped.push((f.label.clone(), format!("in cluster {o}")));
            continue;
        }
        let same = &by_label[f.label.as_str()];
        if let Some(o) = same.iter().find_map(|&j| funcs[j].owner.as_deref().filter(|o| *o != name)) {
            s.skipped.push((f.label.clone(), format!("its name also selects a section of cluster {o}")));
            continue;
        }
        let bytes: u32 = same.iter().map(|&j| packed(funcs[j].bytes)).sum();
        if s.bytes + bytes > cap {
            s.skipped.push((f.label.clone(), format!("{bytes} B do not fit ({} of {cap} B used)", s.bytes)));
            continue;
        }
        s.bytes += bytes;
        s.picks.push(Pick {
            pattern: literal_pattern(&f.label),
            sections: same.len(),
            bytes,
            dist: dist[i].unwrap_or(0),
        });
    }
    s
}

/// Keep the order of the cluster as it stands in `hot.toml` (`old`, its
/// patterns): picks it has keep their places, new ones follow in the
/// selection's order. An order tuned by hand (which code shares a
/// physical line with what, AU-PERF games pieces) survives `--write`.
pub fn keep_order(sel: &mut Selection, old: &[String]) {
    let at = |p: &Pick| old.iter().position(|o| *o == p.pattern).unwrap_or(usize::MAX);
    sel.picks.sort_by_key(at);
}

/// What a cluster's reachable paged code costs in code lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkingSet {
    /// Packed bytes of every paged function the cluster owns (it is placed
    /// as one block).
    pub own: u32,
    /// Reachable paged bytes in the clusters of `fits_with`.
    pub with: u32,
    /// Reachable paged functions elsewhere: `(label, bytes, owner)`.
    pub outside: Vec<(String, u32, Option<String>)>,
    /// The cluster's lines plus one line per started KiB of each function
    /// outside.
    pub lines: u32,
    /// The lines of the `fits_with` clusters, whole (each is placed as one
    /// block beside this one: in the cache with it by design).
    pub with_lines: u32,
}

/// Physical lines of the code cache (direct-mapped, 32 × 1 KiB, 02 §3.1).
pub const CACHE_LINES: u32 = 32;

/// The working set of cluster `name` (running with `fits_with`) for the
/// reachable functions `dist`.
#[must_use]
pub fn working_set(funcs: &[Func], dist: &[Option<u32>], name: &str, fits_with: &[String]) -> WorkingSet {
    let mut w = WorkingSet::default();
    for (i, f) in funcs.iter().enumerate().filter(|(_, f)| f.paged) {
        let o = f.owner.as_deref();
        if o == Some(name) {
            w.own += packed(f.bytes);
        } else if dist[i].is_some() {
            if o.is_some_and(|o| fits_with.iter().any(|x| x == o)) {
                w.with += packed(f.bytes);
            } else {
                w.outside.push((f.label.clone(), f.bytes, f.owner.clone()));
            }
        }
    }
    w.outside.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    w.lines = w.own.div_ceil(LINE_SIZE) + w.outside.iter().map(|o| o.1.div_ceil(LINE_SIZE)).sum::<u32>();
    w.with_lines = fits_with
        .iter()
        .map(|c| {
            let bytes: u32 = funcs
                .iter()
                .filter(|f| f.paged && f.owner.as_deref() == Some(c.as_str()))
                .map(|f| packed(f.bytes))
                .sum();
            bytes.div_ceil(LINE_SIZE)
        })
        .sum();
    w
}

/// The check of one cluster: `Err` when its working set exceeds `budget`,
/// or when it and its `fits_with` clusters together take more lines than
/// the cache has (SYN-3, 2026-11-26: the synth's render and its per-piece
/// code were 34 lines, and every piece reloaded code).
pub fn check(w: &WorkingSet, name: &str, budget: u32) -> Result<(), String> {
    if w.lines <= budget && w.lines + w.with_lines <= CACHE_LINES {
        return Ok(());
    }
    if w.lines <= budget {
        return Err(format!(
            "cluster {name}: its reachable paged code ({} lines) and its fits_with clusters ({} lines) take {} lines, more than the {CACHE_LINES} of the code cache: every pass reloads code; shrink the path or keep code out of it",
            w.lines,
            w.with_lines,
            w.lines + w.with_lines
        ));
    }
    let first: Vec<String> = w.outside.iter().take(4).map(|o| format!("{} ({} B)", o.0, o.1)).collect();
    Err(format!(
        "cluster {name}: its reachable paged code takes {} lines, over its budget_lines {budget} ({} lines in the cluster, {} functions outside it and its fits_with: {}{}); `cargo xtask hot --name {name} --write`, or keep the new code out of the path",
        w.lines,
        w.own.div_ceil(LINE_SIZE),
        w.outside.len(),
        first.join(", "),
        if w.outside.len() > 4 { ", …" } else { "" }
    ))
}

// ---------------------------------------------------------------------------
// The firmware.

/// The call graph's edges for reachability: direct branches and the
/// indirect targets `stack.toml` names globally.
fn edges(an: &Analysis, res: &BTreeMap<usize, Vec<Resolve>>) -> Vec<Vec<usize>> {
    let mut e: Vec<Vec<usize>> = an.cg.calls.iter().map(|c| c.iter().map(|&(t, _)| t).collect()).collect();
    for (&n, rs) in res {
        for r in rs {
            if let Resolve::Targets(ts) = r {
                e[n].extend(ts.iter().copied());
            }
        }
    }
    for v in &mut e {
        v.sort_unstable();
        v.dedup();
    }
    e
}

fn funcs(an: &Analysis, hot: &[Cluster]) -> Vec<Func> {
    (0..an.secs.len())
        .map(|i| {
            let s = &an.secs[i];
            let code = an.kinds[i] == Kind::Code;
            Func {
                label: an.cg.names[i].clone(),
                bytes: s.size,
                paged: code && matches!(orpheus_pager::Page::of(s.addr), Some(orpheus_pager::Page::Code(_))),
                owner: if code {
                    hot.iter().find(|c| c.rank(s).is_some()).map(|c| c.name.clone())
                } else {
                    None
                },
            }
        })
        .collect()
}

/// Nodes matching any of `pats`; a pattern that matches nothing is an
/// error.
fn nodes(an: &Analysis, pats: &[String], what: &str) -> Result<Vec<usize>> {
    let mut v = Vec::new();
    for p in pats {
        let m = an.find(p);
        if m.is_empty() {
            bail!("{what} `{p}` matches nothing");
        }
        v.extend(m);
    }
    v.sort_unstable();
    v.dedup();
    Ok(v)
}

/// Never entered (besides `stop`): the panic paths, which end in a reset.
pub const COLD: &[&str] = &[
    "core::panicking::*",
    "__rustc::*",
    "core::option::unwrap_failed",
    "core::option::expect_failed",
    "core::result::unwrap_failed",
    "core::slice::index::*_fail",
    "core::str::slice_error_fail*",
    "core::cell::panic_already_*",
];

/// The reach of `from` (not entering `stop` or [`COLD`]) in the firmware.
fn reach(an: &Analysis, e: &[Vec<usize>], from: &[String], stop: &[String]) -> Result<Vec<Option<u32>>> {
    let roots = nodes(an, from, "root")?;
    let mut stopped = vec![false; an.secs.len()];
    for n in nodes(an, stop, "stop")?.into_iter().chain(COLD.iter().flat_map(|p| an.find(p))) {
        stopped[n] = true;
    }
    Ok(distances(e, &roots, &stopped))
}

fn global_resolver(an: &mut Analysis, cfg: &stack::Config) -> BTreeMap<usize, Vec<Resolve>> {
    let mut missing = Vec::new();
    stack::prepare(an, cfg, &mut missing);
    let global: Vec<&IndirectCfg> = cfg.indirect.iter().collect();
    an.resolver(&global, &mut missing).0
}

/// `xtask stack --check`: every cluster with `from` within its
/// `budget_lines`. Returns the report lines and the errors.
pub fn check_all(an: &mut Analysis, cfg: &stack::Config) -> Result<(Vec<String>, Vec<String>)> {
    let hot = rules::load_hot(&root().join("hot.toml"), "firmware")?;
    if !hot.iter().any(|c| !c.from.is_empty()) {
        return Ok((Vec::new(), Vec::new()));
    }
    let res = global_resolver(an, cfg);
    let e = edges(an, &res);
    let fs = funcs(an, &hot);
    let (mut lines, mut errors) = (Vec::new(), Vec::new());
    for c in hot.iter().filter(|c| !c.from.is_empty()) {
        let d = match reach(an, &e, &c.from, &c.stop) {
            Ok(d) => d,
            Err(err) => {
                errors.push(format!("hot.toml cluster {}: {err}", c.name));
                continue;
            }
        };
        let w = working_set(&fs, &d, &c.name, &c.fits_with);
        let budget = c.budget_lines.unwrap_or(0);
        lines.push(format!(
            "cluster {}: reachable paged code {} lines of budget {budget} ({} B in it, {} B in its fits_with, {} functions outside); with its fits_with {} of {CACHE_LINES} cache lines",
            c.name,
            w.lines,
            w.own,
            w.with,
            w.outside.len(),
            w.lines + w.with_lines
        ));
        if let Err(m) = check(&w, &c.name, budget) {
            errors.push(m);
        }
    }
    Ok((lines, errors))
}

/// What a generated `[[cluster]]` block says besides its patterns.
struct Head<'a> {
    name: &'a str,
    from: &'a [String],
    stop: &'a [String],
    fits_with: &'a [String],
    starts_over: Option<&'a str>,
    budget: u32,
    /// The reach, for the comment.
    note: String,
}

/// The `[[cluster]]` block of `h.name`.
fn block(h: &Head<'_>, sel: &Selection) -> String {
    let (name, from, stop, fits_with, budget) = (h.name, h.from, h.stop, h.fits_with, h.budget);
    let mut o = String::from("[[cluster]]\n");
    let _ = writeln!(
        o,
        "# Generated by `cargo xtask hot --name {name}` from the static call graph: {} patterns, {} B ({} lines); {}. Closest to the roots first.",
        sel.picks.len(),
        sel.bytes,
        sel.bytes.div_ceil(LINE_SIZE),
        h.note
    );
    let _ = writeln!(o, "name = {}", toml_str(name));
    o.push_str("bins = [\"firmware\"]\n");
    let list = |o: &mut String, key: &str, v: &[String]| {
        let items: Vec<String> = v.iter().map(|s| toml_str(s)).collect();
        let _ = writeln!(o, "{key} = [{}]", items.join(", "));
    };
    if !fits_with.is_empty() {
        list(&mut o, "fits_with", fits_with);
    }
    if let Some(over) = h.starts_over {
        let _ = writeln!(o, "starts_over = {}", toml_str(over));
    }
    o.push_str("from = [\n");
    for f in from {
        let _ = writeln!(o, "    {},", toml_str(f));
    }
    o.push_str("]\n");
    if !stop.is_empty() {
        o.push_str("stop = [\n");
        for s in stop {
            let _ = writeln!(o, "    {},", toml_str(s));
        }
        o.push_str("]\n");
    }
    let _ = writeln!(o, "budget_lines = {budget}");
    o.push_str("symbols = [\n");
    for p in &sel.picks {
        let _ = writeln!(o, "    {}, # {} B, {} calls from a root", toml_str(&p.pattern), p.bytes, p.dist);
    }
    o.push_str("]\n");
    o
}

fn kib(b: u32) -> String {
    format!("{b} B ({} lines)", b.div_ceil(LINE_SIZE))
}

/// `cargo xtask hot [--name NAME] [--from PAT]… [--stop PAT]… [--lines N]
/// [--fits-with A,B] [--top N] [--write]`.
pub fn cmd(args: &mut Args) -> Result<String> {
    let name = args.value("name").unwrap_or_else(|| "reach".into());
    let mut from = Vec::new();
    while let Some(f) = args.value("from") {
        from.push(f);
    }
    let mut stop = Vec::new();
    while let Some(s) = args.value("stop") {
        stop.push(s);
    }
    let lines: Option<u32> = args.value("lines").map(|v| v.parse()).transpose().context("--lines N")?;
    let fits: Option<Vec<String>> =
        args.value("fits-with").map(|v| v.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect());
    let top: usize = args.value("top").map_or(Ok(20), |v| v.parse()).context("--top N")?;
    let write = args.flag("write");
    let (elf, map) = stack::artefact_paths(args)?;

    let hot_path = root().join("hot.toml");
    let hot = rules::load_hot(&hot_path, "firmware")?;
    let stored = hot.iter().find(|c| c.name == name);
    if from.is_empty()
        && let Some(c) = stored
    {
        from.clone_from(&c.from);
        if stop.is_empty() {
            stop.clone_from(&c.stop);
        }
    }
    if from.is_empty() {
        bail!("no roots: --from PATTERN (or a cluster `{name}` with `from` in hot.toml)");
    }
    let lines = lines.or_else(|| stored.and_then(|c| c.budget_lines)).unwrap_or(8);
    let fits_with = fits.unwrap_or_else(|| stored.map(|c| c.fits_with.clone()).unwrap_or_default());

    let cfg = stack::Config::load(&root().join("stack.toml"))?;
    let mut an = stack::load(&elf, &map)?;
    let res = global_resolver(&mut an, &cfg);
    let e = edges(&an, &res);
    let fs = funcs(&an, &hot);
    let d = reach(&an, &e, &from, &stop)?;

    // The reach.
    let reached: Vec<usize> =
        (0..fs.len()).filter(|&i| d[i].is_some() && an.kinds[i] == Kind::Code).collect();
    let paged: Vec<usize> = reached.iter().copied().filter(|&i| fs[i].paged).collect();
    let paged_bytes: u32 = paged.iter().map(|&i| packed(fs[i].bytes)).sum();
    let resident_bytes: u32 = reached.iter().filter(|&&i| !fs[i].paged).map(|&i| fs[i].bytes).sum();
    log(&format!(
        "reach of {name}: {} functions; paged {} in {} functions, resident {resident_bytes} B",
        reached.len(),
        kib(paged_bytes),
        paged.len()
    ));
    let mut by_crate: BTreeMap<String, (usize, u32)> = BTreeMap::new();
    let mut by_owner: BTreeMap<String, (usize, u32)> = BTreeMap::new();
    for &i in &paged {
        let c = crate_of(fs[i].label.trim_start_matches('<')).unwrap_or("(other)").to_string();
        let x = by_crate.entry(c).or_default();
        x.0 += 1;
        x.1 += fs[i].bytes;
        let o = fs[i].owner.clone().unwrap_or_else(|| "(no cluster)".into());
        let x = by_owner.entry(o).or_default();
        x.0 += 1;
        x.1 += fs[i].bytes;
    }
    let mut v: Vec<_> = by_crate.into_iter().collect();
    v.sort_by_key(|a| std::cmp::Reverse(a.1.1));
    log("  paged by crate:");
    for (c, (n, b)) in &v {
        log(&format!("    {b:>7} B {n:>4} functions  {c}"));
    }
    log("  paged by cluster:");
    for (c, (n, b)) in &by_owner {
        log(&format!("    {b:>7} B {n:>4} functions  {c}"));
    }
    let mut big = paged.clone();
    big.sort_by(|&a, &b| fs[b].bytes.cmp(&fs[a].bytes));
    log("  largest paged functions (calls from a root, owner):");
    for &i in big.iter().take(top) {
        log(&format!(
            "    {:>6} B  {:>2}  {}  {}",
            fs[i].bytes,
            d[i].unwrap_or(0),
            fs[i].owner.as_deref().unwrap_or("-"),
            fs[i].label
        ));
    }

    // The cluster.
    let mut sel = select(&fs, &e, &d, &name, lines);
    if let Some(c) = stored {
        keep_order(&mut sel, &c.symbols);
    }
    log(&format!("  cluster {name}: {} patterns, {} within {lines} lines", sel.picks.len(), kib(sel.bytes)));
    for (l, why) in sel.skipped.iter().take(top) {
        log(&format!("    - {l}: {why}"));
    }
    if sel.skipped.len() > top {
        log(&format!("    - … {} more", sel.skipped.len() - top));
    }
    let head = Head {
        name: &name,
        from: &from,
        stop: &stop,
        fits_with: &fits_with,
        starts_over: stored.and_then(|c| c.starts_over.as_deref()),
        budget: lines,
        note: format!(
            "{} B ({} lines) of paged code are reachable from the roots",
            paged_bytes,
            paged_bytes.div_ceil(LINE_SIZE)
        ),
    };
    let text = block(&head, &sel);
    if write {
        let old = std::fs::read_to_string(&hot_path)?;
        let new = update_hot(&old, &name, &text)?;
        std::fs::write(&hot_path, new)?;
        log(&format!("  wrote cluster {name} to {}", hot_path.display()));
    } else {
        log("  (not written; --write replaces or appends it in hot.toml):");
        for l in text.lines() {
            log(&format!("    {l}"));
        }
    }
    // The working set with this selection in place (as after a rebuild).
    let mut after = fs.clone();
    let picked: Vec<&str> = sel.picks.iter().map(|p| p.pattern.as_str()).collect();
    for f in after.iter_mut().filter(|f| f.paged) {
        if f.owner.as_deref() == Some(name.as_str()) {
            f.owner = None;
        }
        if f.owner.is_none() && picked.iter().any(|p| rules::symbol_matches(p, &f.label)) {
            f.owner = Some(name.clone());
        }
    }
    let w = working_set(&after, &d, &name, &fits_with);
    let verdict = format!(
        "{name}: {} reachable; cluster {} lines; working set {} lines with {} functions outside it and its fits_with ({}, {} lines: {} of {CACHE_LINES} cache lines)",
        kib(paged_bytes),
        sel.bytes.div_ceil(LINE_SIZE),
        w.lines,
        w.outside.len(),
        fits_with.join(", "),
        w.with_lines,
        w.lines + w.with_lines
    );
    Ok(verdict)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(label: &str, bytes: u32, paged: bool, owner: Option<&str>) -> Func {
        Func { label: label.into(), bytes, paged, owner: owner.map(str::to_string) }
    }

    fn graph(n: usize, e: &[(usize, usize)]) -> Vec<Vec<usize>> {
        let mut g = vec![Vec::new(); n];
        for &(a, b) in e {
            g[a].push(b);
        }
        g
    }

    #[test]
    fn distances_stop_and_roots() {
        // 0 -> 1 -> 2 -> 3; 0 -> 4 -> 3; 5 unreached; 4 stopped.
        let e = graph(6, &[(0, 1), (1, 2), (2, 3), (0, 4), (4, 3)]);
        assert_eq!(distances(&e, &[0], &[false; 6]), vec![Some(0), Some(1), Some(2), Some(2), Some(1), None]);
        let stop = [false, false, false, false, true, false];
        assert_eq!(distances(&e, &[0], &stop), vec![Some(0), Some(1), Some(2), Some(3), None, None]);
        // A root is entered even when stopped.
        assert_eq!(distances(&e, &[4], &stop)[3], Some(1));
    }

    #[test]
    fn selection_closest_first_within_the_budget() {
        // root (0) calls a (1), b (2, resident), c (3, owned by "fill");
        // a calls d (4, 1500 B) and e (5); b calls e; e is called twice.
        let funcs = vec![
            f("app::root", 300, true, None),
            f("app::a", 200, true, None),
            f("app::b", 100, false, None),
            f("fs::c", 400, true, Some("fill")),
            f("app::d", 1500, true, None),
            f("app::e", 100, true, None),
            f("app::cold", 100, true, None),
        ];
        let e = graph(7, &[(0, 1), (0, 2), (0, 3), (1, 4), (1, 5), (2, 5)]);
        let d = distances(&e, &[0], &[false; 7]);
        let s = select(&funcs, &e, &d, "io", 2);
        let names: Vec<&str> = s.picks.iter().map(|p| p.pattern.as_str()).collect();
        // Distance first; at distance 2, e (two callers) before d; d does
        // not fit 2 lines after the others (303 + 203 + 103 + 1503).
        assert_eq!(names, ["app::root", "app::a", "app::e"]);
        assert_eq!(s.bytes, 303 + 203 + 103);
        assert_eq!(s.picks[2].dist, 2);
        assert_eq!(
            s.skipped,
            vec![
                ("fs::c".to_string(), "in cluster fill".to_string()),
                ("app::d".to_string(), "1503 B do not fit (609 of 2048 B used)".to_string()),
            ]
        );
        // With room, d too; the unreached function never.
        let s = select(&funcs, &e, &d, "io", 3);
        assert_eq!(s.picks.len(), 4);
        assert!(s.picks.iter().all(|p| p.pattern != "app::cold"));
    }

    #[test]
    fn selection_keeps_its_own_and_skips_shared_names() {
        // Two sections named `x::g<_>` (instantiations), one of them in
        // cluster "walk": the name would move it, so it is skipped. A
        // function the cluster owns already stays in it.
        let funcs = vec![
            f("x::root", 100, true, Some("io")),
            f("x::g<_>", 100, true, None),
            f("x::g<_>", 100, true, Some("walk")),
            f("x::h*", 50, true, None),
        ];
        let e = graph(4, &[(0, 1), (0, 3)]);
        let d = distances(&e, &[0], &[false; 4]);
        let s = select(&funcs, &e, &d, "io", 8);
        let names: Vec<&str> = s.picks.iter().map(|p| p.pattern.as_str()).collect();
        assert_eq!(names, ["x::root", "x::h?"]);
        assert_eq!(s.skipped[0].1, "its name also selects a section of cluster walk");
    }

    #[test]
    fn working_set_counts_the_cluster_and_each_function_outside() {
        let funcs = vec![
            f("r", 900, true, Some("io")),
            f("in_io_unreached", 300, true, Some("io")),
            f("fill", 2000, true, Some("fill")),
            f("other", 1100, true, Some("block")),
            f("loose", 10, true, None),
            f("res", 5000, false, None),
        ];
        let e = graph(6, &[(0, 2), (0, 3), (0, 4), (0, 5)]);
        let d = distances(&e, &[0], &[false; 6]);
        let w = working_set(&funcs, &d, "io", &["fill".to_string()]);
        assert_eq!(w.own, 903 + 303);
        assert_eq!(w.with, 2003);
        assert_eq!(w.outside.len(), 2);
        // 2 lines of the cluster, 2 for `other`, 1 for `loose`; `fill`
        // whole (2 lines) beside them.
        assert_eq!(w.lines, 5);
        assert_eq!(w.with_lines, 2);
        assert!(check(&w, "io", 5).is_ok());
        let err = check(&w, "io", 4).unwrap_err();
        assert!(err.contains("5 lines, over its budget_lines 4"), "{err}");
        assert!(err.contains("other (1100 B)"), "{err}");
    }

    #[test]
    fn regeneration_keeps_the_order_in_hot_toml() {
        let p = |n: &str, dist: u32| Pick { pattern: n.into(), sections: 1, bytes: 10, dist };
        let mut s =
            Selection { picks: vec![p("a", 0), p("b", 1), p("new", 1), p("c", 2)], ..Selection::default() };
        keep_order(&mut s, &["c".to_string(), "gone".to_string(), "a".to_string(), "b".to_string()]);
        let names: Vec<&str> = s.picks.iter().map(|p| p.pattern.as_str()).collect();
        assert_eq!(names, ["c", "a", "b", "new"]);
    }

    /// `hot.toml`: `games-io` has its roots and a budget that fits the
    /// cache beside the clusters it runs with, and sits between them in
    /// file order (packing order: play-fill, games-io, dir-walk, then
    /// flac-frame back over mp3-frame's lines).
    #[test]
    fn hot_toml_games_io_is_checked_and_placed() {
        let hot = rules::load_hot(&root().join("hot.toml"), "firmware").unwrap();
        let at = |n: &str| hot.iter().position(|c| c.name == n).unwrap();
        let g = &hot[at("games-io")];
        assert!(!g.from.is_empty() && !g.symbols.is_empty());
        assert!(g.fits_with.iter().any(|f| f == "play-fill") && g.fits_with.iter().any(|f| f == "dir-walk"));
        // 32 physical lines less play-fill's 6 and dir-walk's 11.
        assert!(g.budget_lines.is_some_and(|b| b <= 15), "{:?}", g.budget_lines);
        assert!(at("play-fill") < at("games-io") && at("games-io") < at("dir-walk"));
        assert!(at("dir-walk") < at("flac-frame"));
        assert_eq!(hot[at("flac-frame")].starts_over.as_deref(), Some("mp3-frame"));
    }

    /// The working set and its `fits_with` clusters must fit the cache,
    /// whatever `budget_lines` says.
    #[test]
    fn working_set_with_its_fits_with_fits_the_cache() {
        let funcs = vec![f("root", 21_000, true, Some("a")), f("b", 12_000, true, Some("b"))];
        let e = graph(2, &[(0, 1)]);
        let d = distances(&e, &[0], &[false; 2]);
        let w = working_set(&funcs, &d, "a", &["b".to_string()]);
        assert_eq!((w.lines, w.with_lines), (21, 12));
        let err = check(&w, "a", 21).unwrap_err();
        assert!(err.contains("take 33 lines, more than the 32"), "{err}");
        // 32 fit.
        let funcs = vec![f("root", 20_000, true, Some("a")), f("b", 12_000, true, Some("b"))];
        let w = working_set(&funcs, &d, "a", &["b".to_string()]);
        assert_eq!((w.lines, w.with_lines), (20, 12));
        assert!(check(&w, "a", 20).is_ok());
    }

    /// SYN-3 (2026-11-26, the lead on the player, build `8240f128`): MIDI
    /// playback paged 200–250 % of real time (`status` `syn: pg=248`)
    /// and every demo song ended `synth: gave up, nothing can play at 288
    /// … pg=248%`. The render's cluster (`synth-render`, 16 lines) left
    /// nine functions it calls every 32-sample segment outside it (the
    /// filter design, `exp2`, pan gains, six drum kinds: 25 lines of
    /// budget 26, so `stack --check` passed), and it ran with the
    /// per-piece cluster `synth-midi` (9 lines): 34 lines for a 32-line
    /// cache, and the outside functions sat on the render's own physical
    /// lines. The check must fail that working set and pass the one built
    /// now: the render whole (18 lines), the per-piece code
    /// (`synth-inner`, 8) and the shared PCM path (`play-pcm`, 5).
    #[test]
    fn regress_synth_working_set_over_the_cache() {
        // Before: the cluster's 16 lines, nine functions outside (a line
        // each), with synth-midi.
        let mut funcs = vec![f("render_i16", 16 * 1024 - 3, true, Some("synth-render"))];
        for i in 0..9 {
            funcs.push(f(&format!("helper{i}"), 400, true, None));
        }
        funcs.push(f("plan_piece", 9 * 1024 - 3, true, Some("synth-midi")));
        let n = funcs.len();
        let e = graph(n, &(1..n).map(|i| (0, i)).collect::<Vec<_>>());
        let d = distances(&e, &[0], &vec![false; n]);
        let w = working_set(&funcs, &d, "synth-render", &["synth-midi".to_string()]);
        assert_eq!((w.lines, w.with_lines), (25, 9));
        let err = check(&w, "synth-render", 26).unwrap_err();
        assert!(err.contains("take 34 lines, more than the 32"), "{err}");
        // After: everything the render calls per segment in its cluster,
        // the per-piece code and the PCM path beside it.
        let funcs = vec![
            f("render", 18 * 1024 - 3, true, Some("synth-render")),
            f("pieces", 8 * 1024 - 3, true, Some("synth-inner")),
            f("feed", 5 * 1024 - 3, true, Some("play-pcm")),
        ];
        let e = graph(3, &[(1, 0), (1, 2)]);
        let d = distances(&e, &[1], &[false; 3]);
        let fits = ["synth-inner".to_string(), "play-pcm".to_string()];
        let w = working_set(&funcs, &d, "synth-render", &fits);
        assert_eq!((w.lines, w.with_lines), (18, 13));
        assert!(check(&w, "synth-render", 18).is_ok());
    }

    /// `hot.toml`: the synth's steady working set is checked and placed —
    /// `synth-render` has roots (the pieces' loop), runs with the per-piece
    /// code and the shared PCM path (not with a codec's clusters: a codec
    /// and the synth never run together), starts on `play-block`'s line
    /// (after `play-pcm`'s five), and the three fit the cache.
    #[test]
    fn hot_toml_synth_is_checked_and_placed() {
        let hot = rules::load_hot(&root().join("hot.toml"), "firmware").unwrap();
        let at = |n: &str| hot.iter().position(|c| c.name == n).unwrap();
        let r = &hot[at("synth-render")];
        assert!(!r.from.is_empty() && !r.stop.is_empty());
        assert_eq!(r.fits_with, ["synth-inner", "play-pcm"]);
        assert!(r.budget_lines.is_some_and(|b| b < CACHE_LINES), "{:?}", r.budget_lines);
        assert_eq!(r.starts_over.as_deref(), Some("play-block"));
        assert!(at("mp3-frame") < at("play-pcm") && at("play-pcm") < at("play-block"));
        assert!(at("flac-frame") < at("synth-render") && at("synth-render") < at("synth-inner"));
        for c in ["mp3-frame", "flac-frame"] {
            assert!(hot[at(c)].fits_with.iter().any(|f| f == "play-pcm"), "{c}");
        }
    }

    /// Regression (2026-11-26, the player, build `b694ac61`: "Etude" 18
    /// underruns at a render load of 24–46 %): the main executor's loop
    /// was inlined into `MainExecutor::run`, on physical lines 9–10 (the
    /// render's last and the per-piece code's first), so every slot of
    /// thread mode while a MIDI file played loaded two lines and the
    /// render loaded them again. Now its own function, packed after the
    /// key tick at the head of `synth-block` (the line the synth's wake-up
    /// entry is on), before `play_midi`'s poll.
    #[test]
    fn regress_main_loop_on_the_render_lines() {
        let hot = rules::load_hot(&root().join("hot.toml"), "firmware").unwrap();
        let b = hot.iter().find(|c| c.name == "synth-block").unwrap();
        let at = |p: &str| b.symbols.iter().position(|s| s.contains(p));
        let (tick, main, midi) = (at("__input_task"), at("executor::main_loop"), at("synth::play_midi"));
        assert!(tick.is_some() && main.is_some() && midi.is_some(), "{:?}", b.symbols);
        assert!(tick < main && main < midi, "{:?}", b.symbols);
    }

    /// AU-PERF games pieces (2026-11-25): a games card step while an MP3
    /// played (`serve: games 53 ms while playing pl=223 r=3 w=0`, 40 ms
    /// pl=170, 37 ms pl=156) loaded 156–223 code lines for 1–3 card
    /// blocks: the path's code was scattered over the whole image, in no
    /// cluster, and emptied the DAC ring (3 underruns in 6 game switches).
    /// The check must fail such a working set against an 8-line budget and
    /// pass it once the path is one cluster beside `play-fill`.
    #[test]
    fn regress_games_piece_working_set_over_its_line_budget() {
        // Before: 156 scattered functions of 600 B (one line each, the
        // smallest measured `pl=`), the card read path in play-fill.
        let mut funcs: Vec<Func> = (0..156).map(|i| f(&format!("io::f{i}"), 600, true, None)).collect();
        funcs.push(f("sd::read", 3000, true, Some("play-fill")));
        let n = funcs.len();
        let e = graph(n, &(1..n).map(|i| (0, i)).collect::<Vec<_>>());
        let d = distances(&e, &[0], &vec![false; n]);
        let fits = ["play-fill".to_string()];
        let w = working_set(&funcs, &d, "games-io", &fits);
        assert_eq!(w.lines, 156);
        assert!(check(&w, "games-io", 8).is_err());
        // After: the reachable functions picked into the cluster (here 12
        // of them fit 8 lines; the rest kept off the path, `stop`).
        let s = select(&funcs, &e, &d, "games-io", 8);
        assert_eq!(s.picks.len(), 13);
        let picked: Vec<String> = s.picks.iter().map(|p| p.pattern.clone()).collect();
        let mut stop = vec![false; n];
        for (i, fu) in funcs.iter_mut().enumerate() {
            if picked.contains(&fu.label) {
                fu.owner = Some("games-io".into());
            } else if fu.owner.is_none() {
                stop[i] = true;
            }
        }
        let d = distances(&e, &[0], &stop);
        let w = working_set(&funcs, &d, "games-io", &fits);
        assert_eq!((w.lines, w.with), (8, 3003));
        assert!(check(&w, "games-io", 8).is_ok());
    }
}
