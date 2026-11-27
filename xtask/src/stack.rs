//! `xtask stack [--check] [--top N] [--root PATTERN] [--elf PATH]`
//! (07 §4.4, VER-10): static worst-case depth of the one MSP stack.
//!
//! Method:
//!
//! * **Call graph**: the reference graph of the final link (`linkgraph`,
//!   the one `check-resident` walks), restricted to direct branches between
//!   code sections: `BL`/`BLX` are calls (the caller's frame stays live),
//!   `B`/`B<cond>` to another function are tail calls (the frame is popped
//!   before the branch; the callee's depth replaces it).
//! * **Frames**: per function, the deepest its own SP adjustments go
//!   (`metrics::parse_functions` over `llvm-objdump -d`: pushes, `vpush`,
//!   `sub sp`, with pops and epilogues subtracted in address order). The
//!   DWARF CFI does not help: after the prologue it follows the frame
//!   pointer (`CFA = r7 + 8`), not SP.
//! * **Indirect branches** (`blx rN`, `bx rN`, `ldr pc`) cannot be followed.
//!   `stack.toml` names their targets per calling function (or a byte bound
//!   with the reason), globally or per context; the executors' task polls
//!   and the interrupt dispatcher's handlers are resolved that way. A depth
//!   that reaches an unresolved site is a **lower bound** (`>=`), and the
//!   site is listed with the instruction that loaded the branch register
//!   (a waker vtable slot `[rX, #4]`, a trait-object method, an argument).
//! * **Recursion** (a cycle, a function branching to itself) is reported.
//!   The depth counts one pass through the cycle and is a lower bound,
//!   unless `stack.toml` lists the cycle as bounded (`[[recursion]]`).
//! * **Nesting**: one stack takes thread mode and every exception. The
//!   contexts of `stack.toml` have priorities (thread, then the NVIC levels
//!   3, 2, 1, 0 of `atj2157_hal::interrupt`); the worst case stacks the
//!   deepest context of each priority on the one below, each exception
//!   with its hardware frame (FPU context stacked eagerly: 26 words, plus
//!   4 B of alignment padding). A fault stacks one more frame and continues
//!   on the emergency stack, whose own depth is checked against its size.
//! * **Budget**: the total against the stack above the MPU guard
//!   (`__stack_top` − (`__stack_bottom` rounded up to the guard size) −
//!   the guard). `--check` fails when the margin is below `budgets.toml`
//!   `[firmware] stack_static_margin_bytes`, when a `stack.toml` pattern
//!   matches nothing, when a task poll is in no context, when an indirect
//!   site is unresolved (the total would be a lower bound) or when a
//!   context on its own stack overflows it.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use object::{Object, ObjectSymbol};
use serde::Deserialize;

use crate::args::Args;
use crate::linkgraph::{Graph, Kind, demangle};
use crate::linkmap::{self, InputSection};
use crate::pagelink::artefacts;
use crate::rules::symbol_matches;
use crate::util::{log, root};

// ---------------------------------------------------------------------------
// Configuration (`stack.toml`).

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Bytes the hardware stacks per exception (extended frame + padding).
    pub exception_frame_bytes: u32,
    /// MPU guard size (also its alignment) at the bottom of the stack.
    pub guard_bytes: u32,
    #[serde(default)]
    pub context: Vec<ContextCfg>,
    /// Indirect call targets valid in every context.
    #[serde(default)]
    pub indirect: Vec<IndirectCfg>,
    /// Cycles known to be bounded.
    #[serde(default)]
    pub recursion: Vec<RecursionCfg>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextCfg {
    pub name: String,
    /// 4 = thread mode, 0..=3 = NVIC level (0 most urgent).
    pub priority: u8,
    /// Function the context enters (symbol pattern).
    pub entry: String,
    /// `"emergency"`: the context switches to its own stack of
    /// `stack_bytes` before it calls anything; the main stack only takes
    /// its exception frame.
    #[serde(default)]
    pub stack: Option<String>,
    #[serde(default)]
    pub stack_bytes: Option<u32>,
    /// The entry may be absent (a feature build: the profiler).
    #[serde(default)]
    pub optional: bool,
    /// The phase the context exists in (`"init"`: `main` before it starts
    /// the executors; `"run"`: afterwards); none: every phase. The nesting
    /// is computed per phase.
    #[serde(default)]
    pub phase: Option<String>,
    /// Functions the walk does not enter (what belongs to another phase).
    #[serde(default)]
    pub stop: Vec<String>,
    /// Functions whose frames stay live below the entry (the caller of an
    /// entry that never returns).
    #[serde(default)]
    pub under: Vec<String>,
    /// Indirect call targets valid in this context only (the executors'
    /// task polls, the dispatcher's handlers). Their targets are the
    /// context's reported roots.
    #[serde(default)]
    pub indirect: Vec<IndirectCfg>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndirectCfg {
    /// Functions (symbol patterns) containing the indirect branch(es).
    pub callers: Vec<String>,
    /// What they can call (symbol patterns).
    #[serde(default)]
    pub targets: Vec<String>,
    /// Or: the callees' worst depth in bytes (needs `why`).
    #[serde(default)]
    pub bound: Option<u32>,
    #[serde(default)]
    pub why: String,
    /// Patterns that must match nothing for the entry to hold (a bound
    /// that relies on no implementation being linked).
    #[serde(default)]
    pub absent: Vec<String>,
    /// The callers may all be absent (a feature build: Books): then the
    /// entry is skipped, not a problem. Once one caller is linked, every
    /// target pattern must match as for any entry.
    #[serde(default)]
    pub optional: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecursionCfg {
    /// A function on the cycle (symbol pattern).
    pub through: String,
    pub why: String,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let t = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let c: Self = toml::from_str(&t).with_context(|| format!("parse {}", path.display()))?;
        for e in c.indirect.iter().chain(c.context.iter().flat_map(|c| c.indirect.iter())) {
            if e.bound.is_some() && e.why.trim().is_empty() {
                bail!("stack.toml: the bound for {:?} needs a `why`", e.callers);
            }
            if e.bound.is_none() && e.targets.is_empty() {
                bail!("stack.toml: {:?} has neither targets nor a bound", e.callers);
            }
        }
        if let Some(r) = c.recursion.iter().find(|r| r.why.trim().is_empty()) {
            bail!("stack.toml: the bounded recursion through `{}` needs a `why`", r.through);
        }
        Ok(c)
    }
}

// ---------------------------------------------------------------------------
// The analysis (host-testable on a synthetic graph).

/// Call graph with frame sizes: node `i` is one function.
#[derive(Clone, Debug, Default)]
pub struct CallGraph {
    pub names: Vec<String>,
    pub frame: Vec<u32>,
    /// `(callee, tail)`.
    pub calls: Vec<Vec<(usize, bool)>>,
    /// Indirect branch sites: `(address, instruction)`.
    pub sites: Vec<Vec<(u32, String)>>,
    /// Re-entering this node closes a cycle known to be bounded: taken
    /// twice, not more (a re-entry guard).
    pub bounded_cycle: Vec<bool>,
}

/// How the indirect sites of one function are resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolve {
    Targets(Vec<usize>),
    Bound(u32),
}

