//! Host side of the profilers (WP-T-8, 07 §5.6): symbolise a capture read
//! from the player's profiler block (`orpheus_proto::prof`) against the
//! firmware's `layout.json` and ELF, aggregate it, write the reports, and
//! turn a fault profile into a `hot.toml` cluster. Everything here is pure
//! (no player); `dev::prof` reads the block and calls it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::{Context, Result, bail};
use orpheus_pager::LINE_SIZE;
use orpheus_proto::prof::Capture;

use crate::linkgraph::{crate_of, demangle};
use crate::pagelink::Layout;
use crate::rules::{self, Cluster};

/// Where an address lies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Placement {
    /// SRAM0: the resident image.
    Resident,
    /// The code window (paged code).
    Paged,
    /// The rodata window (paged rodata).
    PagedRo,
    Other,
}

impl Placement {
    #[must_use]
    pub fn of(addr: u32) -> Self {
        match addr {
            0x0010_0000..0x0013_8000 => Self::Resident,
            orpheus_pager::CODE_WIN..orpheus_pager::CODE_WIN_END => Self::Paged,
            orpheus_pager::RO_WIN..orpheus_pager::RO_WIN_END => Self::PagedRo,
            _ => Self::Other,
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Resident => "resident",
            Self::Paged => "paged",
            Self::PagedRo => "paged-ro",
            Self::Other => "other",
        }
    }

    #[must_use]
    pub fn paged(self) -> bool {
        matches!(self, Self::Paged | Self::PagedRo)
    }
}

/// A code or rodata section of the image (from `layout.json`).
#[derive(Clone, Debug)]
pub struct Sec {
    pub name: String,
    /// The demangled first symbol (the name `hot.toml` patterns match).
    pub label: String,
    pub krate: String,
    /// `code` or `rodata`.
    pub kind: String,
    pub placement: Placement,
    pub addr: u32,
    pub size: u32,
    /// The `hot.toml` cluster that owns it (first match in file order).
    pub owner: Option<String>,
}

/// Address → function and section.
#[derive(Clone, Debug, Default)]
pub struct SymTab {
    /// `(start, size, demangled name)`, sorted by start.
    funcs: Vec<(u32, u32, String)>,
    /// Sorted by address.
    pub secs: Vec<Sec>,
    /// Code bytes of each `hot.toml` cluster as packed (`layout.json`).
    pub cluster_bytes: BTreeMap<String, u32>,
}

impl SymTab {
    /// From the image's layout and, when available, its ELF (function
    /// symbols give names inside sections that hold more than one function,
    /// such as the resident image's merged ones).
    pub fn new(layout: &Layout, elf: Option<&[u8]>, clusters: &[Cluster]) -> Result<Self> {
        let secs = layout
            .sections
            .iter()
            .filter(|s| s.kind == "code" || s.kind == "rodata")
            .map(|s| Sec {
                name: s.section.clone(),
                label: s.symbol.clone(),
                krate: s.krate.clone(),
                kind: s.kind.clone(),
                placement: Placement::of(s.addr),
                addr: s.addr,
                size: s.size,
                owner: None,
            })
            .collect();
        let funcs = match elf {
            Some(e) => elf_functions(e)?,
            None => Vec::new(),
        };
        let mut t = Self::from_parts(secs, funcs);
        t.cluster_bytes = layout.clusters.iter().map(|c| (c.name.clone(), c.bytes)).collect();
        t.assign_owners(clusters);
        Ok(t)
    }

    #[must_use]
    pub fn from_parts(mut secs: Vec<Sec>, mut funcs: Vec<(u32, u32, String)>) -> Self {
        secs.sort_by_key(|s| s.addr);
        funcs.sort_by_key(|f| f.0);
        Self { funcs, secs, cluster_bytes: BTreeMap::new() }
    }

    /// Mark each paged section with the first cluster whose patterns select
    /// it (pagelink's rule: a section goes to the first cluster in file
    /// order that matches it).
    pub fn assign_owners(&mut self, clusters: &[Cluster]) {
        for s in &mut self.secs {
            s.owner = clusters.iter().find(|c| selects(c, &s.name, &s.label)).map(|c| c.name.clone());
        }
    }

    #[must_use]
    pub fn section_at(&self, addr: u32) -> Option<usize> {
        let i = self.secs.partition_point(|s| s.addr <= addr);
        let i = i.checked_sub(1)?;
        let s = &self.secs[i];
        (addr < s.addr + s.size.max(1)).then_some(i)
    }

    #[must_use]
    pub fn function_at(&self, addr: u32) -> Option<&str> {
        let i = self.funcs.partition_point(|f| f.0 <= addr).checked_sub(1)?;
        let f = &self.funcs[i];
        (addr < f.0 + f.1.max(2)).then_some(f.2.as_str())
    }

    /// Function name (ELF symbol, else the section's label, else the
    /// address) and crate.
    #[must_use]
    pub fn name(&self, addr: u32) -> (String, String) {
        if let Some(f) = self.function_at(addr) {
            let k = crate_of(f).map_or_else(
                || self.section_at(addr).map_or_else(|| "(c)".to_string(), |i| self.secs[i].krate.clone()),
                str::to_string,
            );
            return (f.to_string(), k);
        }
        match self.section_at(addr) {
            Some(i) => (self.secs[i].label.clone(), self.secs[i].krate.clone()),
            None => (format!("?{addr:#08x}"), "?".into()),
        }
    }
}

/// Whether a cluster's patterns select a section (as `Cluster::rank`, on
/// the section name and its label).
#[must_use]
pub fn selects(c: &Cluster, section: &str, label: &str) -> bool {
    c.sections.iter().any(|p| rules::glob(p, section))
        || c.symbols.iter().any(|p| rules::symbol_matches(p, label))
}

/// Function symbols of an ELF: `(start, size, demangled)`, Thumb bit
/// cleared.
pub fn elf_functions(elf: &[u8]) -> Result<Vec<(u32, u32, String)>> {
    use object::{Object, ObjectSymbol, SymbolKind};
    let file = object::File::parse(elf).context("parse ELF")?;
    let mut v: Vec<(u32, u32, String)> = file
        .symbols()
        .filter(|s| s.kind() == SymbolKind::Text && s.size() > 0)
        .filter_map(|s| Some(((s.address() as u32) & !1, s.size() as u32, demangle(s.name().ok()?))))
        .collect();
    v.sort_by_key(|f| f.0);
    v.dedup_by_key(|f| f.0);
    Ok(v)
}

// ------------------------------------------------------------- PC profile

/// One function of a PC profile.
#[derive(Clone, Debug, Default)]
pub struct PcRow {
    pub name: String,
    pub krate: String,
    pub placement: Option<Placement>,
    pub thread: u64,
    pub handler: u64,
}

