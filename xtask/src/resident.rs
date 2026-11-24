//! `xtask check-resident` (docs/plan/07-tooling-testing.md §4.3): prove on
//! the *final* link that nothing reachable from the hard roots lies in a
//! page.
//!
//! Method: the reference graph of the final ELF (`linkgraph`: input
//! sections of the final map as nodes, `--emit-relocs` relocations decoded
//! from the linked bytes as edges: calls, tail calls, branches, literal
//! words, `MOVW`/`MOVT` address pairs, data pointers). Relocations rather
//! than a linear disassembly: they mark exactly the places that refer to
//! another section, so literal pools and jump tables cannot be mistaken
//! for instructions, and no instruction decoder beyond the branch and
//! immediate encodings is needed. What relocations cannot show are
//! indirect branches; the walk lists every `blx rN`/`bx rN`/`ldr pc` it
//! finds (Thumb sweep guided by the `$t`/`$d` mapping symbols) so their
//! targets can be named as roots in `resident.toml`.
//!
//! Roots are those of `resident.toml` (`@vectors` = every vector-table
//! entry except `vector_skip`, plus symbol patterns), resolved on the final
//! link. The walk follows every edge, through RAM data too (a static holding
//! a function pointer). It fails with the chain `root -> ... -> offender`
//! for every reachable node in a page window.

use std::path::Path;

use anyhow::{Result, bail};

use crate::args::Args;
use crate::linkgraph::{Graph, Kind};
use crate::linkmap;
use crate::pagelink::{Layout, SRAM0, artefacts, resolve_roots};
use crate::rules::Rules;
use crate::util::{log, root};

pub struct Report {
    pub roots: usize,
    pub reached: usize,
    /// Chains `root -> ... -> offender`.
    pub violations: Vec<Vec<String>>,
    /// `(function, address, instruction)`.
    pub indirect: Vec<(String, u32, String)>,
    /// Reachable panic entry points.
    pub panics: Vec<String>,
    pub unresolved: usize,
}

pub fn check(elf: &Path, map: &Path, bin: &str) -> Result<Report> {
    let m = linkmap::parse(&std::fs::read_to_string(map)?)?;
    let g = Graph::load(elf, m)?;
    let rules = Rules::load(&root().join("resident.toml"), bin)?;
    let roots = resolve_roots(&g, &rules);
    let root_nodes: Vec<usize> = roots.iter().flat_map(|(_, v)| v.iter().copied()).collect();
    for (p, v) in &roots {
        if v.is_empty() {
            log(&format!("  warning: root `{p}` matches nothing in {}", elf.display()));
        }
    }
    let parent = g.walk(&root_nodes);
    let mut r = Report {
        roots: root_nodes.len(),
        reached: parent.iter().filter(|p| p.is_some()).count(),
        violations: Vec::new(),
        indirect: Vec::new(),
        panics: Vec::new(),
        unresolved: g.unresolved.len(),
    };
    for n in 0..g.nodes.len() {
        if parent[n].is_none() {
            continue;
        }
        let s = &g.nodes[n];
        if yoton_pager::Page::of(s.addr).is_some() || yoton_pager::Page::of(s.addr + s.size - 1).is_some() {
            let mut chain = g.chain(&parent, n);
            if let Some(last) = chain.last_mut() {
                *last = format!("{last} [{} at {:#x}]", s.out, s.addr);
            }
            r.violations.push(chain);
        }
        let label = g.label(n);
        if label.starts_with("core::panicking::") {
            r.panics.push(label.clone());
        }
        if g.kinds[n] == Kind::Code && SRAM0.contains(&s.addr) {
            for (a, d) in g.indirect_branches(n) {
                r.indirect.push((label.clone(), a, d));
            }
        }
    }
    Ok(r)
}

pub fn cmd(args: &mut Args) -> Result<String> {
    let bin = args.value("bin").unwrap_or_else(|| "firmware".into());
    let a = artefacts(&bin);
    let elf = args.value("elf").map_or(a.elf, Into::into);
    let map = args.value("map").map_or(a.map, Into::into);
    if !elf.exists() || !map.exists() {
        bail!("{} / {} missing; run `cargo xtask build` (or pagelink) first", elf.display(), map.display());
    }
    if let Ok(l) = Layout::load(&a.layout) {
        log(&format!("{bin}: {} code + {} rodata pages (layout.json)", l.pages.code, l.pages.rodata));
    }
    let r = check(&elf, &map, &bin)?;
    print(&r);
    if r.roots == 0 {
        // The placeholder firmware has no vector table yet.
        return Ok("no root resolves in this image (no vector table?): nothing to check".into());
    }
    if r.violations.is_empty() {
        Ok(format!(
            "ok, {} sections reachable from {} roots, none paged; {} indirect branches listed",
            r.reached,
            r.roots,
            r.indirect.len()
        ))
    } else {
        bail!("{} paged sections reachable from the roots", r.violations.len())
    }
}

pub fn print(r: &Report) {
    for v in &r.violations {
        log(&format!("  PAGED: {}", v.join(" -> ")));
    }
    if !r.indirect.is_empty() {
        log("  indirect branches reachable from the roots (their targets must be roots):");
        for (f, a, d) in &r.indirect {
            log(&format!("    {a:#010x}  {d:<18} in {f}"));
        }
    }
    if !r.panics.is_empty() {
        log(&format!("  panic paths reachable: {}", r.panics.join(", ")));
    }
    if r.unresolved > 0 {
        log(&format!("  {} relocations not decoded", r.unresolved));
    }
}