/// One step of a worst-case chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Call(usize),
    Tail(usize),
    Indirect(usize),
    Bound(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    pub depth: u32,
    pub next: Option<Step>,
    /// An unresolved indirect site or an unbounded cycle is reachable.
    pub lower: bool,
}

#[derive(Clone)]
enum State {
    New,
    Active,
    Done(Info),
}

/// Depth-first worst case over `cg` with one resolver; memoised.
pub struct Walk<'a> {
    cg: &'a CallGraph,
    res: &'a BTreeMap<usize, Vec<Resolve>>,
    state: Vec<State>,
    path: Vec<usize>,
    /// Cycles found (node lists, first = re-entered node) and whether they
    /// are known to be bounded.
    pub cycles: Vec<(Vec<usize>, bool)>,
    /// Reached nodes with indirect sites nothing resolves.
    pub unresolved: Vec<usize>,
}

impl<'a> Walk<'a> {
    pub fn new(cg: &'a CallGraph, res: &'a BTreeMap<usize, Vec<Resolve>>) -> Self {
        Self {
            cg,
            res,
            state: vec![State::New; cg.names.len()],
            path: Vec::new(),
            cycles: Vec::new(),
            unresolved: Vec::new(),
        }
    }

    /// Do not enter `n` (it counts 0 B and is never on a chain).
    pub fn stop(&mut self, n: usize) {
        self.state[n] = State::Done(Info { depth: 0, next: None, lower: false });
    }

    pub fn depth(&mut self, n: usize) -> Info {
        match &self.state[n] {
            State::Done(i) => return i.clone(),
            State::Active => {
                let at = self.path.iter().rposition(|&p| p == n).unwrap_or(0);
                let cycle = self.path[at..].to_vec();
                let bounded = cycle.iter().any(|&c| self.cg.bounded_cycle.get(c).copied().unwrap_or(false));
                if !self.cycles.iter().any(|(c, _)| *c == cycle) {
                    self.cycles.push((cycle.clone(), bounded));
                }
                // A bounded cycle is entered once more (its re-entry guard
                // stops the third pass): the frames along it again.
                let again = if bounded { cycle.iter().map(|&c| self.cg.frame[c]).sum() } else { 0 };
                return Info { depth: again, next: None, lower: !bounded };
            }
            State::New => {}
        }
        self.state[n] = State::Active;
        self.path.push(n);
        let f = self.cg.frame[n];
        let mut best = Info { depth: f, next: None, lower: false };
        let take = |best: &mut Info, d: u32, step: Step, lower: bool| {
            best.lower |= lower;
            if d > best.depth {
                best.depth = d;
                best.next = Some(step);
            }
        };
        for &(c, tail) in &self.cg.calls[n] {
            let i = self.depth(c);
            if tail {
                take(&mut best, i.depth, Step::Tail(c), i.lower);
            } else {
                take(&mut best, f + i.depth, Step::Call(c), i.lower);
            }
        }
        if !self.cg.sites[n].is_empty() {
            match self.res.get(&n) {
                Some(rs) => {
                    for r in rs {
                        match r {
                            Resolve::Targets(ts) => {
                                for &t in ts {
                                    let i = self.depth(t);
                                    take(&mut best, f + i.depth, Step::Indirect(t), i.lower);
                                }
                            }
                            Resolve::Bound(b) => take(&mut best, f + b, Step::Bound(*b), false),
                        }
                    }
                }
                None => {
                    best.lower = true;
                    self.unresolved.push(n);
                }
            }
        }
        self.path.pop();
        self.state[n] = State::Done(best.clone());
        best
    }