impl PcRow {
    #[must_use]
    pub fn total(&self) -> u64 {
        self.thread + self.handler
    }
}

/// One crate of a PC profile: samples split resident/paged × thread/handler.
#[derive(Clone, Debug, Default)]
pub struct CrateRow {
    pub krate: String,
    pub resident_thread: u64,
    pub resident_handler: u64,
    pub paged_thread: u64,
    pub paged_handler: u64,
}

impl CrateRow {
    #[must_use]
    pub fn total(&self) -> u64 {
        self.resident_thread + self.resident_handler + self.paged_thread + self.paged_handler
    }
}

#[derive(Clone, Debug, Default)]
pub struct PcProfile {
    pub samples: u64,
    pub stride: u32,
    pub period_us: u32,
    pub span_us: u32,
    pub seen: u32,
    pub functions: Vec<PcRow>,
    pub crates: Vec<CrateRow>,
    /// `[resident, paged, other] × [thread, handler]`.
    pub split: [[u64; 2]; 3],
}

#[must_use]
pub fn pc_profile(c: &Capture<'_>, t: &SymTab) -> PcProfile {
    let mut f: BTreeMap<String, PcRow> = BTreeMap::new();
    let mut k: BTreeMap<String, CrateRow> = BTreeMap::new();
    let mut p = PcProfile {
        stride: c.stride(),
        period_us: c.period_us,
        span_us: c.span_us(),
        seen: c.seen,
        ..PcProfile::default()
    };
    for s in c.pc_samples() {
        p.samples += 1;
        let place = Placement::of(s.pc);
        let (name, krate) = t.name(s.pc);
        let row = f.entry(name.clone()).or_insert_with(|| PcRow {
            name,
            krate: krate.clone(),
            placement: Some(place),
            ..PcRow::default()
        });
        let h = usize::from(s.handler);
        if s.handler {
            row.handler += 1;
        } else {
            row.thread += 1;
        }
        let cr = k.entry(krate.clone()).or_insert_with(|| CrateRow { krate, ..CrateRow::default() });
        let slot = match (place.paged(), s.handler) {
            (false, false) => &mut cr.resident_thread,
            (false, true) => &mut cr.resident_handler,
            (true, false) => &mut cr.paged_thread,
            (true, true) => &mut cr.paged_handler,
        };
        *slot += 1;
        let pi = match place {
            Placement::Resident => 0,
            Placement::Paged | Placement::PagedRo => 1,
            Placement::Other => 2,
        };
        p.split[pi][h] += 1;
    }
    p.functions = f.into_values().collect();
    p.functions.sort_by(|a, b| b.total().cmp(&a.total()).then_with(|| a.name.cmp(&b.name)));
    p.crates = k.into_values().collect();
    p.crates.sort_by(|a, b| b.total().cmp(&a.total()).then_with(|| a.krate.cmp(&b.krate)));
    p
}

fn pct(n: u64, d: u64) -> f64 {
    if d == 0 { 0.0 } else { 100.0 * n as f64 / d as f64 }
}

impl PcProfile {
    #[must_use]
    pub fn text(&self, top: usize) -> String {
        let mut o = String::new();
        let n = self.samples;
        let _ = writeln!(
            o,
            "PC profile: {n} samples ({} ticks of {} us, one sample per {} ticks), over {:.1} s",
            self.seen,
            self.period_us,
            self.stride,
            f64::from(self.span_us) / 1e6
        );
        let [r, pg, ot] = self.split;
        let _ = writeln!(
            o,
            "  resident {:.1} % (thread {:.1} %, handler {:.1} %), paged {:.1} % (thread {:.1} %, handler {:.1} %){}",
            pct(r[0] + r[1], n),
            pct(r[0], n),
            pct(r[1], n),
            pct(pg[0] + pg[1], n),
            pct(pg[0], n),
            pct(pg[1], n),
            if ot[0] + ot[1] > 0 { format!(", other {:.1} %", pct(ot[0] + ot[1], n)) } else { String::new() }
        );
        let _ = writeln!(
            o,
            "\nBy function (top {top}):\n  {:>6} {:>6} {:>6} {:<9} name",
            "%", "thread", "handlr", "where"
        );
        for f in self.functions.iter().take(top) {
            let _ = writeln!(
                o,
                "  {:>6.1} {:>6} {:>6} {:<9} {}",
                pct(f.total(), n),
                f.thread,
                f.handler,
                f.placement.map_or("?", Placement::name),
                f.name
            );
        }
        let _ = writeln!(
            o,
            "\nBy crate:\n  {:>6} {:>8} {:>8} {:>8} {:>8} crate",
            "%", "res/thr", "res/hdl", "pg/thr", "pg/hdl"
        );
        for c in &self.crates {
            let _ = writeln!(
                o,
                "  {:>6.1} {:>8} {:>8} {:>8} {:>8} {}",
                pct(c.total(), n),
                c.resident_thread,
                c.resident_handler,
                c.paged_thread,
                c.paged_handler,
                c.krate
            );
        }
        o
    }

    #[must_use]
    pub fn csv_functions(&self) -> String {
        let mut o = String::from("samples,percent,thread,handler,placement,crate,function\n");
        for f in &self.functions {
            let _ = writeln!(
                o,
                "{},{:.2},{},{},{},{},{}",
                f.total(),
                pct(f.total(), self.samples),
                f.thread,
                f.handler,
                f.placement.map_or("?", Placement::name),
                csv(&f.krate),
                csv(&f.name)
            );
        }
        o
    }

    #[must_use]
    pub fn csv_crates(&self) -> String {
        let mut o = String::from(
            "samples,percent,resident_thread,resident_handler,paged_thread,paged_handler,crate\n",
        );
        for c in &self.crates {
            let _ = writeln!(
                o,
                "{},{:.2},{},{},{},{},{}",
                c.total(),
                pct(c.total(), self.samples),
                c.resident_thread,
                c.resident_handler,
                c.paged_thread,
                c.paged_handler,
                csv(&c.krate)
            );
        }
        o
    }
}

fn csv(s: &str) -> String {
    if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

// ---------------------------------------------------------- fault profile

/// A fault with its time unwrapped (µs since the start of the capture).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultEv {
    pub t_us: u64,
    pub addr: u32,
    pub pc: u32,
    pub loaded: bool,
    pub data: bool,
}

/// The capture's faults, the 18-bit times unwrapped on the assumption that
/// entries are in time order (they are: the ring is never reordered) and
/// that no two consecutive entries are more than 268 s apart.
#[must_use]
pub fn unwrap_faults(c: &Capture<'_>) -> Vec<FaultEv> {
    let period = 1u64 << orpheus_proto::prof::TIME_BITS;
    let mut base = 0u64;
    let mut last = 0u64;
    c.faults()
        .map(|f| {
            let mut t = base + u64::from(f.t);
            if t < last {
                base += period;
                t += period;
            }
            last = t;
            FaultEv {
                t_us: t * u64::from(orpheus_proto::prof::TIME_US),
                addr: f.addr,
                pc: f.pc,
                loaded: f.loaded,
                data: f.data,
            }
        })
        .collect()
}

/// What a fault was for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FaultKind {
    /// An instruction fetch from a code page.
    Code,
    /// A data read from a code page (a literal pool, a jump table).
    Literal,
    /// A read of a rodata page.
    Rodata,
    Other,
}

impl FaultKind {
    #[must_use]
    pub fn of(f: &FaultEv) -> Self {
        match Placement::of(f.addr) {
            Placement::Paged if f.data => Self::Literal,
            Placement::Paged => Self::Code,
            Placement::PagedRo => Self::Rodata,
            _ => Self::Other,
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Literal => "literal",
            Self::Rodata => "rodata",
            Self::Other => "other",
        }
    }
}

/// Faults on one section (the faulting address's).
#[derive(Clone, Debug, Default)]
pub struct SecRow {
    /// Index into `SymTab::secs`, if the address lies in a section.
    pub sec: Option<usize>,
    pub label: String,
    pub events: u64,
    pub loads: u64,
    /// VA lines (address / 1 KiB) faulted on.
    pub lines: BTreeSet<u32>,
    pub kinds: BTreeMap<FaultKind, u64>,
}

/// Faults by the function holding the faulting PC.
#[derive(Clone, Debug, Default)]
pub struct FnRow {
    pub name: String,
    pub krate: String,
    pub placement: Option<Placement>,
    pub events: u64,
    pub loads: u64,
    pub rodata: u64,
}

/// One time window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Window {
    pub start_ms: u64,
    pub events: u64,
    pub loads: u64,
    pub code_lines: usize,
    pub ro_lines: usize,
}

/// How much of the measured faults one cluster's sections take.
#[derive(Clone, Debug, Default)]
pub struct Coverage {
    pub cluster: String,
    pub events: u64,
    pub loads: u64,
    pub lines: usize,
}

/// Counters the pager keeps (STATS words), over the capture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Totals {
    pub loads: u64,
    pub code_faults: u64,
    pub ro_faults: u64,
    pub evictions: u64,
    pub ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct FaultProfile {
    pub stride: u32,
    pub seen: u32,
    pub span_us: u32,
    pub burst: bool,
    pub full: bool,
    pub events: u64,
    pub loads: u64,
    pub kinds: BTreeMap<FaultKind, u64>,
    pub sections: Vec<SecRow>,
    pub functions: Vec<FnRow>,
    pub windows: Vec<Window>,
    pub window_ms: u64,
    pub code_lines: usize,
    pub ro_lines: usize,
    pub coverage: Vec<Coverage>,
    pub totals: Option<Totals>,
}

#[must_use]
pub fn fault_profile(c: &Capture<'_>, t: &SymTab, window_ms: u64, totals: Option<Totals>) -> FaultProfile {
    let evs = unwrap_faults(c);
    let mut p = FaultProfile {
        stride: c.stride(),
        seen: c.seen,
        span_us: c.span_us(),
        burst: c.burst,
        full: c.full,
        window_ms: window_ms.max(1),
        totals,
        ..FaultProfile::default()
    };
    let mut secs: BTreeMap<Option<usize>, SecRow> = BTreeMap::new();
    let mut fns: BTreeMap<String, FnRow> = BTreeMap::new();
    let mut code = BTreeSet::new();
    let mut ro = BTreeSet::new();
    let mut win: BTreeMap<u64, (Window, BTreeSet<u32>, BTreeSet<u32>)> = BTreeMap::new();
    let mut cov: BTreeMap<String, (u64, u64, BTreeSet<u32>)> = BTreeMap::new();
    for e in &evs {
        p.events += 1;
        p.loads += u64::from(e.loaded);
        let kind = FaultKind::of(e);
        *p.kinds.entry(kind).or_default() += 1;
        let line = e.addr / LINE_SIZE;
        let sec = t.section_at(e.addr);
        let row = secs.entry(sec).or_insert_with(|| SecRow {
            sec,
            label: sec
                .map_or_else(|| format!("?{:#08x}", e.addr & !(LINE_SIZE - 1)), |i| t.secs[i].label.clone()),
            ..SecRow::default()
        });
        row.events += 1;
        row.loads += u64::from(e.loaded);
        row.lines.insert(line);
        *row.kinds.entry(kind).or_default() += 1;
        if let Some(owner) = sec.and_then(|i| t.secs[i].owner.clone()) {
            let c = cov.entry(owner).or_default();
            c.0 += 1;
            c.1 += u64::from(e.loaded);
            c.2.insert(line);
        }
        let (name, krate) = t.name(e.pc);
        let f = fns.entry(name.clone()).or_insert_with(|| FnRow {
            name,
            krate,
            placement: Some(Placement::of(e.pc)),
            ..FnRow::default()
        });
        f.events += 1;
        f.loads += u64::from(e.loaded);
        f.rodata += u64::from(kind == FaultKind::Rodata);
        let ro_line = kind == FaultKind::Rodata;
        if ro_line {
            ro.insert(line)
        } else {
            code.insert(line)
        };
        let w0 = e.t_us / 1000 / p.window_ms * p.window_ms;
        let w = win.entry(w0).or_insert_with(|| {
            (Window { start_ms: w0, ..Window::default() }, BTreeSet::new(), BTreeSet::new())
        });
        w.0.events += 1;
        w.0.loads += u64::from(e.loaded);
        if ro_line {
            w.2.insert(line)
        } else {
            w.1.insert(line)
        };
    }
    p.code_lines = code.len();
    p.ro_lines = ro.len();
    p.sections = secs.into_values().collect();
    p.sections.sort_by(|a, b| {
        b.loads.cmp(&a.loads).then(b.events.cmp(&a.events)).then_with(|| a.label.cmp(&b.label))
    });
    p.functions = fns.into_values().collect();
    p.functions.sort_by(|a, b| b.events.cmp(&a.events).then_with(|| a.name.cmp(&b.name)));
    p.windows = win
        .into_values()
        .map(|(mut w, c, r)| {
            w.code_lines = c.len();
            w.ro_lines = r.len();
            w
        })
        .collect();
    p.coverage = cov
        .into_iter()
        .map(|(cluster, (events, loads, lines))| Coverage { cluster, events, loads, lines: lines.len() })
        .collect();
    p.coverage.sort_by(|a, b| b.events.cmp(&a.events).then_with(|| a.cluster.cmp(&b.cluster)));
    p
}