    /// The worst chain from `n`: `(node, bytes it adds, how it was
    /// entered)`; a trailing bound has `node = usize::MAX`.
    pub fn chain(&mut self, n: usize) -> Vec<(usize, u32, Option<Step>)> {
        let mut out: Vec<(usize, u32, Option<Step>)> = Vec::new();
        let mut cur = Some((n, None));
        while let Some((m, how)) = cur.take() {
            if out.len() > 256 || out.iter().any(|&(o, ..)| o == m) {
                break;
            }
            let i = self.depth(m);
            let f = self.cg.frame[m];
            match i.next {
                Some(Step::Tail(c)) => {
                    out.push((m, 0, how));
                    cur = Some((c, Some(Step::Tail(c))));
                }
                Some(s @ (Step::Call(c) | Step::Indirect(c))) => {
                    out.push((m, f, how));
                    cur = Some((c, Some(s)));
                }
                Some(Step::Bound(b)) => {
                    out.push((m, f, how));
                    out.push((usize::MAX, b, Some(Step::Bound(b))));
                }
                None => out.push((m, f, how)),
            }
        }
        out
    }
}

/// Stack the contexts: the deepest of each priority on the one below,
/// thread mode (priority ≥ 4) without an exception frame, every exception
/// level with `frame`, plus `faults` frames for contexts that run on their
/// own stack. Rows `(priority, name, depth)`; returns the total and the
/// summands `(priority, name, bytes incl. frame)`, thread first.
pub fn nest(rows: &[(u8, String, u32)], frame: u32, faults: u32) -> (u32, Vec<(u8, String, u32)>) {
    let mut by_prio: BTreeMap<std::cmp::Reverse<u8>, (String, u32)> = BTreeMap::new();
    for (p, name, d) in rows {
        let e = by_prio.entry(std::cmp::Reverse(*p)).or_insert_with(|| (name.clone(), 0));
        if *d >= e.1 {
            *e = (name.clone(), *d);
        }
    }
    let mut total = 0;
    let mut parts = Vec::new();
    for (std::cmp::Reverse(p), (name, d)) in by_prio {
        let add = if p >= 4 { d } else { d + frame };
        total += add;
        parts.push((p, name, add));
    }
    (total + faults * frame, parts)
}

// ---------------------------------------------------------------------------
// The firmware.

pub(crate) struct Analysis {
    pub cg: CallGraph,
    /// Demangled symbols per node (pattern lookups).
    labels: Vec<Vec<String>>,
    /// Instruction text by address (site context).
    insn: BTreeMap<u32, String>,
    /// Stack region: (bottom, top).
    stack: (u32, u32),
    /// The input sections (`xtask hot`: sizes, placement, cluster owners).
    pub secs: Vec<InputSection>,
    pub kinds: Vec<Kind>,
}

pub(crate) fn load(elf: &Path, map: &Path) -> Result<Analysis> {
    let m =
        linkmap::parse(&std::fs::read_to_string(map).with_context(|| format!("read {}", map.display()))?)?;
    let g = Graph::load(elf, m)?;
    let text = crate::metrics::disassemble(elf)?;
    let funcs = crate::metrics::parse_functions(&text);
    let n = g.nodes.len();
    let mut frame = vec![0u32; n];
    let mut self_call = vec![false; n];
    for f in &funcs {
        if let Some(a) = f.addr
            && let Some(i) = g.node_at(a)
        {
            frame[i] = frame[i].max(u32::try_from(f.peak).unwrap_or(u32::MAX));
            self_call[i] |= f.self_call;
        }
    }
    let mut calls = g.calls.clone();
    for (i, c) in calls.iter_mut().enumerate() {
        if self_call[i] && !c.iter().any(|&(t, _)| t == i) {
            c.push((i, false));
        }
    }
    let sites: Vec<Vec<(u32, String)>> =
        (0..n).map(|i| if g.kinds[i] == Kind::Code { g.indirect_branches(i) } else { Vec::new() }).collect();
    let mut insn = BTreeMap::new();
    for l in text.lines().filter(|l| l.starts_with("  ")) {
        if let Some((a, rest)) = l.trim().split_once(':')
            && let Ok(a) = u32::from_str_radix(a, 16)
        {
            insn.insert(
                a,
                rest.split(" @")
                    .next()
                    .unwrap_or("")
                    .split('<')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .replace('\t', " "),
            );
        }
    }
    let names = (0..n).map(|i| g.label(i)).collect();
    let labels = g.nodes.iter().map(|s| s.symbols.iter().map(|y| demangle(&y.name)).collect()).collect();
    let data = std::fs::read(elf)?;
    let file = object::File::parse(&*data)?;
    let sym = |name: &str| file.symbols().find(|s| s.name() == Ok(name)).map(|s| s.address() as u32);
    let stack = match (sym("__stack_bottom"), sym("__stack_top")) {
        (Some(b), Some(t)) => (b, t),
        _ => bail!("{}: no __stack_bottom/__stack_top", elf.display()),
    };
    let cg = CallGraph { names, frame, calls, sites, bounded_cycle: vec![false; n] };
    Ok(Analysis { cg, labels, insn, stack, secs: g.nodes, kinds: g.kinds })
}

impl Analysis {
    pub(crate) fn find(&self, pat: &str) -> Vec<usize> {
        (0..self.cg.names.len())
            .filter(|&i| self.labels[i].iter().any(|l| l == pat || symbol_matches(pat, l)))
            .collect()
    }

    /// The instruction before `addr` that set `reg` (within 8 instructions).
    fn loaded_by(&self, addr: u32, insn: &str) -> String {
        let reg = insn.rsplit(' ').next().unwrap_or("").trim_end_matches(',');
        for (_, t) in self.insn.range(addr.saturating_sub(24)..addr).rev() {
            let mut p = t.split_whitespace();
            let (op, dst) = (p.next().unwrap_or(""), p.next().unwrap_or("").trim_end_matches(','));
            if dst == reg && (op.starts_with("ldr") || op.starts_with("mov")) {
                return t.clone();
            }
        }
        String::new()
    }

    /// Resolver from `[[indirect]]` entries and the targets they name;
    /// patterns that match nothing go to `missing`.
    pub(crate) fn resolver(
        &self,
        entries: &[&IndirectCfg],
        missing: &mut Vec<String>,
    ) -> (BTreeMap<usize, Vec<Resolve>>, Vec<usize>) {
        let mut res: BTreeMap<usize, Vec<Resolve>> = BTreeMap::new();
        let mut roots = Vec::new();
        for e in entries {
            if e.optional && e.callers.iter().all(|c| self.find(c).is_empty()) {
                continue;
            }
            let mut targets = Vec::new();
            for t in &e.targets {
                let v = self.find(t);
                if v.is_empty() {
                    missing.push(format!("target `{t}` matches nothing"));
                }
                targets.extend(v);
            }
            for p in &e.absent {
                if let Some(&n) = self.find(p).first() {
                    missing.push(format!(
                        "`{p}` must match nothing for the bound on {:?} but matches `{}` (list the targets)",
                        e.callers, self.cg.names[n]
                    ));
                }
            }
            for c in &e.callers {
                let callers = self.find(c);
                if callers.is_empty() {
                    missing.push(format!("caller `{c}` matches nothing"));
                }
                for c in callers {
                    let r = res.entry(c).or_default();
                    if !targets.is_empty() {
                        r.push(Resolve::Targets(targets.clone()));
                    }
                    if let Some(b) = e.bound {
                        r.push(Resolve::Bound(b));
                    }
                }
            }
            roots.extend(targets);
        }
        (res, roots)
    }
}

fn ge(lower: bool) -> &'static str {
    if lower { ">=" } else { "" }
}

fn prio(p: u8) -> String {
    if p >= 4 { "thr".into() } else { format!("L{p}") }
}

fn print_chain(a: &Analysis, w: &mut Walk<'_>, n: usize) {
    log("        frame  total       function");
    let mut total = 0;
    for (m, add, how) in w.chain(n) {
        total += add;
        let how = match how {
            Some(Step::Tail(_)) => "tail",
            Some(Step::Indirect(_)) => "ind.",
            Some(Step::Bound(_)) => "bnd.",
            _ => "    ",
        };
        let name = if m == usize::MAX {
            "(indirect callees: bound from stack.toml)"
        } else {
            a.cg.names[m].as_str()
        };
        log(&format!("      {add:>7} {total:>6}  {how}  {name}"));
    }
}

struct CtxResult {
    name: String,
    priority: u8,
    emergency: Option<u32>,
    phase: Option<String>,
    entry: usize,
    /// Bytes of the `under` frames (in `worst`, not in the chain).
    under: u32,
    stop: Vec<usize>,
    worst: Info,
    /// `(root, info)`, deepest first.
    roots: Vec<(usize, Info)>,
}

/// The nested worst case of one phase.
struct Nesting {
    phase: Option<String>,
    total: u32,
    lower: bool,
    /// The summands, for the report.
    sum: String,
}

/// Everything `xtask stack` found.
struct Report {
    /// Stack bytes above the guard.
    usable: u32,
    size: u32,
    results: Vec<CtxResult>,
    resolvers: Vec<BTreeMap<usize, Vec<Resolve>>>,
    unresolved: BTreeMap<usize, Vec<String>>,
    cycles: Vec<(Vec<usize>, bool)>,
    nesting: Vec<Nesting>,
    /// The deepest phase's total.
    total: u32,
    lower: bool,
    /// What `--check` fails on besides the margin.
    errors: Vec<String>,
}

impl Report {
    fn margin(&self) -> i64 {
        i64::from(self.usable) - i64::from(self.total)
    }
}

/// Load `stack.toml`, mark its bounded cycles; the global indirect entries.
pub(crate) fn prepare(an: &mut Analysis, cfg: &Config, missing: &mut Vec<String>) {
    for r in &cfg.recursion {
        let v = an.find(&r.through);
        if v.is_empty() {
            missing.push(format!("recursion `{}` matches nothing", r.through));
        }
        for n in v {
            an.cg.bounded_cycle[n] = true;
        }
    }
}