impl FaultProfile {
    #[must_use]
    pub fn text(&self, t: &SymTab, top: usize) -> String {
        let mut o = String::new();
        let s = u64::from(self.stride);
        let _ = writeln!(
            o,
            "Fault profile: {} faults logged ({} loads), one per {} faults ({} seen{}), over {:.1} s",
            self.events,
            self.loads,
            self.stride,
            self.seen,
            match (self.burst, self.full) {
                (true, true) => "; burst: the ring filled, later faults not logged",
                (true, false) => "; burst",
                _ => "",
            },
            f64::from(self.span_us) / 1e6
        );
        if let Some(tt) = self.totals {
            let secs = tt.ms as f64 / 1000.0;
            let _ = writeln!(
                o,
                "  pager counters over the capture: {} line loads ({:.0}/s), {} code faults, {} rodata faults, {} evictions in {:.1} s",
                tt.loads,
                if secs > 0.0 { tt.loads as f64 / secs } else { 0.0 },
                tt.code_faults,
                tt.ro_faults,
                tt.evictions,
                secs
            );
        }
        let kinds: Vec<String> =
            self.kinds.iter().map(|(k, n)| format!("{} {:.1} %", k.name(), pct(*n, self.events))).collect();
        let _ = writeln!(o, "  by kind: {}", kinds.join(", "));
        let _ = writeln!(
            o,
            "  distinct lines faulted on: {} code, {} rodata (lower bound when thinned)",
            self.code_lines, self.ro_lines
        );
        let _ = writeln!(
            o,
            "\nBy section (faulting address; est. = logged x {s}; top {top}):\n  {:>6} {:>7} {:>6} {:>5} {:<8} {:<14} section",
            "events", "est.", "loads", "lines", "kind", "cluster"
        );
        for r in self.sections.iter().take(top) {
            let kind = r.kinds.iter().max_by_key(|(_, n)| **n).map_or("?", |(k, _)| k.name());
            let owner = r.sec.and_then(|i| t.secs[i].owner.clone()).unwrap_or_else(|| "-".into());
            let _ = writeln!(
                o,
                "  {:>6} {:>7} {:>6} {:>5} {:<8} {:<14} {}",
                r.events,
                r.events * s,
                r.loads,
                r.lines.len(),
                kind,
                owner,
                r.label
            );
        }
        let _ = writeln!(
            o,
            "\nBy faulting function (PC; top {top}):\n  {:>6} {:>6} {:>6} {:<9} function",
            "events", "loads", "rodata", "where"
        );
        for f in self.functions.iter().take(top) {
            let _ = writeln!(
                o,
                "  {:>6} {:>6} {:>6} {:<9} {}",
                f.events,
                f.loads,
                f.rodata,
                f.placement.map_or("?", Placement::name),
                f.name
            );
        }
        let _ = writeln!(o, "\nClusters (share of the logged faults on their sections):");
        if self.coverage.is_empty() {
            let _ = writeln!(o, "  none of the faults is on a clustered section");
        }
        for c in &self.coverage {
            let _ = writeln!(
                o,
                "  {:<14} {:>5.1} % of faults, {:>5.1} % of loads, {} lines faulted on{}",
                c.cluster,
                pct(c.events, self.events),
                pct(c.loads, self.loads),
                c.lines,
                t.cluster_bytes.get(&c.cluster).map_or(String::new(), |b| {
                    let l = b.div_ceil(LINE_SIZE);
                    format!(" (cluster: {l} lines{})", if l > 24 { ", over the 24-line budget" } else { "" })
                })
            );
        }
        let _ = writeln!(
            o,
            "\nWorking set per {} ms window (distinct lines among the logged faults):\n  {:>8} {:>6} {:>6} {:>5} {:>5}",
            self.window_ms, "t ms", "events", "loads", "code", "ro"
        );
        for w in &self.windows {
            let _ = writeln!(
                o,
                "  {:>8} {:>6} {:>6} {:>5} {:>5}",
                w.start_ms, w.events, w.loads, w.code_lines, w.ro_lines
            );
        }
        o
    }

    #[must_use]
    pub fn csv_sections(&self, t: &SymTab) -> String {
        let mut o = String::from(
            "events,estimated,loads,lines,code,literal,rodata,placement,cluster,crate,section,label\n",
        );
        for r in &self.sections {
            let k = |x| r.kinds.get(&x).copied().unwrap_or(0);
            let (place, owner, krate, name) = match r.sec {
                Some(i) => {
                    let s = &t.secs[i];
                    (s.placement.name(), s.owner.clone().unwrap_or_default(), s.krate.clone(), s.name.clone())
                }
                None => ("?", String::new(), String::new(), String::new()),
            };
            let _ = writeln!(
                o,
                "{},{},{},{},{},{},{},{},{},{},{},{}",
                r.events,
                r.events * u64::from(self.stride),
                r.loads,
                r.lines.len(),
                k(FaultKind::Code),
                k(FaultKind::Literal),
                k(FaultKind::Rodata),
                place,
                csv(&owner),
                csv(&krate),
                csv(&name),
                csv(&r.label)
            );
        }
        o
    }

    #[must_use]
    pub fn csv_functions(&self) -> String {
        let mut o = String::from("events,loads,rodata,placement,crate,function\n");
        for f in &self.functions {
            let _ = writeln!(
                o,
                "{},{},{},{},{},{}",
                f.events,
                f.loads,
                f.rodata,
                f.placement.map_or("?", Placement::name),
                csv(&f.krate),
                csv(&f.name)
            );
        }
        o
    }

    #[must_use]
    pub fn csv_windows(&self) -> String {
        let mut o = String::from("start_ms,events,loads,code_lines,ro_lines\n");
        for w in &self.windows {
            let _ = writeln!(o, "{},{},{},{},{}", w.start_ms, w.events, w.loads, w.code_lines, w.ro_lines);
        }
        o
    }
}

// ------------------------------------------------------ cluster generator

/// Line budgets of a generated cluster (02 §3.5: a cluster of at most 24
/// code lines leaves 8 of the 32 physical lines for everything else; the
/// rodata cache has 16 lines).
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub code_lines: u32,
    pub ro_lines: u32,
}

impl Default for Budget {
    fn default() -> Self {
        Self { code_lines: 24, ro_lines: 8 }
    }
}

/// One pattern of a generated cluster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    pub pattern: String,
    /// `sections = […]` pattern (no symbol name), else `symbols = […]`.
    pub by_section: bool,
    pub sections: usize,
    pub bytes: u32,
    pub events: u64,
    pub loads: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Generated {
    pub name: String,
    pub code: Vec<Pick>,
    pub ro: Vec<Pick>,
    pub code_bytes: u32,
    pub ro_bytes: u32,
    /// Hot sections left out, with the reason.
    pub skipped: Vec<(String, String)>,
    /// Logged faults/loads on paged sections, and on the picked ones.
    pub paged_events: u64,
    pub paged_loads: u64,
    pub picked_events: u64,
    pub picked_loads: u64,
}