fn analyse(an: &mut Analysis, cfg: &Config) -> Report {
    let mut missing = Vec::new();
    prepare(an, cfg, &mut missing);
    let global: Vec<&IndirectCfg> = cfg.indirect.iter().collect();
    let (bottom, top_addr) = an.stack;
    let g = cfg.guard_bytes.max(1);
    let usable = top_addr.saturating_sub(bottom.div_ceil(g) * g + g);

    let mut results: Vec<CtxResult> = Vec::new();
    let mut resolvers = Vec::new();
    let mut unresolved: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut cycles: Vec<(Vec<usize>, bool)> = Vec::new();
    let mut task_roots: Vec<usize> = Vec::new();
    for c in &cfg.context {
        if c.optional && an.find(&c.entry).is_empty() {
            continue;
        }
        let mut entries = global.clone();
        entries.extend(c.indirect.iter());
        let (res, _) = an.resolver(&entries, &mut missing);
        let local: Vec<&IndirectCfg> = c.indirect.iter().collect();
        let (_, roots) = an.resolver(&local, &mut Vec::new());
        let entry = an.find(&c.entry);
        if entry.len() != 1 {
            missing.push(format!("entry `{}` has {} matches, needs 1", c.entry, entry.len()));
            continue;
        }
        let mut stop = Vec::new();
        for p in &c.stop {
            let v = an.find(p);
            if v.is_empty() {
                missing.push(format!("stop `{p}` matches nothing"));
            }
            stop.extend(v);
        }
        let mut under = 0;
        for p in &c.under {
            let v = an.find(p);
            if v.is_empty() {
                missing.push(format!("under `{p}` matches nothing"));
            }
            under += v.iter().map(|&n| an.cg.frame[n]).max().unwrap_or(0);
        }
        let mut w = Walk::new(&an.cg, &res);
        for &s in &stop {
            w.stop(s);
        }
        let mut worst = w.depth(entry[0]);
        worst.depth += under;
        let mut rs: Vec<(usize, Info)> = roots.iter().map(|&r| (r, w.depth(r))).collect();
        rs.sort_by_key(|x| std::cmp::Reverse(x.1.depth));
        rs.dedup_by_key(|r| r.0);
        task_roots.extend(roots.iter().copied().filter(|&r| an.cg.names[r].contains("TaskStorage<")));
        for &u in &w.unresolved {
            unresolved.entry(u).or_default().push(c.name.clone());
        }
        for cy in &w.cycles {
            if !cycles.iter().any(|x| x.0 == cy.0) {
                cycles.push(cy.clone());
            }
        }
        let emergency = (c.stack.as_deref() == Some("emergency")).then(|| c.stack_bytes.unwrap_or(0));
        results.push(CtxResult {
            name: c.name.clone(),
            priority: c.priority,
            emergency,
            phase: c.phase.clone(),
            entry: entry[0],
            under,
            stop,
            worst,
            roots: rs,
        });
        resolvers.push(res);
    }

    let mut errors = Vec::new();
    for i in 0..an.cg.names.len() {
        let n = &an.cg.names[i];
        if n.contains("TaskStorage<") && n.ends_with(">::poll") && !task_roots.contains(&i) {
            errors.push(format!("task `{n}` is in no context of stack.toml"));
        }
    }
    missing.sort();
    missing.dedup();
    errors.extend(missing.iter().map(|m| format!("stack.toml: {m}")));
    if !unresolved.is_empty() {
        errors.push(format!(
            "{} functions with unresolved indirect branches (stack.toml [[indirect]])",
            unresolved.len()
        ));
    }
    for (cy, bounded) in &cycles {
        if !bounded {
            errors.push(format!("recursion through {}", an.cg.names[cy[0]]));
        }
    }
    for r in &results {
        if let Some(size) = r.emergency
            && r.worst.depth > size
        {
            errors.push(format!("{}: {} B > its own {size} B stack", r.name, r.worst.depth));
        }
    }

    // Nesting, per phase.
    let frame = cfg.exception_frame_bytes;
    let mut phases: Vec<Option<&str>> = results.iter().filter_map(|r| r.phase.as_deref()).map(Some).collect();
    phases.sort_unstable();
    phases.dedup();
    if phases.is_empty() {
        phases.push(None);
    }
    let mut nesting = Vec::new();
    for ph in &phases {
        let here: Vec<&CtxResult> = results
            .iter()
            .filter(|r| ph.is_none() || r.phase.is_none() || r.phase.as_deref() == *ph)
            .collect();
        let main_rows: Vec<(u8, String, u32)> = here
            .iter()
            .filter(|r| r.emergency.is_none())
            .map(|r| (r.priority, r.name.clone(), r.worst.depth))
            .collect();
        let faults = here.iter().filter(|r| r.emergency.is_some()).count() as u32;
        let (total, parts) = nest(&main_rows, frame, faults);
        let mut sum = Vec::new();
        for (p, name, add) in &parts {
            sum.push(if *p >= 4 {
                format!("{name} {add}")
            } else {
                format!("{} {name} {} + {frame}", prio(*p), add - frame)
            });
        }
        for _ in 0..faults {
            sum.push(format!("fault frame {frame}"));
        }
        nesting.push(Nesting {
            phase: ph.map(str::to_string),
            total,
            lower: here.iter().any(|r| r.worst.lower),
            sum: sum.join(" + "),
        });
    }
    let total = nesting.iter().map(|n| n.total).max().unwrap_or(0);
    let lower = results.iter().any(|r| r.worst.lower);
    Report {
        usable,
        size: top_addr - bottom,
        results,
        resolvers,
        unresolved,
        cycles,
        nesting,
        total,
        lower,
        errors,
    }
}

pub(crate) fn artefact_paths(args: &mut Args) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    let a = artefacts("firmware");
    let elf = args.value("elf").map_or(a.elf, Into::into);
    let map = args.value("map").map_or(a.map, Into::into);
    if !elf.exists() || !map.exists() {
        bail!("{} / {} missing; run `cargo xtask build` first", elf.display(), map.display());
    }
    Ok((elf, map))
}

/// The metrics of `xtask metrics` (07 §4.4): `(key, bytes)` — the worst
/// case of every context, `nested` (the deepest phase's total) and `usable`.
pub fn metrics(elf: &Path, map: &Path) -> Result<Vec<(String, u64)>> {
    let cfg = Config::load(&root().join("stack.toml"))?;
    let mut an = load(elf, map)?;
    let r = analyse(&mut an, &cfg);
    let mut out: Vec<(String, u64)> =
        r.results.iter().map(|c| (c.name.clone(), u64::from(c.worst.depth))).collect();
    out.push(("nested".into(), u64::from(r.total)));
    out.push(("usable".into(), u64::from(r.usable)));
    Ok(out)
}

/// `--root PATTERN`: the depth, chain and deepest callees of each match.
fn probe(an: &mut Analysis, cfg: &Config, p: &str) -> Result<String> {
    let mut missing = Vec::new();
    prepare(an, cfg, &mut missing);
    let global: Vec<&IndirectCfg> = cfg.indirect.iter().collect();
    let (res, _) = an.resolver(&global, &mut missing);
    let nodes = an.find(p);
    if nodes.is_empty() {
        bail!("`{p}` matches nothing");
    }
    let mut w = Walk::new(&an.cg, &res);
    for &n in &nodes {
        let i = w.depth(n);
        log(&format!("  {}{} B  {}", ge(i.lower), i.depth, an.cg.names[n]));
        if nodes.len() > 8 {
            continue;
        }
        print_chain(an, &mut w, n);
        let f = an.cg.frame[n];
        let mut fan: Vec<(u32, &str, usize)> = Vec::new();
        for &(c, tail) in &an.cg.calls[n] {
            let d = w.depth(c).depth;
            fan.push((if tail { d } else { f + d }, if tail { "tail" } else { "call" }, c));
        }
        if let Some(rs) = res.get(&n) {
            let targets = rs.iter().filter_map(|r| if let Resolve::Targets(t) = r { Some(t) } else { None });
            for t in targets.flatten() {
                fan.push((f + w.depth(*t).depth, "ind.", *t));
            }
        }
        fan.sort_by_key(|a| std::cmp::Reverse(a.0));
        fan.dedup_by_key(|x| x.2);
        log("      via callees:");
        for (d, how, c) in fan.iter().take(16) {
            log(&format!("      {d:>7}  {how}  {}", an.cg.names[*c]));
        }
    }
    Ok(format!("{} functions", nodes.len()))
}