/// Bytes a section takes in a cluster: its size plus up to 3 bytes of
/// alignment padding (pagelink aligns each section; code is 2- or 4-byte
/// aligned).
fn packed(size: u32) -> u32 {
    size + 3
}

/// A `hot.toml` glob that matches `name` literally (`*` and `?` become `?`,
/// which matches the character itself among others).
#[must_use]
pub fn literal_pattern(name: &str) -> String {
    name.replace(['*', '?'], "?")
}

/// Pick the hottest paged sections of a fault profile for cluster `name`:
/// by line loads (then faults), greedily within the budgets, skipping
/// sections another cluster owns (a section belongs to the first cluster
/// that matches it, so taking it here would move it out of that cluster or
/// do nothing). Patterns are the sections' labels; a label shared by
/// several paged sections takes them all (all count against the budget),
/// and a label shared with a section of another cluster is skipped. Order:
/// code, hottest first, then rodata, hottest first.
#[must_use]
pub fn generate(name: &str, p: &FaultProfile, t: &SymTab, b: Budget) -> Generated {
    let mut g = Generated { name: name.to_string(), ..Generated::default() };
    let mut by_label: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, s) in t.secs.iter().enumerate() {
        by_label.entry(s.label.as_str()).or_default().push(i);
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for r in &p.sections {
        let Some(i) = r.sec else { continue };
        let s = &t.secs[i];
        if !s.placement.paged() {
            continue;
        }
        g.paged_events += r.events;
        g.paged_loads += r.loads;
        if let Some(o) = &s.owner
            && o != name
        {
            g.skipped.push((s.label.clone(), format!("in cluster {o}")));
            continue;
        }
        let by_section = s.label == s.name || s.label.starts_with('.');
        let key = if by_section { s.name.clone() } else { s.label.clone() };
        if !seen.insert(key.clone()) {
            // Another section with this label was considered already.
            continue;
        }
        let same: Vec<usize> = if by_section { vec![i] } else { by_label[s.label.as_str()].clone() };
        if let Some(o) = same.iter().find_map(|&j| t.secs[j].owner.as_ref().filter(|o| o.as_str() != name)) {
            g.skipped.push((s.label.clone(), format!("its name also selects a section of cluster {o}")));
            continue;
        }
        let paged: Vec<usize> = same.into_iter().filter(|&j| t.secs[j].placement.paged()).collect();
        let bytes: u32 = paged.iter().map(|&j| packed(t.secs[j].size)).sum();
        let (events, loads): (u64, u64) = p
            .sections
            .iter()
            .filter(|x| x.sec.is_some_and(|j| paged.contains(&j)))
            .fold((0, 0), |a, x| (a.0 + x.events, a.1 + x.loads));
        let ro = s.kind == "rodata";
        let (used, cap) =
            if ro { (g.ro_bytes, b.ro_lines * LINE_SIZE) } else { (g.code_bytes, b.code_lines * LINE_SIZE) };
        if used + bytes > cap {
            g.skipped.push((s.label.clone(), format!("{bytes} B do not fit ({used} of {cap} B used)")));
            continue;
        }
        let pick =
            Pick { pattern: literal_pattern(&key), by_section, sections: paged.len(), bytes, events, loads };
        g.picked_events += events;
        g.picked_loads += loads;
        if ro {
            g.ro_bytes += bytes;
            g.ro.push(pick);
        } else {
            g.code_bytes += bytes;
            g.code.push(pick);
        }
    }
    // Output order: hottest first (a shared label's pick sums its
    // sections), stable.
    for v in [&mut g.code, &mut g.ro] {
        v.sort_by(|a, b| b.loads.cmp(&a.loads).then(b.events.cmp(&a.events)));
    }
    g
}

impl Generated {
    /// The `[[cluster]]` block (TOML, with its explanation as comments).
    #[must_use]
    pub fn block(&self, note: &str) -> String {
        let mut o = String::from("[[cluster]]\n");
        let _ = writeln!(
            o,
            "# Generated by `cargo xtask dev prof --update-hot {}` ({note}): {} code sections, {} B ({} lines), {} rodata sections, {} B; they took {:.0} % of the logged loads on paged sections.",
            self.name,
            self.code.iter().map(|p| p.sections).sum::<usize>(),
            self.code_bytes,
            self.code_bytes.div_ceil(LINE_SIZE),
            self.ro.iter().map(|p| p.sections).sum::<usize>(),
            self.ro_bytes,
            pct(self.picked_loads, self.paged_loads)
        );
        let _ = writeln!(o, "name = \"{}\"", self.name);
        o.push_str("bins = [\"firmware\"]\n");
        let list = |o: &mut String, key: &str, v: Vec<&Pick>| {
            if v.is_empty() {
                return;
            }
            let _ = writeln!(o, "{key} = [");
            for p in v {
                let _ = writeln!(o, "    {}, # {} loads, {} B", toml_str(&p.pattern), p.loads, p.bytes);
            }
            o.push_str("]\n");
        };
        let all = || self.code.iter().chain(&self.ro);
        list(&mut o, "sections", all().filter(|p| p.by_section).collect());
        list(&mut o, "symbols", all().filter(|p| !p.by_section).collect());
        o
    }

    #[must_use]
    pub fn text(&self) -> String {
        let mut o = format!(
            "Cluster {}: {} code patterns, {} B ({} lines); {} rodata patterns, {} B ({} lines); {:.1} % of the logged faults and {:.1} % of the loads on paged sections\n",
            self.name,
            self.code.len(),
            self.code_bytes,
            self.code_bytes.div_ceil(LINE_SIZE),
            self.ro.len(),
            self.ro_bytes,
            self.ro_bytes.div_ceil(LINE_SIZE),
            pct(self.picked_events, self.paged_events),
            pct(self.picked_loads, self.paged_loads)
        );
        for p in self.code.iter().chain(&self.ro) {
            let _ = writeln!(o, "  + {:>5} loads {:>6} B  {}", p.loads, p.bytes, p.pattern);
        }
        for (l, why) in self.skipped.iter().take(20) {
            let _ = writeln!(o, "  - {l}: {why}");
        }
        o
    }
}

fn toml_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if c.is_control() => {
                let _ = write!(o, "\\u{:04X}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Replace the `[[cluster]]` named `name` in `hot.toml`'s text with `block`,
/// or append `block` when there is none. The comments before the next
/// cluster stay with it. The result is checked to parse and to hold the
/// cluster once.
pub fn update_hot(text: &str, name: &str, block: &str) -> Result<String> {
    let lines: Vec<&str> = text.lines().collect();
    let starts: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].trim() == "[[cluster]]").collect();
    let want = format!("name = {}", toml_str(name));
    let mut found = None;
    for (k, &s) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(lines.len());
        let named = lines[s + 1..end].iter().find(|l| l.trim_start().starts_with("name"));
        if named.is_some_and(|l| l.split_whitespace().collect::<Vec<_>>().join(" ") == want) {
            // The block ends at its last line that is neither blank nor a
            // comment (those introduce the next cluster).
            let mut e = end;
            while e > s + 1 && {
                let t = lines[e - 1].trim();
                t.is_empty() || t.starts_with('#')
            } {
                e -= 1;
            }
            found = Some((s, e));
            break;
        }
    }
    let mut out = String::new();
    match found {
        Some((s, e)) => {
            for l in &lines[..s] {
                out.push_str(l);
                out.push('\n');
            }
            out.push_str(block);
            for l in &lines[e..] {
                out.push_str(l);
                out.push('\n');
            }
        }
        None => {
            out.push_str(text.trim_end_matches('\n'));
            out.push_str("\n\n");
            out.push_str(block);
        }
    }
    let clusters = rules::parse_hot(&out).context("the updated hot.toml does not parse")?;
    let n = clusters.iter().filter(|c| c.name == name).count();
    if n != 1 {
        bail!("the updated hot.toml has {n} clusters named {name}");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orpheus_proto::prof as p;

    fn sec(name: &str, label: &str, kind: &str, addr: u32, size: u32) -> Sec {
        Sec {
            name: name.into(),
            label: label.into(),
            krate: crate_of(label).unwrap_or("?").into(),
            kind: kind.into(),
            placement: Placement::of(addr),
            addr,
            size,
            owner: None,
        }
    }

    /// A small image: resident code, paged code over three lines, paged
    /// rodata, one section already in a cluster, two monomorphs sharing a
    /// label.
    fn tab() -> SymTab {
        let secs = vec![
            sec(".text.main", "firmware::main", "code", 0x0010_1000, 0x200),
            sec(".text.isr", "atj2157_hal::rt::irq_dispatch", "code", 0x0010_1200, 0x80),
            sec(".text.a", "firmware::ui::draw", "code", 0x0020_0000, 0x300),
            sec(".text.b", "orpheus_fs::read", "code", 0x0020_0300, 0x200),
            sec(".text.c", "firmware::ui::row", "code", 0x0020_0500, 0x500),
            sec(".text.d1", "<orpheus_img::Png>::step", "code", 0x0020_0A00, 0x100),
            sec(".text.d2", "<orpheus_img::Png>::step", "code", 0x0020_0B00, 0x100),
            sec(".text.e", "orpheus_codec_opus::celt::decode", "code", 0x0020_0C00, 0x400),
            sec(".text.big", "firmware::big", "code", 0x0020_1000, 30 * 1024),
            sec(".rodata.t", "orpheus_img::TABLE", "rodata", 0x0034_0000, 0x800),
            sec(".rodata..Lanon.1", ".rodata..Lanon.1", "rodata", 0x0034_0800, 0x40),
        ];
        let mut t = SymTab::from_parts(
            secs,
            vec![
                (0x0010_1000, 0x100, "firmware::main".into()),
                (0x0010_1100, 0x100, "firmware::idle".into()),
            ],
        );
        let c = Cluster {
            name: "opus-celt".into(),
            bins: vec![],
            symbols: vec!["orpheus_codec_opus::celt::*".into()],
            sections: vec![],
        };
        t.assign_owners(&[c]);
        t
    }

    fn block_of(kind: u32, entries: &[&[u32]], shift: u32, seen: u32) -> Vec<u8> {
        let mut b = [0u32; p::WORDS];
        p::start(&mut b, kind, false, 1000, 100);
        for e in entries {
            p::push(&mut b, e);
        }
        b[p::W_SHIFT] = shift;
        b[p::W_SEEN] = seen;
        p::stop(&mut b, 100 + 2_000_000);
        b.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    #[test]
    fn symbolises_functions_sections_and_unknowns() {
        let t = tab();
        assert_eq!(t.name(0x0010_1104).0, "firmware::idle");
        // No ELF symbol: the section's label.
        assert_eq!(t.name(0x0020_0302), ("orpheus_fs::read".to_string(), "orpheus_fs".to_string()));
        assert_eq!(t.section_at(0x0020_04FF).map(|i| t.secs[i].name.as_str()), Some(".text.b"));
        assert!(t.name(0x0030_0000).0.starts_with('?'));
        assert_eq!(t.section_at(0x0010_0000), None);
        assert_eq!(t.secs[t.section_at(0x0020_0C10).unwrap()].owner.as_deref(), Some("opus-celt"));
    }

    #[test]
    fn pc_profile_splits_mode_and_placement() {
        let t = tab();
        let e = |pc: u32, h: bool| p::encode_pc(pc, h);
        let ents: Vec<[u32; 1]> = vec![
            [e(0x0010_1104, false)],
            [e(0x0010_1104, false)],
            [e(0x0010_1210, true)],
            [e(0x0020_0010, false)],
            [e(0x0020_0012, true)],
            [e(0x0020_0302, false)],
        ];
        let refs: Vec<&[u32]> = ents.iter().map(|x| &x[..]).collect();
        let raw = block_of(p::KIND_PC, &refs, 2, 24);
        let c = Capture::parse(&raw).unwrap();
        let pr = pc_profile(&c, &t);
        assert_eq!((pr.samples, pr.stride, pr.seen, pr.span_us), (6, 4, 24, 2_000_000));
        assert_eq!(pr.split, [[2, 1], [2, 1], [0, 0]]);
        assert_eq!(pr.functions[0].name, "firmware::idle");
        let draw = pr.functions.iter().find(|f| f.name == "firmware::ui::draw").unwrap();
        assert_eq!((draw.thread, draw.handler, draw.placement), (1, 1, Some(Placement::Paged)));
        let fw = pr.crates.iter().find(|c| c.krate == "firmware").unwrap();
        assert_eq!((fw.resident_thread, fw.paged_thread, fw.paged_handler), (2, 1, 1));
        let text = pr.text(10);
        assert!(text.contains("6 samples") && text.contains("one sample per 4 ticks"), "{text}");
        assert!(pr.csv_functions().lines().count() == 1 + pr.functions.len());
        assert!(pr.csv_crates().starts_with("samples,percent,"));
    }

    fn faults() -> Vec<[u32; 2]> {
        let f = |addr: u32, pc: u32, loaded: bool, data: bool, ms: u32| {
            p::encode_fault(addr, pc, loaded, data, ms * 1000)
        };
        vec![
            // ui::draw, then fs::read on the next line.
            f(0x0020_0010, 0x0020_0010, true, false, 0),
            f(0x0020_0410, 0x0020_0410, true, false, 10),
            // ui::row: three loads on two lines (window 1).
            f(0x0020_0510, 0x0020_0510, true, false, 1200),
            f(0x0020_0900, 0x0020_0900, true, false, 1300),
            f(0x0020_0510, 0x0020_0510, false, false, 1400),
            f(0x0020_0520, 0x0020_0520, true, false, 1500),
            // A literal pool read in fs::read from fs::read.
            f(0x0020_0310, 0x0020_0302, true, true, 1600),
            // The table, read by draw.
            f(0x0034_0010, 0x0020_0020, true, true, 1700),
            // Opus (clustered already).
            f(0x0020_0C04, 0x0020_0C04, true, false, 2100),
            // The monomorphs.
            f(0x0020_0A04, 0x0020_0A04, true, false, 2200),
            f(0x0020_0B04, 0x0020_0B04, true, false, 2300),
            f(0x0020_1004, 0x0020_1004, true, false, 2400),
        ]
    }

    fn fault_prof(t: &SymTab) -> FaultProfile {
        let ents = faults();
        let refs: Vec<&[u32]> = ents.iter().map(|x| &x[..]).collect();
        let raw = block_of(p::KIND_FAULTS, &refs, 0, ents.len() as u32);
        let c = Capture::parse(&raw).unwrap();
        fault_profile(
            &c,
            t,
            1000,
            Some(Totals { loads: 900, code_faults: 950, ro_faults: 50, evictions: 800, ms: 2500 }),
        )
    }

    #[test]
    fn fault_profile_aggregates_by_section_function_kind_and_window() {
        let t = tab();
        let p = fault_prof(&t);
        assert_eq!((p.events, p.loads), (12, 11));
        assert_eq!(p.kinds[&FaultKind::Code], 10);
        assert_eq!(p.kinds[&FaultKind::Literal], 1);
        assert_eq!(p.kinds[&FaultKind::Rodata], 1);
        let row = p.sections.iter().find(|r| r.label == "firmware::ui::row").unwrap();
        assert_eq!((row.events, row.loads, row.lines.len()), (4, 3, 2));
        // Hottest by loads first.
        assert_eq!(p.sections[0].label, "firmware::ui::row");
        let draw_fn = p.functions.iter().find(|f| f.name == "firmware::ui::draw").unwrap();
        assert_eq!((draw_fn.events, draw_fn.rodata), (2, 1));
        let read = p.sections.iter().find(|r| r.label == "orpheus_fs::read").unwrap();
        assert_eq!((read.events, read.kinds[&FaultKind::Literal]), (2, 1));
        assert_eq!(p.windows.len(), 3);
        assert_eq!(p.windows[1], Window { start_ms: 1000, events: 6, loads: 5, code_lines: 3, ro_lines: 1 });
        assert_eq!(p.code_lines, 5);
        assert_eq!(p.ro_lines, 1);
        let cov = &p.coverage[0];
        assert_eq!((cov.cluster.as_str(), cov.events, cov.lines), ("opus-celt", 1, 1));
        let mut t = t;
        t.cluster_bytes.insert("opus-celt".into(), 25 * 1024);
        let text = p.text(&t, 20);
        assert!(text.contains("900 line loads (360/s)"), "{text}");
        assert!(text.contains("(cluster: 25 lines, over the 24-line budget)"), "{text}");
        assert!(text.contains("opus-celt"), "{text}");
        assert_eq!(p.csv_windows().lines().count(), 4);
        assert!(p.csv_sections(&t).contains(",paged,,firmware,.text.c,firmware::ui::row"));
        assert!(p.csv_functions().starts_with("events,loads"));
    }

    #[test]
    fn fault_times_unwrap_past_18_bits() {
        let span = 1u32 << (p::TIME_BITS + p::TIME_SHIFT); // 268 s
        let ents = [
            p::encode_fault(0x0020_0000, 0, true, false, span - 2048),
            p::encode_fault(0x0020_0000, 0, true, false, span + 4096),
            p::encode_fault(0x0020_0000, 0, true, false, span + 8192),
        ];
        let refs: Vec<&[u32]> = ents.iter().map(|x| &x[..]).collect();
        let raw = block_of(p::KIND_FAULTS, &refs, 0, 3);
        let c = Capture::parse(&raw).unwrap();
        let t: Vec<u64> = unwrap_faults(&c).iter().map(|e| e.t_us).collect();
        assert_eq!(t, vec![u64::from(span - 2048), u64::from(span + 4096), u64::from(span + 8192)]);
    }

    #[test]
    fn generator_respects_budget_owners_and_order() {
        let t = tab();
        let p = fault_prof(&t);
        let g = generate("pictures", &p, &t, Budget { code_lines: 3, ro_lines: 3 });
        let pats: Vec<&str> = g.code.iter().map(|x| x.pattern.as_str()).collect();
        // Hottest first: row (3 loads), fs::read (2), then the 1-load ones
        // in label order while they fit 3 KiB: the two monomorphs share a
        // label and go in together (2 sections, 2 loads); draw (771 B)
        // does not fit any more, nor the 30 KiB function; Opus is another
        // cluster's.
        assert_eq!(pats, ["firmware::ui::row", "orpheus_fs::read", "<orpheus_img::Png>::step"]);
        assert_eq!(g.code[2].sections, 2);
        assert_eq!(g.code_bytes, 1283 + 515 + 518);
        assert!(g.code_bytes <= 3 * 1024, "{}", g.code_bytes);
        assert!(
            g.skipped.iter().any(|(l, w)| l == "orpheus_codec_opus::celt::decode" && w.contains("opus-celt"))
        );
        assert!(g.skipped.iter().any(|(l, w)| l == "firmware::big" && w.contains("do not fit")));
        assert!(g.skipped.iter().any(|(l, w)| l == "firmware::ui::draw" && w.contains("do not fit")));
        assert_eq!(g.ro.len(), 1);
        assert_eq!(g.ro[0].pattern, "orpheus_img::TABLE");
        assert!(g.picked_loads <= g.paged_loads);
        // A bigger budget takes everything that fits, still in heat order.
        let g2 = generate("pictures", &p, &t, Budget::default());
        let loads: Vec<u64> = g2.code.iter().map(|x| x.loads).collect();
        assert!(loads.windows(2).all(|w| w[0] >= w[1]), "{loads:?}");
        assert!(g2.code.iter().any(|x| x.pattern == "<orpheus_img::Png>::step" && x.sections == 2));
    }

    #[test]
    fn generator_keeps_its_own_cluster_members() {
        let mut t = tab();
        let mine = Cluster {
            name: "pictures".into(),
            bins: vec!["firmware".into()],
            symbols: vec!["firmware::ui::row".into()],
            sections: vec![],
        };
        let celt = Cluster {
            name: "opus-celt".into(),
            bins: vec![],
            symbols: vec!["orpheus_codec_opus::celt::*".into()],
            sections: vec![],
        };
        t.assign_owners(&[celt, mine]);
        let p = fault_prof(&t);
        let g = generate("pictures", &p, &t, Budget::default());
        assert_eq!(g.code[0].pattern, "firmware::ui::row");
    }

    #[test]
    fn literal_patterns_escape_globs() {
        let pat = literal_pattern("<*const u8 as core::fmt::Debug>::fmt");
        assert!(rules::symbol_matches(&pat, "<*const u8 as core::fmt::Debug>::fmt"));
        assert!(!rules::symbol_matches(&pat, "<*const u8 as core::fmt::Debug>::fmt_more"));
    }

    const HOT: &str = "# head\n\n# about a\n[[cluster]]\nname = \"a\"\nsymbols = [\"x::*\"]\n\n# about b\n[[cluster]]\nname = \"b\"\nbins = [\"firmware\"]\nsymbols = [\n    \"y::*\",\n]\n\n# about c\n[[cluster]]\nname = \"c\"\nsymbols = [\"z\"]\n";

    #[test]
    fn hot_toml_update_replaces_or_appends() {
        let t = tab();
        let p = fault_prof(&t);
        let g = generate("b", &p, &t, Budget::default());
        let block = g.block("test");
        let out = update_hot(HOT, "b", &block).unwrap();
        assert!(out.contains("# about b\n[[cluster]]\n# Generated by"), "{out}");
        assert!(out.contains("# about c\n[[cluster]]\nname = \"c\""), "{out}");
        assert!(!out.contains("\"y::*\""));
        let cs = rules::parse_hot(&out).unwrap();
        assert_eq!(cs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        let b = &cs[1];
        assert_eq!(b.symbols[0], "firmware::ui::row");
        assert!(b.sections.is_empty() || b.sections.iter().all(|s| s.starts_with('.')));
        // Again: idempotent.
        assert_eq!(update_hot(&out, "b", &block).unwrap(), out);
        // A new name goes at the end.
        let out2 =
            update_hot(HOT, "pictures", &generate("pictures", &p, &t, Budget::default()).block("t")).unwrap();
        let cs2 = rules::parse_hot(&out2).unwrap();
        assert_eq!(cs2.last().unwrap().name, "pictures");
        assert!(out2.starts_with(HOT.trim_end()));
    }

    /// A minimal ELF32 (ARM, little-endian) with a symbol table: two Thumb
    /// functions and an object.
    fn tiny_elf() -> Vec<u8> {
        let strtab = b"\0f_one\0f_two\0obj\0".to_vec();
        let shstr = b"\0.text\0.symtab\0.strtab\0.shstrtab\0".to_vec();
        let sym = |name: u32, value: u32, size: u32, info: u8, shndx: u16| {
            let mut v = Vec::new();
            v.extend(name.to_le_bytes());
            v.extend(value.to_le_bytes());
            v.extend(size.to_le_bytes());
            v.push(info);
            v.push(0);
            v.extend(shndx.to_le_bytes());
            v
        };
        let mut symtab = vec![0u8; 16];
        symtab.extend(sym(1, 0x0020_0001, 0x40, 0x12, 1)); // FUNC GLOBAL, Thumb
        symtab.extend(sym(7, 0x0020_0041, 0x20, 0x12, 1));
        symtab.extend(sym(13, 0x0020_0100, 0x10, 0x11, 1)); // OBJECT
        let text = vec![0u8; 0x200];
        let mut body = Vec::new();
        let ehsize = 52usize;
        let off_text = ehsize;
        body.extend(&text);
        let off_sym = ehsize + body.len();
        body.extend(&symtab);
        let off_str = ehsize + body.len();
        body.extend(&strtab);
        let off_shs = ehsize + body.len();
        body.extend(&shstr);
        while (ehsize + body.len()) % 4 != 0 {
            body.push(0);
        }
        let shoff = ehsize + body.len();
        let sh = |name: u32,
                  ty: u32,
                  flags: u32,
                  addr: u32,
                  off: usize,
                  size: usize,
                  link: u32,
                  info: u32,
                  align: u32,
                  entsize: u32| {
            let mut v = Vec::new();
            for w in [name, ty, flags, addr, off as u32, size as u32, link, info, align, entsize] {
                v.extend(w.to_le_bytes());
            }
            v
        };
        let mut shdrs = vec![0u8; 40];
        shdrs.extend(sh(1, 1, 6, 0x0020_0000, off_text, text.len(), 0, 0, 4, 0));
        shdrs.extend(sh(7, 2, 0, 0, off_sym, symtab.len(), 3, 1, 4, 16));
        shdrs.extend(sh(15, 3, 0, 0, off_str, strtab.len(), 0, 0, 1, 0));
        shdrs.extend(sh(23, 3, 0, 0, off_shs, shstr.len(), 0, 0, 1, 0));
        let mut e = vec![0x7F, b'E', b'L', b'F', 1, 1, 1, 0];
        e.extend([0u8; 8]);
        e.extend(2u16.to_le_bytes()); // ET_EXEC
        e.extend(40u16.to_le_bytes()); // EM_ARM
        e.extend(1u32.to_le_bytes());
        e.extend(0x0020_0001u32.to_le_bytes());
        e.extend(0u32.to_le_bytes()); // phoff
        e.extend((shoff as u32).to_le_bytes());
        e.extend(0x0500_0000u32.to_le_bytes()); // EABI5
        e.extend((ehsize as u16).to_le_bytes());
        e.extend(32u16.to_le_bytes());
        e.extend(0u16.to_le_bytes());
        e.extend(40u16.to_le_bytes());
        e.extend(5u16.to_le_bytes());
        e.extend(4u16.to_le_bytes());
        e.extend(body);
        e.extend(shdrs);
        e
    }

    #[test]
    fn elf_functions_clear_the_thumb_bit() {
        let f = elf_functions(&tiny_elf()).unwrap();
        assert_eq!(
            f,
            vec![(0x0020_0000, 0x40, "f_one".to_string()), (0x0020_0040, 0x20, "f_two".to_string())]
        );
        let t = SymTab::from_parts(vec![sec(".text.x", "x::y", "code", 0x0020_0000, 0x200)], f);
        assert_eq!(t.name(0x0020_0044).0, "f_two");
        assert_eq!(t.name(0x0020_0000).0, "f_one");
        // Past both functions: the section.
        assert_eq!(t.name(0x0020_0080).0, "x::y");
    }
}