pub fn cmd(args: &mut Args) -> Result<String> {
    let check = args.flag("check");
    let top: usize = args.value("top").map_or(Ok(6), |v| v.parse()).context("--top N")?;
    let probe_pat = args.value("root");
    let (elf, map) = artefact_paths(args)?;
    let cfg = Config::load(&root().join("stack.toml"))?;
    let mut an = load(&elf, &map)?;
    if let Some(p) = probe_pat {
        return probe(&mut an, &cfg, &p);
    }
    let r = analyse(&mut an, &cfg);
    log(&format!(
        "stack {:#x}..{:#x}: {} B, {} B usable above the {} B MPU guard",
        an.stack.0, an.stack.1, r.size, r.usable, cfg.guard_bytes
    ));

    // Contexts and their roots.
    log("  context      prio  worst (B)  roots");
    for c in &r.results {
        let own = c.emergency.map_or(String::new(), |s| format!(" (own {s} B stack)"));
        let phase = c.phase.as_ref().map_or(String::new(), |p| format!(" [{p}]"));
        log(&format!(
            "  {:<12} {:>4}  {:>9}  {}{own}{phase}",
            c.name,
            prio(c.priority),
            format!("{}{}", ge(c.worst.lower), c.worst.depth),
            an.cg.names[c.entry]
        ));
        for (n, i) in &c.roots {
            log(&format!("  {:>27}    {}", format!("{}{}", ge(i.lower), i.depth), an.cg.names[*n]));
        }
    }
    // The deepest chains: every context's, then its roots', deepest first,
    // `top` in all; a root whose chain was shown as part of another is
    // skipped.
    let mut order: Vec<(u32, usize, usize)> = Vec::new(); // (depth, ctx, node)
    for (ci, c) in r.results.iter().enumerate() {
        order.push((c.worst.depth, ci, c.entry));
        for (n, i) in &c.roots {
            order.push((i.depth, ci, *n));
        }
    }
    order.sort_by_key(|x| std::cmp::Reverse(x.0));
    let mut shown: Vec<usize> = Vec::new();
    let mut chains = 0;
    for &(d, ci, n) in &order {
        if chains >= top {
            break;
        }
        let c = &r.results[ci];
        let mut w = Walk::new(&an.cg, &r.resolvers[ci]);
        for &s in &c.stop {
            w.stop(s);
        }
        let first = w.chain(n).get(1).map(|x| x.0);
        if shown.contains(&n) || first.is_some_and(|f| shown.contains(&f)) && c.entry != n {
            continue;
        }
        let under = if c.entry == n && c.under > 0 {
            format!(" (+ {} B of frames below the entry)", c.under)
        } else {
            String::new()
        };
        log(&format!("  chain {}: {} ({}), {d} B{under}", chains + 1, an.cg.names[n], c.name));
        print_chain(&an, &mut w, n);
        shown.extend(w.chain(n).iter().map(|x| x.0).filter(|&x| x != usize::MAX));
        chains += 1;
    }

    if !r.unresolved.is_empty() {
        log(&format!(
            "  unresolved indirect branches ({} functions; depths through them are lower bounds):",
            r.unresolved.len()
        ));
        for (n, ctxs) in &r.unresolved {
            log(&format!("    {} ({} B frame; {})", an.cg.names[*n], an.cg.frame[*n], ctxs.join(", ")));
            for (a, d) in &an.cg.sites[*n] {
                log(&format!("      {a:#x}  {d:<10} after `{}`", an.loaded_by(*a, d)));
            }
        }
    }
    for (cy, bounded) in &r.cycles {
        let names: Vec<&str> = cy.iter().map(|&n| an.cg.names[n].as_str()).collect();
        let what = if *bounded { "bounded recursion (stack.toml)" } else { "RECURSION" };
        log(&format!("  {what}: {} -> (again)", names.join(" -> ")));
    }
    for n in &r.nesting {
        let ph = n.phase.as_ref().map_or(String::new(), |p| format!(" ({p})"));
        log(&format!(
            "  nested worst case{ph}: {} = {}{} B of {} B usable",
            n.sum,
            ge(n.lower),
            n.total,
            r.usable
        ));
    }
    let margin = r.margin();
    let verdict =
        format!("{}{} B nested worst case of {} B usable, margin {margin} B", ge(r.lower), r.total, r.usable);
    let mut errors = r.errors;
    // The line budgets of `hot.toml` clusters with roots (`xtask hot`).
    let (hot_lines, hot_errors) = crate::hot::check_all(&mut an, &cfg)?;
    for l in &hot_lines {
        log(&format!("  {l}"));
    }
    errors.extend(hot_errors);
    if check {
        let b: toml::Table = toml::from_str(&std::fs::read_to_string(root().join("budgets.toml"))?)?;
        let need = b
            .get("firmware")
            .and_then(|f| f.get("stack_static_margin_bytes"))
            .and_then(toml::Value::as_integer)
            .context("budgets.toml: [firmware] stack_static_margin_bytes")?;
        if margin < need {
            errors.push(format!("margin {margin} B < {need} B (budgets.toml stack_static_margin_bytes)"));
        }
        if !errors.is_empty() {
            bail!("{}", errors.join("; "));
        }
        return Ok(format!("{verdict} (budget {need} B)"));
    }
    for e in &errors {
        log(&format!("  problem: {e}"));
    }
    Ok(if errors.is_empty() {
        verdict
    } else {
        format!("{verdict}; {} problems (--check fails)", errors.len())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `f0..` with `frames`, calls `(from, to, tail)`, one indirect site on
    /// each node in `sites`.
    fn graph(frames: &[u32], calls: &[(usize, usize, bool)], sites: &[usize]) -> CallGraph {
        let n = frames.len();
        let mut cg = CallGraph {
            names: (0..n).map(|i| format!("f{i}")).collect(),
            frame: frames.to_vec(),
            calls: vec![Vec::new(); n],
            sites: vec![Vec::new(); n],
            bounded_cycle: vec![false; n],
        };
        for &(a, b, t) in calls {
            cg.calls[a].push((b, t));
        }
        for &s in sites {
            cg.sites[s].push((0x100 + s as u32, "blx r3".into()));
        }
        cg
    }

    fn chain(w: &mut Walk<'_>, n: usize) -> Vec<(usize, u32)> {
        w.chain(n).iter().map(|&(n, b, _)| (n, b)).collect()
    }

    #[test]
    fn calls_add_frames_tail_calls_replace_them() {
        // f0 (16) calls f1 (100) and tail-calls f2 (40), which calls f3 (8).
        let cg = graph(&[16, 100, 40, 8], &[(0, 1, false), (0, 2, true), (2, 3, false)], &[]);
        let res = BTreeMap::new();
        let mut w = Walk::new(&cg, &res);
        assert_eq!(w.depth(0), Info { depth: 116, next: Some(Step::Call(1)), lower: false });
        assert_eq!(w.depth(2).depth, 48);
        // A tail call deeper than the other paths wins, without the
        // caller's frame.
        let cg = graph(&[16, 4, 400, 8], &[(0, 1, false), (0, 2, true), (2, 3, false)], &[]);
        let mut w = Walk::new(&cg, &res);
        assert_eq!(w.depth(0), Info { depth: 408, next: Some(Step::Tail(2)), lower: false });
        assert_eq!(chain(&mut w, 0), vec![(0, 0), (2, 400), (3, 8)]);
    }

    #[test]
    fn indirect_sites_resolved_bounded_or_lower_bound() {
        // f0 (32) has an indirect site; f1 (200) and f2 (50) are targets.
        let cg = graph(&[32, 200, 50], &[], &[0]);
        let none = BTreeMap::new();
        let mut w = Walk::new(&cg, &none);
        assert_eq!(w.depth(0), Info { depth: 32, next: None, lower: true });
        assert_eq!(w.unresolved, vec![0]);
        let res = BTreeMap::from([(0, vec![Resolve::Targets(vec![1, 2])])]);
        let mut w = Walk::new(&cg, &res);
        assert_eq!(w.depth(0), Info { depth: 232, next: Some(Step::Indirect(1)), lower: false });
        assert!(w.unresolved.is_empty());
        let res = BTreeMap::from([(0, vec![Resolve::Bound(64)])]);
        let mut w = Walk::new(&cg, &res);
        assert_eq!(w.depth(0), Info { depth: 96, next: Some(Step::Bound(64)), lower: false });
        assert_eq!(chain(&mut w, 0), vec![(0, 32), (usize::MAX, 64)]);
        // A lower bound propagates to the callers.
        let cg = graph(&[8, 32], &[(0, 1, false)], &[1]);
        let mut w = Walk::new(&cg, &none);
        assert!(w.depth(0).lower);
    }

    #[test]
    fn recursion_is_reported_and_a_lower_bound_unless_known() {
        // f0 -> f1 -> f2 -> f1; f3 calls itself.
        let mut cg =
            graph(&[8, 16, 24, 4], &[(0, 1, false), (1, 2, false), (2, 1, false), (3, 3, false)], &[]);
        let res = BTreeMap::new();
        let mut w = Walk::new(&cg, &res);
        let i = w.depth(0);
        assert_eq!(i.depth, 8 + 16 + 24);
        assert!(i.lower);
        assert_eq!(w.cycles, vec![(vec![1, 2], false)]);
        assert!(w.depth(3).lower);
        assert_eq!(w.cycles[1], (vec![3], false));
        // Listed as bounded: reported, not a lower bound.
        cg.bounded_cycle[2] = true;
        let mut w = Walk::new(&cg, &res);
        assert!(!w.depth(0).lower);
        assert_eq!(w.cycles, vec![(vec![1, 2], true)]);
    }

    #[test]
    fn nesting_stacks_one_context_per_priority() {
        let rows = vec![
            (4, "thread".to_string(), 10_000),
            (3, "usb".to_string(), 300),
            (2, "rt".to_string(), 6_000),
            (1, "dma".to_string(), 100),
            (1, "timer".to_string(), 200),
            (0, "pager".to_string(), 500),
        ];
        let (total, parts) = nest(&rows, 108, 1);
        assert_eq!(total, 10_000 + (300 + 108) + (6_000 + 108) + (200 + 108) + (500 + 108) + 108);
        let names: Vec<&str> = parts.iter().map(|p| p.1.as_str()).collect();
        assert_eq!(names, ["thread", "usb", "rt", "timer", "pager"]);
    }

    /// An `optional` entry whose callers are not linked is skipped; linked,
    /// its targets must match (BOOK-1/2 wiring: Books' `dyn` calls exist
    /// only with the firmware feature `books`).
    #[test]
    fn optional_indirect_entries_may_be_absent() {
        let an = Analysis {
            cg: graph(&[8, 16], &[], &[0]),
            labels: vec![vec!["f0".into()], vec!["f1".into()]],
            insn: BTreeMap::new(),
            stack: (0, 0),
            secs: Vec::new(),
            kinds: Vec::new(),
        };
        let entry = |callers: &str, optional: bool| IndirectCfg {
            callers: vec![callers.into()],
            targets: vec!["f9".into()],
            bound: None,
            why: String::new(),
            absent: Vec::new(),
            optional,
        };
        let mut missing = Vec::new();
        let (res, _) = an.resolver(&[&entry("g*", true)], &mut missing);
        assert!(missing.is_empty() && res.is_empty(), "{missing:?}");
        an.resolver(&[&entry("g*", false)], &mut missing);
        assert_eq!(missing.len(), 2, "{missing:?}");
        missing.clear();
        an.resolver(&[&entry("f0", true)], &mut missing);
        assert_eq!(missing, ["target `f9` matches nothing"]);
    }

    #[test]
    fn stack_toml_parses() {
        let c = Config::load(&root().join("stack.toml")).unwrap();
        assert!(c.exception_frame_bytes >= 104);
        assert!(c.context.iter().any(|c| c.priority == 4));
        assert!(c.context.iter().all(|c| c.priority <= 4));
    }
}
