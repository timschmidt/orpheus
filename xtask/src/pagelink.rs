//! `xtask pagelink`: split a linked image into a resident image and a NOR
//! page store (docs/plan/02-architecture.md §3.3).
//!
//! 1. Link pass 1 again from the saved link (`savedlink`) with a map and
//!    `--emit-relocs`: everything resident in a large fake region
//!    (`fw/firmware/link/paged-flat.ld`).
//! 2. Classify every input section ([`classify`]): RAM sections stay where
//!    they are; code and rodata are resident by rule (`resident.toml`), by
//!    being reachable from a hard root (the closure over the relocation
//!    graph), or because they cannot move (start-up, mergeable,
//!    linker-generated, ambiguous names, rodata larger than a group);
//!    everything else is paged.
//! 3. Pack paged sections ([`pack`]): `hot.toml` clusters first, each
//!    starting on a line boundary, then the rest by crate and link order.
//!    Code pages are VA-contiguous over the whole 1 MiB code window
//!    (`VA = 0x200000 + p * 8 KiB`), so code is one output section and a
//!    function may cross page boundaries anywhere; rodata goes into 16 KiB
//!    groups (two VA-contiguous pages at the start of each 32 KiB alias
//!    block), first fit. Cold code (`resident.toml` `[cold]`, PG-2) goes
//!    into rodata groups of its own after the constants' (`.px_gN`,
//!    executable pages), first fit, avoiding the physical lines of the
//!    constants it refers to where it can.
//! 4. Generate the pass-2 script (memory.ld, the page output sections at
//!    their alias addresses, then layout.ld) and link again.
//! 5. Check the final placement against the plan, extract the resident image
//!    (SRAM0 sections) and the page store (code pages, then rodata pages,
//!    8 KiB each, unused bytes 0xFF), write the `"YFW1"` slot image and
//!    `layout.json`.
//!
//! The page output sections are ordinary allocated sections (own PT_LOAD
//! segments at the alias addresses): lld's `(COPY)`/`(INFO)` types make a
//! section non-allocated *with address 0*, which breaks every relocation
//! into it. Nothing loads the ELF directly; the slot image is built from
//! sections.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use object::{Object, ObjectSection, SectionKind};
use orpheus_pager::{
    ALIAS_BLOCK, CODE_WIN, LINE_SIZE, MAX_CODE_PAGES, MAX_RO_PAGES, PAGE_SIZE, RO_GROUP_SIZE, code_page_va,
    ro_page_va,
};
use orpheus_proto::hash::{crc32, sum32};
use orpheus_proto::slot::{LAYOUT_DUAL, LAYOUT_FULL, SlotHeader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::args::Args;
use crate::linkgraph::{Graph, Kind, crate_of};
use crate::linkmap;
use crate::rules::{self, Cluster, Rules};
use crate::savedlink::SavedLink;
use crate::util::{log, out_dir, root};

pub const SRAM0: std::ops::Range<u32> = 0x0010_0000..0x0013_8000;
pub const CODE_OUT: &str = ".pg_code";
const SECTOR: u32 = 0x1000;
const MAX_RO_GROUPS: u32 = MAX_RO_PAGES / 2;

/// Output files of one binary in `target/orpheus/` (the firmware keeps the
/// plain names of 07 §2).
pub struct Artefacts {
    pub elf: PathBuf,
    pub map: PathBuf,
    pub flat_elf: PathBuf,
    pub flat_map: PathBuf,
    pub script: PathBuf,
    pub yfw: PathBuf,
    pub resident: PathBuf,
    pub pages: PathBuf,
    pub layout: PathBuf,
}

pub fn artefacts(bin: &str) -> Artefacts {
    let d = out_dir();
    let p = |plain: &str, other: &str| {
        if bin == "firmware" { d.join(plain) } else { d.join(format!("{bin}.{other}")) }
    };
    Artefacts {
        elf: d.join(format!("{bin}.elf")),
        map: d.join(format!("{bin}.map")),
        flat_elf: d.join(format!("{bin}.flat.elf")),
        flat_map: d.join(format!("{bin}.flat.map")),
        script: d.join(format!("{bin}.pages.ld")),
        yfw: d.join(format!("{bin}.yfw")),
        resident: p("resident.bin", "resident.bin"),
        pages: p("pages.bin", "pages.bin"),
        layout: p("layout.json", "layout.json"),
    }
}

/// `layout.json`: what went where, sizes and margins.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Layout {
    pub format: u32,
    pub bin: String,
    pub elf: String,
    pub map: String,
    pub build_id: String,
    pub slot: SlotInfo,
    pub resident: ResidentInfo,
    pub pages: PagesInfo,
    pub groups: Vec<GroupInfo>,
    pub clusters: Vec<ClusterInfo>,
    pub roots: Vec<RootInfo>,
    pub sections: Vec<SectionInfo>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlotInfo {
    pub layout: String,
    pub bytes: u32,
    pub resident_off: u32,
    pub resident_len: u32,
    pub resident_sum: u32,
    pub pages_off: u32,
    pub pages_len: u32,
    pub pages_crc: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResidentInfo {
    pub load_addr: u32,
    pub entry: u32,
    /// Loaded bytes (`.text` incl. start-up and vectors, `.rodata`, `.data`).
    pub image_bytes: u32,
    pub text: u32,
    pub rodata: u32,
    pub data: u32,
    /// `.bss` + `.uninit`.
    pub bss: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PagesInfo {
    pub code: u32,
    pub rodata: u32,
    pub code_max: u32,
    pub rodata_max: u32,
    /// Bytes of paged sections (without padding).
    pub code_bytes: u32,
    pub rodata_bytes: u32,
    /// Code window left empty before `starts_over` clusters.
    #[serde(default)]
    pub code_padding: u32,
    /// Bytes of code in no cluster placed in the line before a cluster's
    /// start (what used to be alignment gaps).
    #[serde(default)]
    pub code_filler: u32,
    /// Rodata pages (the last ones, counted in `rodata`) that hold cold code
    /// (PG-2), and the bytes of that code.
    #[serde(default)]
    pub cold: u32,
    #[serde(default)]
    pub cold_bytes: u32,
    /// Cold code sections that share a physical line of the rodata cache
    /// with a constant they refer to (each such read is a bounce).
    #[serde(default)]
    pub cold_conflicts: u32,
    /// Loads in cold code (not PC-relative), and those the trampoline
    /// could not run (`orpheus_pager::bounce` refuses them: fatal should
    /// their constant ever share their line).
    #[serde(default)]
    pub cold_loads: u32,
    #[serde(default)]
    pub cold_unbounceable: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupInfo {
    pub name: String,
    pub kind: String,
    pub va: u32,
    pub size: u32,
    pub capacity: u32,
    pub first_page: u32,
    pub pages: u32,
}

/// A `hot.toml` cluster of paged code: first byte and extent (lines =
/// `bytes / 1 KiB`, rounded up).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClusterInfo {
    pub name: String,
    pub sections: u32,
    pub va: u32,
    pub bytes: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RootInfo {
    pub pattern: String,
    pub matched: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionInfo {
    pub section: String,
    pub symbol: String,
    #[serde(rename = "crate")]
    pub krate: String,
    pub kind: String,
    /// `resident`, `paged` or `ram`.
    pub placement: String,
    pub reason: String,
    pub addr: u32,
    pub size: u32,
    /// Page number (code or rodata, per `kind`) of the first byte.
    pub page: Option<u32>,
    /// Offset of the first byte in the page store.
    pub store_offset: Option<u32>,
}

impl Layout {
    pub fn load(path: &Path) -> Result<Self> {
        let t = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Ok(serde_json::from_str(&t)?)
    }
}

pub struct Options {
    pub bin: String,
    pub release: bool,
    pub layout: u32,
    /// Extra `paged.symbols` patterns (planting violations in tests).
    pub force_paged: Vec<String>,
    /// Output name (defaults to `bin`).
    pub name: Option<String>,
}

/// Placement decided in pass 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    Ram,
    Resident(String),
    Paged(String),
}

/// `hot.toml` `resident_constants` (PG-2b): the small plain constants a
/// cluster's paged code reads directly become resident. Returns how many
/// sections and bytes.
pub fn cluster_constants(g: &Graph, places: &mut [Place], hot: &[Cluster]) -> (usize, u32) {
    let (mut n_secs, mut bytes) = (0, 0);
    for c in hot {
        let Some(max) = c.resident_constants else { continue };
        for n in 0..g.nodes.len() {
            if g.kinds[n] != Kind::Code
                || !matches!(places[n], Place::Paged(_))
                || c.rank(&g.nodes[n]).is_none()
            {
                continue;
            }
            for &m in &g.edges[n] {
                let s = &g.nodes[m];
                if g.kinds[m] == Kind::Rodata
                    && matches!(places[m], Place::Paged(_))
                    && !merged(s)
                    && s.size <= max
                    && g.edges[m].is_empty()
                {
                    places[m] = Place::Resident(format!("cluster-constants:{}", c.name));
                    n_secs += 1;
                    bytes += s.size;
                }
            }
        }
    }
    (n_secs, bytes)
}

/// Resolve the hard roots to nodes: `(pattern, nodes)`.
pub fn resolve_roots(g: &Graph, rules: &Rules) -> Vec<(String, Vec<usize>)> {
    rules
        .roots
        .iter()
        .map(|r| {
            let nodes: Vec<usize> = if r == "@vectors" {
                let mut v: Vec<usize> =
                    g.vector_targets(&rules.vector_skip).into_iter().map(|(_, n)| n).collect();
                v.sort_unstable();
                v.dedup();
                v
            } else {
                (0..g.nodes.len())
                    .filter(|&n| {
                        g.nodes[n].symbols.iter().any(|s| {
                            s.name == *r || rules::symbol_matches(r, &crate::linkgraph::demangle(&s.name))
                        })
                    })
                    .collect()
            };
            (r.clone(), nodes)
        })
        .collect()
}

/// Why a code/rodata section cannot be moved into a page, if so.
fn unmovable(g: &Graph, n: usize, unique: &BTreeMap<&str, usize>) -> Option<&'static str> {
    let s = &g.nodes[n];
    if s.name == ".init" || s.name == ".vectors" {
        return Some("start-up");
    }
    if !(s.name.starts_with(".text.") || s.name.starts_with(".rodata.")) {
        return Some("unnamed section");
    }
    if s.file == "<internal>" {
        return Some("linker-generated");
    }
    if s.name.starts_with(".rodata.str") || s.name.starts_with(".rodata.cst") {
        return Some("mergeable");
    }
    if !unique.contains_key(s.name.as_str()) {
        return Some("section name not unique");
    }
    None
}

/// A section the linker merged from mergeable input (`SHF_MERGE`: string
/// literals, `.rodata.str*`; constant pools, `.rodata.cst*`): one
/// linker-generated section per name, which a pass-2 script rule places
/// whole like any other input section.
fn merged(s: &crate::linkmap::InputSection) -> bool {
    s.file == "<internal>" && (s.name.starts_with(".rodata.str") || s.name.starts_with(".rodata.cst"))
}

/// The reason of a section left resident because it is larger than a
/// rodata group (no group can hold it; 02 §3.3).
pub const OVERSIZED: &str = "fixed:larger than a rodata group";

/// Classify every node of the pass-1 graph. Returns the placements and
/// warnings.
///
/// A merged section ([`merged`]) is paged when only paged sections refer
/// to it, no rule keeps it resident and no root reaches it (2026-11-26:
/// the 8,042 B of panic-location file names in `.rodata.str1.1` were
/// resident as "linker-generated" though only paged code used them).
pub fn classify(g: &Graph, rules: &Rules, roots: &[(String, Vec<usize>)]) -> (Vec<Place>, Vec<String>) {
    let unique = g.unique_names();
    let root_nodes: Vec<usize> = roots.iter().flat_map(|(_, v)| v.iter().copied()).collect();
    let parent = g.walk(&root_nodes);
    let mut warnings = Vec::new();
    let mut later = Vec::new();
    let mut places: Vec<Place> = (0..g.nodes.len())
        .map(|n| {
            let s = &g.nodes[n];
            let kind = g.kinds[n];
            if !matches!(kind, Kind::Code | Kind::Rodata) {
                return Place::Ram;
            }
            if merged(s) && unique.contains_key(s.name.as_str()) {
                later.push(n);
                return Place::Resident("fixed:merged".into());
            }
            if let Some(why) = unmovable(g, n, &unique) {
                return Place::Resident(format!("fixed:{why}"));
            }
            let too_big = kind == Kind::Rodata && s.size > RO_GROUP_SIZE;
            if let Some(p) = rules.paged.select(s) {
                if too_big {
                    warnings.push(format!("{} forced paged but larger than a rodata group", g.label(n)));
                    return Place::Resident(OVERSIZED.into());
                }
                if parent[n].is_some() {
                    warnings.push(format!("{} is reachable from a root but forced paged ({p})", g.label(n)));
                }
                return Place::Paged(format!("forced:{p}"));
            }
            if let Some(p) = rules.resident.select(s) {
                return Place::Resident(format!("rule:{p}"));
            }
            if parent[n].is_some() {
                return Place::Resident(format!("closure:{}", g.chain(&parent, n).join(" -> ")));
            }
            if too_big {
                warnings.push(format!(
                    "{} ({} bytes) is larger than a rodata group: resident",
                    g.label(n),
                    s.size
                ));
                return Place::Resident(OVERSIZED.into());
            }
            Place::Paged("default".into())
        })
        .collect();
    // Merged sections: after everything that refers to them is placed.
    for n in later {
        let s = &g.nodes[n];
        places[n] = if let Some(p) = rules.resident.select(s) {
            Place::Resident(format!("rule:{p}"))
        } else if parent[n].is_some() {
            Place::Resident(format!("closure:{}", g.chain(&parent, n).join(" -> ")))
        } else if s.size > RO_GROUP_SIZE {
            Place::Resident(format!("fixed:merged, {OVERSIZED}"))
        } else if let Some(r) = (0..g.nodes.len()).find(|&m| {
            m != n && g.edges[m].binary_search(&n).is_ok() && !matches!(places[m], Place::Paged(_))
        }) {
            Place::Resident(format!("fixed:merged, used by {} (not paged)", g.label(r)))
        } else {
            Place::Paged("merged: only paged sections use it".into())
        };
    }
    (places, warnings)
}

/// Sections left resident only because they are larger than a rodata
/// group: `pagelink` refuses them (2026-11-26: the UI font, 17,794 B, was
/// resident behind a warning on every build). Split the data (`orpheus_ui::font`
/// links a large font in two pieces) or name it in `resident.toml`.
pub fn oversized(g: &Graph, places: &[Place]) -> Vec<String> {
    (0..g.nodes.len())
        .filter(|&n| matches!(&places[n], Place::Resident(r) if r == OVERSIZED))
        .map(|n| {
            let referrers: Vec<String> = (0..g.nodes.len())
                .filter(|&m| g.edges[m].binary_search(&n).is_ok())
                .take(3)
                .map(|m| g.label(m))
                .collect();
            format!(
                "{} ({} B, used by {}) is larger than a {RO_GROUP_SIZE} B rodata group",
                g.label(n),
                g.nodes[n].size,
                if referrers.is_empty() { "nothing".to_string() } else { referrers.join(", ") }
            )
        })
        .collect()
}

/// The crate each node belongs to: from its first symbol, else from the
/// first node referencing it.
pub fn crates(g: &Graph) -> Vec<String> {
    let own: Vec<Option<String>> = (0..g.nodes.len())
        .map(|n| {
            g.nodes[n].symbols.first().map(|s| {
                let d = crate::linkgraph::demangle(&s.name);
                crate_of(&d).map_or_else(|| "(c)".to_string(), str::to_string)
            })
        })
        .collect();
    let mut referrer: Vec<Option<usize>> = vec![None; g.nodes.len()];
    for (from, es) in g.edges.iter().enumerate() {
        for &to in es {
            if referrer[to].is_none() && own[from].is_some() {
                referrer[to] = Some(from);
            }
        }
    }
    (0..g.nodes.len())
        .map(|n| {
            own[n]
                .clone()
                .or_else(|| referrer[n].and_then(|r| own[r].clone()))
                .unwrap_or_else(|| "(anon)".into())
        })
        .collect()
}

/// Placement of paged sections.
#[derive(Debug, Default)]
pub struct Plan {
    /// `.pg_code`: (node, offset, starts a cluster).
    pub code: Vec<(usize, u32, bool)>,
    pub code_size: u32,
    /// Rodata groups: (node, offset) and used size.
    pub groups: Vec<(Vec<(usize, u32)>, u32)>,
    /// Cold code groups, after the rodata groups (group index
    /// `groups.len() + i`): (node, offset) and used size.
    pub cold: Vec<(Vec<(usize, u32)>, u32)>,
    /// Rodata pages (the last ones) that hold cold code.
    pub nro_code: u32,
    /// Cold sections that share a physical line with a constant they use.
    pub cold_conflicts: u32,
    /// Those sections (nodes).
    pub conflicted: Vec<usize>,
    pub ncode: u32,
    pub nro: u32,
    /// Where the `hot.toml` clusters of code went.
    pub clusters: Vec<ClusterInfo>,
    /// Bytes left empty before `starts_over` clusters.
    pub padding: u32,
    /// Bytes of unclustered code placed in the line before a cluster's
    /// start.
    pub filler: u32,
    /// Bytes of constants at the start of the first cold group (a shared
    /// constants' group, [`pack_cold_sharing`]); 0 for none.
    pub shared: u32,
}

impl Plan {
    /// Final address of every paged node.
    pub fn addresses(&self) -> BTreeMap<usize, u32> {
        let mut m = BTreeMap::new();
        for &(n, off, _) in &self.code {
            m.insert(n, code_page_va(0) + off);
        }
        for (g, (secs, _)) in self.all_groups().enumerate() {
            for &(n, off) in secs {
                m.insert(n, ro_page_va(2 * g as u32) + off);
            }
        }
        m
    }

    /// Rodata groups, then cold code groups, in rodata-window order.
    pub fn all_groups(&self) -> impl Iterator<Item = &(Vec<(usize, u32)>, u32)> {
        self.groups.iter().chain(&self.cold)
    }

    /// Output section name of window group `gi` (`.pr_gN` constants,
    /// `.px_gN` cold code).
    #[must_use]
    /// Whether group `gi` is a shared constants' group (constants, then
    /// cold code; [`pack_cold_sharing`]).
    pub fn is_shared(&self, gi: usize) -> bool {
        self.shared > 0 && gi == self.groups.len()
    }

    pub fn group_name(&self, gi: usize) -> String {
        if gi < self.groups.len() { format!(".pr_g{gi}") } else { format!(".px_g{gi}") }
    }
}

/// The physical line of the rodata cache an address of the rodata window
/// maps to (0–15: its offset in a 16 KiB group).
const fn ro_line(va: u32) -> u32 {
    (va - orpheus_pager::RO_WIN) % ALIAS_BLOCK / LINE_SIZE
}

/// Paged code that goes to the rodata window: matches `[cold]`, in no
/// cluster, at most one rodata group. Returns the set and one warning per
/// cold-matching section that could not go (too large).
pub fn cold_set(
    g: &Graph,
    places: &[Place],
    rules: &Rules,
    hot: &[Cluster],
) -> (BTreeSet<usize>, Vec<String>) {
    let mut set = BTreeSet::new();
    let mut warnings = Vec::new();
    for (n, place) in places.iter().enumerate().take(g.nodes.len()) {
        let s = &g.nodes[n];
        if g.kinds[n] != Kind::Code || !matches!(place, Place::Paged(_)) || rules.cold.select(s).is_none() {
            continue;
        }
        if hot.iter().any(|c| c.rank(s).is_some()) {
            continue;
        }
        if s.size > RO_GROUP_SIZE {
            warnings.push(format!(
                "{} ({} B) is cold but larger than a rodata group: code window",
                g.label(n),
                s.size
            ));
            continue;
        }
        set.insert(n);
    }
    (set, warnings)
}

/// Physical lines of the code cache: it is direct-mapped, a line's place is
/// its offset in the code window modulo one alias block (02 §3.1).
const PHYS_LINES: u32 = ALIAS_BLOCK / LINE_SIZE;

/// The physical lines a placed cluster occupies, in order (a cluster longer
/// than the cache repeats some).
fn phys_lines(c: &ClusterInfo) -> Vec<u32> {
    let first = (c.va - CODE_WIN) / LINE_SIZE;
    let last = (c.va + c.bytes.max(1) - 1 - CODE_WIN) / LINE_SIZE;
    (first..=last).map(|l| l % PHYS_LINES).collect()
}

/// `fits_with` (hot.toml): a cluster and the clusters it runs with must
/// not share a physical line (AU-PERF: lines that share one evict each
/// other on every pass). One warning per offending pair or cluster.
pub fn fit_warnings(hot: &[Cluster], placed: &[ClusterInfo]) -> Vec<String> {
    let by_name = |n: &str| placed.iter().find(|c| c.name == n);
    let mut out = Vec::new();
    for h in hot.iter().filter(|h| !h.fits_with.is_empty()) {
        let Some(me) = by_name(&h.name) else { continue };
        let mut group = vec![me];
        for n in &h.fits_with {
            match by_name(n) {
                Some(c) => group.push(c),
                None => out.push(format!("cluster {}: fits_with `{n}` is not a placed cluster", h.name)),
            }
        }
        for (i, a) in group.iter().enumerate() {
            let la = phys_lines(a);
            let mut uniq = la.clone();
            uniq.sort_unstable();
            uniq.dedup();
            if i == 0 && uniq.len() < la.len() {
                out.push(format!(
                    "cluster {}: {} lines, more than the {PHYS_LINES} of the cache",
                    a.name,
                    la.len()
                ));
            }
            for b in &group[i + 1..] {
                let shared = phys_lines(b).iter().filter(|l| la.contains(l)).count();
                if shared > 0 {
                    out.push(format!(
                        "clusters {} and {} share {shared} physical line(s) of the code cache ({}: fits_with)",
                        a.name, b.name, h.name
                    ));
                }
            }
        }
    }
    out
}

fn align_up(v: u32, a: u32) -> u32 {
    let a = a.max(1);
    v.div_ceil(a) * a
}

/// Code window pages that cold placement keeps free (02 §3.6): cold code
/// comes back into the code window, the warmest first, while this much of
/// it stays empty — unless the rodata window cannot hold the rest, then
/// the code window fills up ([`place_cold`]).
pub const CODE_RESERVE_PAGES: u32 = 1;

/// Pack with only as much cold code in the rodata window as the code window
/// needs: all of `cold` first (the code window must fit then), then the
/// warmest cold sections (the last `[cold]` patterns: `resident.toml`
/// lists them coldest first) come back while the code window keeps
/// [`CODE_RESERVE_PAGES`] free. Before those, the cold sections that
/// still share a physical line with a constant they read come back
/// (PG-2b: a loop there pays two line loads per pass; SHA-256's rounds ran
/// an update at 2.5 KB/s). Returns the plan and the cold set it used.
///
/// When the rodata window cannot hold what is left cold with the reserve
/// kept, the reserve goes too (BOOK-1/2, 2026-11-27: speech and Books
/// with PG-2b's padded cold placement needed 33 of the 32 rodata groups,
/// 3,714 B over, with the code window at 127 pages): the two windows
/// share the free room, and when there is none the code window takes
/// what it can.
///
/// When even that does not fit, the constants' group with the most room
/// is shared: it becomes the first cold group and cold code fills the room
/// after its constants ([`pack_cold_sharing`]; PERF-SD round 2,
/// 2026-11-27: the 32 groups were full, the 33rd would have held 5.6 KB of
/// cold code, while the constants' groups had 20 KB free, 8.9 KB in one).
pub fn place_cold(
    g: &Graph,
    places: &[Place],
    hot: &[Cluster],
    crates: &[String],
    rules: &Rules,
    cold: &BTreeSet<usize>,
) -> Result<(Plan, BTreeSet<usize>)> {
    let mut last = None;
    for share in [false, true] {
        for reserve in [CODE_RESERVE_PAGES, 0] {
            let p = place_cold_reserving(g, places, hot, crates, rules, cold, reserve, share)?;
            if check_rodata(&p.0).is_ok() {
                return Ok(p);
            }
            last = Some(p);
        }
    }
    let Some(p) = last else { unreachable!() };
    check_rodata(&p.0)?;
    Ok(p)
}

/// [`place_cold`] with `reserve` code pages kept free (and a constants'
/// group shared with cold code when `share`).
#[allow(clippy::too_many_arguments)]
fn place_cold_reserving(
    g: &Graph,
    places: &[Place],
    hot: &[Cluster],
    crates: &[String],
    rules: &Rules,
    cold: &BTreeSet<usize>,
    reserve: u32,
    share: bool,
) -> Result<(Plan, BTreeSet<usize>)> {
    let pack_cold = |g: &Graph, places: &[Place], hot: &[Cluster], crates: &[String], cold: &BTreeSet<usize>| {
        pack_cold_sharing(g, places, hot, crates, cold, share)
    };
    let mut plan = pack_cold(g, places, hot, crates, cold)?;
    let cap = (MAX_CODE_PAGES - reserve) * PAGE_SIZE;
    let rank = |n: usize| rules.cold.symbol_rank(&g.nodes[n]).unwrap_or(0);
    let mut warm: Vec<usize> = cold.iter().copied().collect();
    warm.sort_by(|&a, &b| rank(b).cmp(&rank(a)).then(g.nodes[a].addr.cmp(&g.nodes[b].addr)));
    let mut set = cold.clone();
    let mut size = plan.code_size;
    let mut back = Vec::new();
    // Conflicts first: cold code that shares a physical line with a
    // constant it reads even after `pack_cold` looked for a clear place
    // (PG-2b: a loop there pays two line loads per pass). A few rounds:
    // bringing some back moves the rest.
    for _ in 0..4 {
        let mut moved = false;
        for &n in &plan.conflicted {
            let s = &g.nodes[n];
            let grown = align_up(size, s.align) + s.size;
            if set.contains(&n) && grown <= cap {
                size = grown;
                set.remove(&n);
                back.push(n);
                moved = true;
            }
        }
        if !moved {
            break;
        }
        plan = pack_cold(g, places, hot, crates, &set)?;
        size = plan.code_size;
    }
    // Then the warmest.
    for n in warm {
        if !set.contains(&n) {
            continue;
        }
        let s = &g.nodes[n];
        let grown = align_up(size, s.align) + s.size;
        if grown <= cap {
            size = grown;
            set.remove(&n);
            back.push(n);
        }
    }
    // The estimate ignores the fillers' and clusters' alignment: repack,
    // and return the last ones taken back until the plan fits the reserve
    // (the warm ones go out again first, the conflicts last).
    loop {
        match pack_cold(g, places, hot, crates, &set) {
            Ok(p) if p.code_size <= cap || back.is_empty() => {
                plan = p;
                break;
            }
            _ => {
                let Some(n) = back.pop() else { break };
                set.insert(n);
            }
        }
    }
    Ok((plan, set))
}

/// The rodata window holds the constants' and the cold code's groups.
pub fn check_rodata(plan: &Plan) -> Result<()> {
    let ngroups = (plan.groups.len() + plan.cold.len()) as u32;
    if ngroups > MAX_RO_GROUPS {
        let used = |g: &[(Vec<(usize, u32)>, u32)]| g.iter().map(|(_, u)| u.to_string()).collect::<Vec<_>>().join(" ");
        bail!(
            "paged rodata and cold code need {ngroups} groups of 16 KiB ({} + {}; used {} | {}), the window holds {MAX_RO_GROUPS}",
            plan.groups.len(),
            plan.cold.len(),
            used(&plan.groups),
            used(&plan.cold),
        );
    }
    Ok(())
}

/// Order and pack the paged sections (no cold code).
#[cfg(test)]
pub fn pack(g: &Graph, places: &[Place], hot: &[Cluster], crates: &[String]) -> Result<Plan> {
    pack_cold(g, places, hot, crates, &BTreeSet::new())
}

/// Order and pack the paged sections; the `cold` code sections go to the
/// rodata window.
#[cfg(test)]
pub fn pack_cold(
    g: &Graph,
    places: &[Place],
    hot: &[Cluster],
    crates: &[String],
    cold: &BTreeSet<usize>,
) -> Result<Plan> {
    pack_cold_sharing(g, places, hot, crates, cold, false)
}

/// [`pack_cold`]; with `share`, the constants' group with the most room
/// moves to the end of the constants and is the first cold group: cold
/// code goes after its constants. Its pages become executable with the
/// other cold pages (`nro_code`); the script gives its constants and its
/// code an output section each (`.pr_gN`, `.px_gN`, [`Plan::shared`]).
/// Constants and code of one group never share a physical line, so this
/// adds no kind of line conflict.
pub fn pack_cold_sharing(
    g: &Graph,
    places: &[Place],
    hot: &[Cluster],
    crates: &[String],
    cold: &BTreeSet<usize>,
    share: bool,
) -> Result<Plan> {
    let paged = |k: Kind| -> Vec<usize> {
        (0..g.nodes.len())
            .filter(|&n| g.kinds[n] == k && matches!(places[n], Place::Paged(_)) && !cold.contains(&n))
            .collect()
    };
    // Order: clusters (pattern order, then link order), then the rest by
    // crate and link order.
    let order = |nodes: Vec<usize>| -> Vec<(usize, Option<usize>)> {
        let mut taken = BTreeSet::new();
        let mut out = Vec::new();
        for (ci, c) in hot.iter().enumerate() {
            let mut members: Vec<(usize, usize)> = nodes
                .iter()
                .filter(|n| !taken.contains(*n))
                .filter_map(|&n| c.rank(&g.nodes[n]).map(|r| (r, n)))
                .collect();
            members.sort_by_key(|&(r, n)| (r, g.nodes[n].addr));
            for &(_, n) in &members {
                taken.insert(n);
                out.push((n, Some(ci)));
            }
        }
        let mut cold: Vec<usize> = nodes.into_iter().filter(|n| !taken.contains(n)).collect();
        cold.sort_by(|&a, &b| crates[a].cmp(&crates[b]).then(g.nodes[a].addr.cmp(&g.nodes[b].addr)));
        out.extend(cold.into_iter().map(|n| (n, None)));
        out
    };
    let mut plan = Plan::default();
    let mut cur = 0u32;
    let mut prev: Option<usize> = None;
    let code_order = order(paged(Kind::Code));
    // Code in no cluster, largest first: what fills the end of the line
    // before a cluster's line-aligned start.
    let mut fillers: Vec<usize> = code_order.iter().filter(|(_, c)| c.is_none()).map(|&(n, _)| n).collect();
    fillers.sort_by_key(|&n| std::cmp::Reverse(g.nodes[n].size));
    let mut filled = BTreeSet::new();
    for (n, cluster) in code_order.iter().copied().filter(|(_, c)| c.is_some()).chain(
        // Then the rest, in order, less what went into the gaps.
        code_order.iter().copied().filter(|(_, c)| c.is_none()),
    ) {
        if cluster.is_none() && filled.contains(&n) {
            continue;
        }
        let start = cluster.is_some() && cluster != prev;
        if start {
            // The rest of the line before a cluster's first line: cold code
            // that fits it (2026-11-26: 7,458 B of what 18 clusters left
            // empty). It shares a line only with the end of the
            // cluster before, so it evicts nothing that cluster does not.
            let line_end = align_up(cur, LINE_SIZE);
            let mut i = 0;
            while cur < line_end && i < fillers.len() {
                let f = fillers[i];
                let s = &g.nodes[f];
                let at = align_up(cur, s.align);
                if !filled.contains(&f) && at + s.size <= line_end {
                    plan.code.push((f, at, false));
                    plan.filler += s.size;
                    filled.insert(f);
                    cur = at + s.size;
                }
                i += 1;
            }
            cur = align_up(cur, LINE_SIZE);
            if let Some(ci) = cluster
                && let Some(over) = &hot[ci].starts_over
            {
                let Some(o) = plan.clusters.iter().find(|c| c.name == *over) else {
                    bail!("cluster {}: starts_over `{over}` is not a cluster placed before it", hot[ci].name);
                };
                let want = (o.va - CODE_WIN) / LINE_SIZE % PHYS_LINES;
                let have = cur / LINE_SIZE % PHYS_LINES;
                let pad = (want + PHYS_LINES - have) % PHYS_LINES;
                plan.padding += pad * LINE_SIZE;
                cur += pad * LINE_SIZE;
            }
        }
        let s = &g.nodes[n];
        cur = align_up(cur, s.align);
        plan.code.push((n, cur, start));
        if let Some(ci) = cluster {
            if start {
                plan.clusters.push(ClusterInfo {
                    name: hot[ci].name.clone(),
                    sections: 0,
                    va: code_page_va(0) + cur,
                    bytes: 0,
                });
            }
            if let Some(c) = plan.clusters.last_mut() {
                c.sections += 1;
                c.bytes = code_page_va(0) + cur + s.size - c.va;
            }
        }
        prev = cluster;
        cur += s.size;
    }
    plan.code_size = cur;
    plan.ncode = cur.div_ceil(PAGE_SIZE);
    if plan.ncode > MAX_CODE_PAGES {
        bail!(
            "paged code needs {} pages ({} B), the code window holds {MAX_CODE_PAGES} ({} B)",
            plan.ncode,
            cur,
            MAX_CODE_PAGES * PAGE_SIZE
        );
    }
    for (n, _) in order(paged(Kind::Rodata)) {
        let s = &g.nodes[n];
        let fit = plan.groups.iter().position(|(_, used)| align_up(*used, s.align) + s.size <= RO_GROUP_SIZE);
        let gi = match fit {
            Some(i) => i,
            None => {
                plan.groups.push((Vec::new(), 0));
                plan.groups.len() - 1
            }
        };
        let (secs, used) = &mut plan.groups[gi];
        let off = align_up(*used, s.align);
        secs.push((n, off));
        *used = off + s.size;
    }
    // Cold code: groups of its own after the constants' (whole pages of
    // executable code), first fit in crate and link order. Of the groups
    // with room, the first where the section shares no physical line with
    // a constant it refers to (such a read would bounce, 02 §3.6).
    let mut holes: Vec<Vec<(u32, u32)>> = Vec::new();
    if share
        && !cold.is_empty()
        && let Some((i, _)) = plan.groups.iter().enumerate().min_by_key(|(_, (_, used))| *used)
    {
        let last = plan.groups.len() - 1;
        plan.groups.swap(i, last);
        if let Some(gr) = plan.groups.pop() {
            plan.shared = gr.1;
            plan.cold.push(gr);
            holes.push(Vec::new());
        }
    }
    let ro_at = plan.addresses();
    let data_lines = |n: usize| -> u32 {
        g.edges[n]
            .iter()
            .filter(|&&m| g.kinds[m] == Kind::Rodata && !merged(&g.nodes[m]))
            .filter_map(|&m| ro_at.get(&m).map(|&va| (va, g.nodes[m].size)))
            .fold(0u32, |acc, (va, size)| {
                let (a, b) = (ro_line(va), ro_line(va + size.max(1) - 1));
                (a..=b).fold(acc, |x, l| x | 1 << l)
            })
    };
    let lines_at = |off: u32, size: u32| -> u32 {
        (off / LINE_SIZE..=(off + size.max(1) - 1) / LINE_SIZE).fold(0u32, |x, l| x | 1 << l)
    };
    let mut cold_order: Vec<usize> = cold.iter().copied().collect();
    cold_order.sort_by(|&a, &b| crates[a].cmp(&crates[b]).then(g.nodes[a].addr.cmp(&g.nodes[b].addr)));
    // The padding left before a section moved to a clear line, per cold
    // group: (start, end) of each hole (`holes`, above). Later sections
    // fill them first (BOOK-1/2, 2026-11-27: with Books the holes were
    // 32 KB of the 23 cold pages, and the code window had to take the
    // rest).
    for n in cold_order {
        let s = &g.nodes[n];
        let want = data_lines(n);
        let fits = |off: u32| off + s.size <= RO_GROUP_SIZE;
        let clear = |off: u32| lines_at(off, s.size) & want == 0;
        // A hole with a clear place: its start, or a line start inside it.
        let in_hole = |&(a, b): &(u32, u32)| -> Option<u32> {
            let first = align_up(a, s.align);
            std::iter::once(first)
                .chain((first / LINE_SIZE + 1..=b / LINE_SIZE).map(|l| l * LINE_SIZE))
                .find(|&off| off + s.size <= b && clear(off))
        };
        if let Some((gi, hi, off)) = holes.iter().enumerate().find_map(|(gi, hs)| {
            hs.iter().enumerate().find_map(|(hi, h)| in_hole(h).map(|off| (gi, hi, off)))
        }) {
            let (a, b) = holes[gi][hi];
            holes[gi].remove(hi);
            for h in [(a, off), (off + s.size, b)] {
                if h.1 > h.0 {
                    holes[gi].push(h);
                }
            }
            plan.cold[gi].0.push((n, off));
            continue;
        }
        // Where in a group with `used` bytes: at its end, or (PG-2b) at the
        // first later line start clear of the constants' lines.
        let spot = |used: u32| -> Option<u32> {
            let end = align_up(used, s.align);
            if !fits(end) {
                return None;
            }
            if clear(end) {
                return Some(end);
            }
            (end / LINE_SIZE + 1..RO_GROUP_SIZE / LINE_SIZE)
                .map(|l| l * LINE_SIZE)
                .find(|&off| fits(off) && clear(off))
        };
        // The first group with a clear place at its end, then the first
        // with one after some padding, then a new group (if the window has
        // room for it), and last the first that fits (a conflict).
        let at_end = plan
            .cold
            .iter()
            .position(|(_, used)| fits(align_up(*used, s.align)) && clear(align_up(*used, s.align)));
        let padded = || plan.cold.iter().position(|(_, used)| spot(*used).is_some());
        let room = (plan.groups.len() + plan.cold.len()) < MAX_RO_GROUPS as usize;
        let pick = match at_end.or_else(padded) {
            Some(i) => Some(i),
            None if room && spot(0).is_some() => None,
            None => plan.cold.iter().position(|(_, used)| fits(align_up(*used, s.align))),
        };
        let gi = match pick {
            Some(i) => i,
            None => {
                plan.cold.push((Vec::new(), 0));
                holes.push(Vec::new());
                plan.cold.len() - 1
            }
        };
        let (secs, used) = &mut plan.cold[gi];
        let off = spot(*used).unwrap_or(align_up(*used, s.align));
        if lines_at(off, s.size) & want != 0 {
            plan.cold_conflicts += 1;
            plan.conflicted.push(n);
        }
        if off > *used {
            holes[gi].push((*used, off));
        }
        secs.push((n, off));
        *used = off + s.size;
    }
    // The linker script places a group's sections in order (`. = addr`
    // never moves back).
    for (secs, _) in &mut plan.cold {
        secs.sort_by_key(|&(_, off)| off);
    }
    // (The rodata window's limit: `check_rodata`, after `place_cold` has
    // brought back what the code window has room for.)
    let ngroups = (plan.groups.len() + plan.cold.len()) as u32;
    plan.nro = match plan.all_groups().last() {
        Some((_, used)) => 2 * (ngroups - 1) + used.div_ceil(PAGE_SIZE),
        None => 0,
    };
    plan.nro_code = if plan.cold.is_empty() { 0 } else { plan.nro - 2 * plan.groups.len() as u32 };
    Ok(plan)
}

/// The pass-2 linker script.
pub fn script(g: &Graph, plan: &Plan, script_dir: &Path, bin: &str) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "/* Generated by `cargo xtask pagelink` for {bin}; do not edit. */");
    let _ = writeln!(s, "INCLUDE \"{}\"", script_dir.join("memory.ld").display());
    let _ = writeln!(s, "__orpheus_code_pages = {};", plan.ncode);
    let _ = writeln!(s, "__orpheus_rodata_pages = {};", plan.nro);
    let _ = writeln!(s, "__orpheus_rodata_code_pages = {};", plan.nro_code);
    s.push_str("SECTIONS\n{\n");
    if !plan.code.is_empty() {
        let _ = writeln!(s, "    {CODE_OUT} {:#010x} :\n    {{", code_page_va(0));
        let mut end = 0;
        for &(n, off, start) in &plan.code {
            if start {
                let _ = writeln!(s, "        . = ALIGN({LINE_SIZE:#x});");
                let pad = off - align_up(end, LINE_SIZE);
                if pad > 0 {
                    // `starts_over`: the code window left empty up to here.
                    let _ = writeln!(s, "        . += {pad:#x};");
                }
            }
            let _ = writeln!(s, "        *(\"{}\")", g.nodes[n].name);
            end = off + g.nodes[n].size;
        }
        s.push_str("    } > PAGED_CODE\n");
    }
    for (gi, (secs, _)) in plan.all_groups().enumerate() {
        let base = ro_page_va(2 * gi as u32);
        let shared = plan.is_shared(gi);
        if shared {
            // The constants of a shared group: an output section of their
            // own, so the pass-2 graph sees them as constants.
            let _ = writeln!(s, "    .pr_g{gi} {base:#010x} :\n    {{");
            for &(n, _) in secs.iter().filter(|&&(n, _)| g.kinds[n] != Kind::Code) {
                let _ = writeln!(s, "        *(\"{}\")", g.nodes[n].name);
            }
            s.push_str("    } > PAGED_RO\n");
        }
        let code_at = secs.iter().find(|&&(n, _)| g.kinds[n] == Kind::Code).map_or(0, |&(_, o)| o);
        let start = if shared { base + code_at } else { base };
        let _ = writeln!(s, "    {} {start:#010x} :\n    {{", plan.group_name(gi));
        for &(n, off) in secs {
            if shared && g.kinds[n] != Kind::Code {
                continue;
            }
            // Cold code keeps the offset the plan gave it (it may have
            // skipped lines of a constant it uses).
            if gi >= plan.groups.len() {
                let _ = writeln!(s, "        . = {:#010x};", base + off);
            }
            let _ = writeln!(s, "        *(\"{}\")", g.nodes[n].name);
        }
        s.push_str("    } > PAGED_RO\n");
    }
    s.push_str("}\n");
    let _ = writeln!(s, "INCLUDE \"{}\"", script_dir.join("layout.ld").display());
    if !plan.code.is_empty() {
        let _ = writeln!(
            s,
            "ASSERT(SIZEOF({CODE_OUT}) == {:#x}, \"pagelink: {CODE_OUT} differs from the plan\");",
            plan.code_size
        );
    }
    for (gi, (secs, used)) in plan.all_groups().enumerate() {
        let name = plan.group_name(gi);
        let mut used = *used;
        if plan.is_shared(gi) {
            let _ = writeln!(
                s,
                "ASSERT(SIZEOF(.pr_g{gi}) == {:#x}, \"pagelink: .pr_g{gi} differs from the plan\");",
                plan.shared
            );
            used -= secs.iter().find(|&&(n, _)| g.kinds[n] == Kind::Code).map_or(0, |&(_, o)| o);
        }
        let _ =
            writeln!(s, "ASSERT(SIZEOF({name}) == {used:#x}, \"pagelink: {name} differs from the plan\");");
    }
    s
}

/// Contents of the final ELF by role.
#[derive(Default)]
pub struct Extracted {
    pub resident: Vec<u8>,
    pub entry: u32,
    pub code: Vec<u8>,
    pub groups: BTreeMap<u32, Vec<u8>>,
    pub text: u32,
    pub rodata: u32,
    pub data: u32,
    pub bss: u32,
}

pub fn extract(elf: &Path) -> Result<Extracted> {
    let bytes = std::fs::read(elf).with_context(|| format!("read {}", elf.display()))?;
    let f = object::File::parse(&*bytes)?;
    let mut x = Extracted { entry: u32::try_from(f.entry())?, ..Default::default() };
    let mut chunks: Vec<(u32, Vec<u8>)> = Vec::new();
    for sec in f.sections() {
        let addr = u32::try_from(sec.address())?;
        let size = u32::try_from(sec.size())?;
        let name = sec.name().unwrap_or_default();
        if addr == 0 || size == 0 {
            continue;
        }
        let nobits = matches!(sec.kind(), SectionKind::UninitializedData | SectionKind::UninitializedTls);
        if SRAM0.contains(&addr) {
            if nobits {
                if name != ".stack" {
                    x.bss += size;
                }
                continue;
            }
            match name {
                ".text" => x.text += size,
                ".rodata" => x.rodata += size,
                _ => x.data += size,
            }
            chunks.push((addr, sec.data()?.to_vec()));
        } else if name == CODE_OUT {
            x.code = sec.data()?.to_vec();
        } else if let Some(gi) = name.strip_prefix(".pr_g").or_else(|| name.strip_prefix(".px_g")) {
            // A shared group (`pack_cold_sharing`) comes as two sections,
            // its constants and its code: each at its offset in the group.
            let gi: u32 = gi.parse().context("group index")?;
            let off = addr.checked_sub(ro_page_va(2 * gi)).context("group section below its group")? as usize;
            let data = sec.data()?;
            let v = x.groups.entry(gi).or_default();
            if v.len() < off + data.len() {
                v.resize(off + data.len(), 0xFF);
            }
            v[off..off + data.len()].copy_from_slice(data);
        } else if !nobits
            && matches!(sec.kind(), SectionKind::Text | SectionKind::Data | SectionKind::ReadOnlyData)
        {
            bail!("{name} at {addr:#x} has contents outside SRAM0 and the page windows");
        }
    }
    let end = chunks.iter().map(|(a, d)| a + d.len() as u32).max().unwrap_or(SRAM0.start);
    let mut img = vec![0u8; (end - SRAM0.start) as usize];
    for (a, d) in chunks {
        let o = (a - SRAM0.start) as usize;
        img[o..o + d.len()].copy_from_slice(&d);
    }
    x.resident = img;
    Ok(x)
}

/// Page store: code pages, then rodata pages; unused bytes 0xFF.
pub fn page_store(x: &Extracted, ncode: u32, nro: u32) -> Result<Vec<u8>> {
    let mut store = vec![0xFFu8; ((ncode + nro) * PAGE_SIZE) as usize];
    if x.code.len() > (ncode * PAGE_SIZE) as usize {
        bail!("{CODE_OUT} is {} bytes, more than {ncode} pages", x.code.len());
    }
    store[..x.code.len()].copy_from_slice(&x.code);
    for (&gi, data) in &x.groups {
        let off = ((ncode + 2 * gi) * PAGE_SIZE) as usize;
        let end = off + data.len();
        if data.len() > RO_GROUP_SIZE as usize || end > store.len() {
            bail!(".pr_g{gi} does not fit its pages");
        }
        store[off..end].copy_from_slice(data);
    }
    Ok(store)
}

/// The `"YFW1"` slot image: header sector, resident image, page store
/// (sector aligned).
pub fn slot_image(
    resident: &[u8],
    entry: u32,
    store: &[u8],
    (ncode, nro, nro_code): (u32, u32, u32),
    layout: u32,
) -> (Vec<u8>, SlotHeader) {
    let resident_off = SECTOR;
    let pages_off = align_up(resident_off + resident.len() as u32, SECTOR);
    let mut id = Sha256::new();
    id.update(resident);
    id.update(store);
    let digest = id.finalize();
    let mut build_id = [0u8; 16];
    build_id.copy_from_slice(&digest[..16]);
    let h = SlotHeader {
        layout,
        resident_off,
        resident_len: resident.len() as u32,
        load_addr: SRAM0.start,
        entry,
        resident_sum: sum32(resident),
        pages_off,
        code_pages: ncode,
        rodata_pages: nro,
        pages_crc: crc32(store),
        res_off: 0,
        res_len: 0,
        verified: 0xFFFF_FFFF,
        build_id,
        rodata_code_pages: nro_code,
    };
    let mut img = vec![0xFFu8; pages_off as usize + store.len()];
    img[..orpheus_proto::slot::HEADER_SIZE].copy_from_slice(&h.to_bytes());
    img[resident_off as usize..resident_off as usize + resident.len()].copy_from_slice(resident);
    img[pages_off as usize..].copy_from_slice(store);
    (img, h)
}

fn hex_id(b: &[u8]) -> String {
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

/// Run pagelink on an already built binary.
pub fn run(o: &Options) -> Result<Layout> {
    let name = o.name.clone().unwrap_or_else(|| o.bin.clone());
    let a = artefacts(&name);
    let saved = SavedLink::load(o.release, &o.bin)?;
    let script_dir = saved.script_dir()?;

    // Pass 1.
    saved.link(None, &a.flat_elf, &a.flat_map)?;
    let map1 = linkmap::parse(&std::fs::read_to_string(&a.flat_map)?)?;
    let g1 = Graph::load(&a.flat_elf, map1)?;
    let mut rules = Rules::load(&root().join("resident.toml"), &o.bin)?;
    rules.paged.symbols.extend(o.force_paged.iter().cloned());
    let hot = rules::load_hot(&root().join("hot.toml"), &o.bin)?;
    let roots = resolve_roots(&g1, &rules);
    let (mut places, mut warnings) = classify(&g1, &rules, &roots);
    let (nc, nc_bytes) = cluster_constants(&g1, &mut places, &hot);
    if nc > 0 {
        log(&format!(
            "  {nc} small constants of clusters' code resident ({nc_bytes} B, hot.toml resident_constants)"
        ));
    }
    let big = oversized(&g1, &places);
    if !big.is_empty() {
        bail!(
            "rodata larger than a group would be resident; split it or name it in resident.toml:\n  {}",
            big.join("\n  ")
        );
    }
    for (p, nodes) in &roots {
        if nodes.is_empty() {
            warnings.push(format!("root `{p}` matches nothing"));
        }
    }
    if !g1.unresolved.is_empty() {
        warnings.push(format!(
            "{} relocations not decoded (first: {})",
            g1.unresolved.len(),
            g1.unresolved[0]
        ));
    }
    let crates1 = crates(&g1);
    let (cold_all, cold_warnings) = cold_set(&g1, &places, &rules, &hot);
    warnings.extend(cold_warnings);
    let (plan, cold) = place_cold(&g1, &places, &hot, &crates1, &rules, &cold_all)?;
    warnings.extend(fit_warnings(&hot, &plan.clusters));

    // Pass 2.
    std::fs::write(&a.script, script(&g1, &plan, &script_dir, &o.bin))?;
    saved.link(Some(&a.script), &a.elf, &a.map)?;
    let map2 = linkmap::parse(&std::fs::read_to_string(&a.map)?)?;
    let g2 = Graph::load(&a.elf, map2)?;

    // The final placement must be the plan.
    let planned = plan.addresses();
    let names2 = g2.unique_names();
    let mut paged_names = BTreeSet::new();
    for (&n, &va) in &planned {
        let sec = &g1.nodes[n].name;
        paged_names.insert(sec.as_str());
        match names2.get(sec.as_str()) {
            Some(&m) if g2.nodes[m].addr == va => {}
            Some(&m) => bail!("{sec}: planned at {va:#x}, linked at {:#x}", g2.nodes[m].addr),
            None => bail!("{sec}: missing from the final link"),
        }
    }
    for s in &g2.nodes {
        let in_window = orpheus_pager::Page::of(s.addr).is_some();
        if in_window != paged_names.contains(s.name.as_str()) {
            bail!("{} at {:#x}: placement differs from the plan", s.name, s.addr);
        }
    }

    // Every load of cold code must be one the trampoline can run.
    let (mut cold_loads, mut unbounceable) = (0u32, Vec::new());
    for m in 0..g2.nodes.len() {
        if g2.kinds[m] == Kind::Code
            && matches!(orpheus_pager::Page::of(g2.nodes[m].addr), Some(orpheus_pager::Page::Rodata(_)))
        {
            let (n, refused) = g2.bounce_audit(m);
            cold_loads += n;
            unbounceable.extend(refused.into_iter().map(|(at, why)| (at, why, g2.label(m))));
        }
    }
    if !unbounceable.is_empty() {
        let first: Vec<String> =
            unbounceable.iter().take(4).map(|(at, why, f)| format!("{at:#x} in {f} ({why:?})")).collect();
        warnings.push(format!(
            "{} of {cold_loads} loads in cold code cannot be bounced (fatal if their constant shares their line): {}",
            unbounceable.len(),
            first.join(", ")
        ));
    }

    // Images.
    let x = extract(&a.elf)?;
    let store = page_store(&x, plan.ncode, plan.nro)?;
    let (slot, header) =
        slot_image(&x.resident, x.entry, &store, (plan.ncode, plan.nro, plan.nro_code), o.layout);
    std::fs::write(&a.resident, &x.resident)?;
    std::fs::write(&a.pages, &store)?;
    std::fs::write(&a.yfw, &slot)?;
    // The update file (UPD-1) of a full-layout firmware build.
    if name == "firmware"
        && let Some(p) = crate::ofw::write_for(&a.yfw)?
    {
        crate::util::log(&format!("update file {}", p.display()));
    }

    // layout.json.
    let reasons: BTreeMap<&str, &Place> =
        g1.nodes.iter().zip(&places).map(|(s, p)| (s.name.as_str(), p)).collect();
    let cold_names: BTreeSet<&str> = cold.iter().map(|&n| g1.nodes[n].name.as_str()).collect();
    let shared: BTreeSet<&str> = plan.conflicted.iter().map(|&n| g1.nodes[n].name.as_str()).collect();
    let crates2 = crates(&g2);
    let sections = (0..g2.nodes.len())
        .map(|m| {
            let s = &g2.nodes[m];
            let kind = g2.kinds[m];
            let (placement, reason) = match reasons.get(s.name.as_str()) {
                Some(Place::Paged(r)) if cold_names.contains(s.name.as_str()) => {
                    // `cold-shared:`: on a physical line of a constant it reads.
                    let tag = if shared.contains(s.name.as_str()) { "cold-shared" } else { "cold" };
                    ("paged", format!("{tag}:{r}"))
                }
                Some(Place::Paged(r)) => ("paged", r.clone()),
                Some(Place::Resident(r)) => ("resident", r.clone()),
                Some(Place::Ram) | None if kind == Kind::Ram => ("ram", String::new()),
                _ => ("resident", "fixed:not in pass 1".into()),
            };
            let (page, store_offset) = match orpheus_pager::Page::of(s.addr) {
                Some(p @ orpheus_pager::Page::Code(n)) => {
                    (Some(u32::from(n)), Some(p.store_index(plan.ncode) * PAGE_SIZE + (s.addr - p.va())))
                }
                Some(p @ orpheus_pager::Page::Rodata(n)) => {
                    (Some(u32::from(n)), Some(p.store_index(plan.ncode) * PAGE_SIZE + (s.addr - p.va())))
                }
                None => (None, None),
            };
            SectionInfo {
                section: s.name.clone(),
                symbol: g2.label(m),
                krate: crates2[m].clone(),
                kind: kind.name().into(),
                placement: placement.into(),
                reason,
                addr: s.addr,
                size: s.size,
                page,
                store_offset,
            }
        })
        .collect();
    let mut groups = Vec::new();
    if !plan.code.is_empty() {
        groups.push(GroupInfo {
            name: CODE_OUT.into(),
            kind: "code".into(),
            va: code_page_va(0),
            size: plan.code_size,
            capacity: MAX_CODE_PAGES * PAGE_SIZE,
            first_page: 0,
            pages: plan.ncode,
        });
    }
    for (gi, (_, used)) in plan.all_groups().enumerate() {
        groups.push(GroupInfo {
            name: plan.group_name(gi),
            kind: if gi < plan.groups.len() { "rodata".into() } else { "cold".into() },
            va: ro_page_va(2 * gi as u32),
            size: *used,
            capacity: RO_GROUP_SIZE,
            first_page: 2 * gi as u32,
            pages: used.div_ceil(PAGE_SIZE),
        });
    }
    let code_bytes = plan.code.iter().map(|&(n, _, _)| g1.nodes[n].size).sum();
    let rodata_bytes = plan.groups.iter().flat_map(|(s, _)| s).map(|&(n, _)| g1.nodes[n].size).sum();
    let cold_bytes = plan.cold.iter().flat_map(|(s, _)| s).map(|&(n, _)| g1.nodes[n].size).sum();
    let layout = Layout {
        format: 1,
        bin: o.bin.clone(),
        elf: a.elf.display().to_string(),
        map: a.map.display().to_string(),
        build_id: hex_id(&header.build_id),
        slot: SlotInfo {
            layout: if o.layout == LAYOUT_FULL { "full".into() } else { "dual".into() },
            bytes: slot.len() as u32,
            resident_off: header.resident_off,
            resident_len: header.resident_len,
            resident_sum: header.resident_sum,
            pages_off: header.pages_off,
            pages_len: store.len() as u32,
            pages_crc: header.pages_crc,
        },
        resident: ResidentInfo {
            load_addr: header.load_addr,
            entry: header.entry,
            image_bytes: x.resident.len() as u32,
            text: x.text,
            rodata: x.rodata,
            data: x.data,
            bss: x.bss,
        },
        pages: PagesInfo {
            code: plan.ncode,
            rodata: plan.nro,
            code_max: MAX_CODE_PAGES,
            rodata_max: MAX_RO_PAGES,
            code_bytes,
            rodata_bytes,
            code_padding: plan.padding,
            code_filler: plan.filler,
            cold: plan.nro_code,
            cold_bytes,
            cold_conflicts: plan.cold_conflicts,
            cold_loads,
            cold_unbounceable: unbounceable.len() as u32,
        },
        groups,
        clusters: plan.clusters.clone(),
        roots: roots
            .iter()
            .map(|(p, ns)| RootInfo {
                pattern: p.clone(),
                matched: ns.iter().map(|&n| g1.label(n)).collect(),
            })
            .collect(),
        sections,
        warnings,
    };
    std::fs::write(&a.layout, serde_json::to_string_pretty(&layout)? + "\n")?;
    Ok(layout)
}

pub fn print_summary(l: &Layout) {
    log(&format!(
        "{}: resident {} B (text {} rodata {} data {}), bss {} B",
        l.bin, l.resident.image_bytes, l.resident.text, l.resident.rodata, l.resident.data, l.resident.bss
    ));
    log(&format!(
        "  pages: code {}/{} ({} B paged), rodata {}/{} ({} B paged); slot {} B ({})",
        l.pages.code,
        l.pages.code_max,
        l.pages.code_bytes,
        l.pages.rodata,
        l.pages.rodata_max,
        l.pages.rodata_bytes,
        l.slot.bytes,
        l.slot.layout
    ));
    if l.pages.code_padding > 0 {
        log(&format!("  code window left empty before starts_over clusters: {} B", l.pages.code_padding));
    }
    let window = l.pages.code_max * PAGE_SIZE;
    let used = l.groups.iter().find(|g| g.kind == "code").map_or(0, |g| g.size);
    log(&format!(
        "  code window: {} B free ({} B in the line before cluster starts); a page is {PAGE_SIZE} B",
        window.saturating_sub(used),
        l.pages.code_filler
    ));
    if l.pages.cold > 0 {
        log(&format!(
            "  cold code in the rodata window: {} pages ({} B, {} sections share a line with a constant they use; {} of {} loads cannot be bounced)",
            l.pages.cold,
            l.pages.cold_bytes,
            l.pages.cold_conflicts,
            l.pages.cold_unbounceable,
            l.pages.cold_loads
        ));
    }
    let closure: Vec<&SectionInfo> = l.sections.iter().filter(|s| s.reason.starts_with("closure:")).collect();
    log(&format!("  {} sections resident through the closure of {} roots", closure.len(), l.roots.len()));
    for c in &l.clusters {
        log(&format!(
            "  cluster {}: {} sections, {} B ({} lines) at {:#x}",
            c.name,
            c.sections,
            c.bytes,
            c.bytes.div_ceil(LINE_SIZE),
            c.va
        ));
    }
    for w in &l.warnings {
        log(&format!("  warning: {w}"));
    }
}

pub fn cmd(args: &mut Args) -> Result<String> {
    let release = !args.flag("debug");
    let no_build = args.flag("no-build");
    let bin = args.value("bin").unwrap_or_else(|| "firmware".into());
    let layout = match args.value("layout").as_deref() {
        // The player runs the full layout since 2026-11-24 (D-2).
        Some("dual") => LAYOUT_DUAL,
        None | Some("full") => LAYOUT_FULL,
        Some(o) => bail!("unknown layout `{o}` (dual, full)"),
    };
    let force_paged: Vec<String> =
        args.value("paged").map(|p| p.split(',').map(str::to_string).collect()).unwrap_or_default();
    let features = args.value("features");
    if !no_build {
        let pkg = if bin == "firmware" { "firmware".to_string() } else { bin.clone() };
        crate::build::cargo_build(&pkg, &bin, release, features.as_deref())?;
    }
    let l = run(&Options { bin, release, layout, force_paged, name: None })?;
    print_summary(&l);
    Ok(format!(
        "ok, resident {} B, {} code + {} rodata pages, slot {} B",
        l.resident.image_bytes, l.pages.code, l.pages.rodata, l.slot.bytes
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linkmap::{InputSection, MapSymbol};
    use orpheus_proto::slot::HEADER_SIZE;

    fn node(out: &str, name: &str, size: u32, align: u32, sym: Option<&str>) -> InputSection {
        InputSection {
            out: out.into(),
            file: "/x/a.o".into(),
            name: name.into(),
            addr: 0,
            size,
            align,
            symbols: sym.map(|s| MapSymbol { addr: 1, size, name: s.into() }).into_iter().collect(),
        }
    }

    /// root -> helper -> anon rodata; cold code; big rodata; mergeable;
    /// rule-resident; forced-paged but reachable; RAM.
    fn sample() -> Graph {
        Graph::from_parts(
            vec![
                node(".text", ".text.root", 100, 4, Some("root_fn")),
                node(".text", ".text.helper", 60, 2, Some("app::helper")),
                node(".rodata", ".rodata..Lanon.1", 32, 4, None),
                node(".text", ".text.cold", 3000, 2, Some("app::cold")),
                node(".rodata", ".rodata.big", 20000, 4, Some("app::BIG")),
                node(".rodata", ".rodata.str1.1", 40, 1, None),
                node(".text", ".text.ruled", 10, 2, Some("orpheus_pager::x")),
                node(".text", ".text.planted", 10, 2, Some("app::planted")),
                node(".bss", ".bss.state", 64, 4, Some("app::STATE")),
                node(".text", ".text.hot_b", 500, 2, Some("app::hot_b")),
                node(".text", ".text.hot_a", 700, 8, Some("app::hot_a")),
                node(".rodata", ".rodata.t1", 9000, 4, Some("app::T1")),
                node(".rodata", ".rodata.t2", 9000, 4, Some("app::T2")),
                node(".rodata", ".rodata.t3", 6000, 4, Some("app::T3")),
            ],
            &[(0, 1), (1, 2), (0, 7), (0, 8), (8, 3)],
        )
    }

    fn rules() -> Rules {
        Rules::parse(
            r#"
roots = ["root_fn"]
[resident]
symbols = ["orpheus_pager::*"]
[paged]
symbols = ["*planted"]
"#,
            "t",
        )
        .unwrap()
    }

    fn internal(name: &str, size: u32) -> InputSection {
        InputSection { file: "<internal>".into(), ..node(".rodata", name, size, 1, None) }
    }

    /// Bug 2026-11-26 (resident image 81,875 of 81,920 B): the UI font,
    /// one 17,794 B anonymous rodata section (`orpheus_ui::font::UI`'s
    /// data), was larger than a 16 KiB rodata group, so pagelink kept it
    /// resident behind a warning printed on every build, and the budget's
    /// note blamed the MP3 tables. Now an oversized section stops the
    /// link, and the font linked in two pieces (16,381 + 1,413 B) is paged
    /// in two groups.
    #[test]
    fn regress_oversized_ui_font_was_resident() {
        let r = rules();
        let whole = Graph::from_parts(
            vec![
                node(".text", ".text.draw", 400, 2, Some("orpheus_ui::gfx::Gfx::text")),
                node(".rodata", ".rodata.ui", 16, 4, Some("orpheus_ui::font::UI")),
                node(".rodata", ".rodata..Lanon.b54b.52", 17_794, 1, None),
            ],
            &[(0, 1), (1, 2)],
        );
        let (p, w) = classify(&whole, &r, &resolve_roots(&whole, &r));
        assert_eq!(p[2], Place::Resident(OVERSIZED.into()));
        assert!(w[0].contains("17794 bytes"), "{w:?}");
        let big = oversized(&whole, &p);
        assert_eq!(big.len(), 1, "{big:?}");
        assert!(big[0].contains("17794 B, used by orpheus_ui::font::UI"), "{big:?}");

        let split = Graph::from_parts(
            vec![
                node(".text", ".text.draw", 400, 2, Some("orpheus_ui::gfx::Gfx::text")),
                node(".rodata", ".rodata.ui", 16, 4, Some("orpheus_ui::font::UI")),
                node(".rodata", ".rodata.head", 16_381, 1, Some("orpheus_ui::font::UI::HEAD")),
                node(".rodata", ".rodata.tail", 1_413, 1, Some("orpheus_ui::font::UI::TAIL")),
            ],
            &[(0, 1), (1, 2), (1, 3)],
        );
        let (p, _) = classify(&split, &r, &resolve_roots(&split, &r));
        assert!(oversized(&split, &p).is_empty());
        assert!(p[2..].iter().all(|x| *x == Place::Paged("default".into())), "{p:?}");
        let plan = pack(&split, &p, &[], &crates(&split)).unwrap();
        let groups: Vec<Vec<usize>> =
            plan.groups.iter().map(|(s, _)| s.iter().map(|&(n, _)| n).collect()).collect();
        // The 16 B table and the tail share the first group, the head
        // fills the second.
        assert_eq!(groups, vec![vec![1, 3], vec![2]]);
        assert_eq!(plan.nro, 4);
    }

    /// Merged sections: paged when only paged sections use them; resident
    /// by rule, through a root, or when resident code uses them.
    #[test]
    fn merged_sections_paged_when_only_paged_code_uses_them() {
        let g = Graph::from_parts(
            vec![
                node(".text", ".text.root", 100, 4, Some("root_fn")),
                node(".text", ".text.cold", 300, 2, Some("app::cold")),
                node(".rodata", ".rodata.loc", 16, 4, None),
                internal(".rodata.str1.1", 8042),
                internal(".rodata.str2.2", 64),
                internal(".rodata.cst8", 1000),
                internal(".rodata.cst4", 692),
                node(".text", ".text.ruled", 10, 2, Some("orpheus_pager::x")),
                internal(".rodata.cst16", 1440),
                node(".rodata", ".rodata.str1.4", 12, 4, None),
            ],
            // cold -> Location -> strings; root -> str2.2; cold -> cst8,
            // cst4; the resident pager code -> cst16 (no root reaches it).
            &[(1, 2), (2, 3), (0, 4), (1, 5), (1, 6), (7, 8), (1, 9)],
        );
        let r = Rules::parse(
            r#"
roots = ["root_fn"]
[resident]
symbols = ["orpheus_pager::*"]
sections = [".rodata.cst4"]
"#,
            "t",
        )
        .unwrap();
        let (p, w) = classify(&g, &r, &resolve_roots(&g, &r));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(p[3], Place::Paged("merged: only paged sections use it".into()));
        assert_eq!(p[4], Place::Resident("closure:root_fn -> .rodata.str2.2".into()));
        assert_eq!(p[5], Place::Paged("merged: only paged sections use it".into()));
        assert_eq!(p[6], Place::Resident("rule:section:.rodata.cst4".into()));
        assert_eq!(p[8], Place::Resident("fixed:merged, used by orpheus_pager::x (not paged)".into()));
        // Mergeable input the linker did not merge cannot move.
        assert_eq!(p[9], Place::Resident("fixed:mergeable".into()));
        let plan = pack(&g, &p, &[], &crates(&g)).unwrap();
        let s = script(&g, &plan, Path::new("/l"), "t");
        assert!(s.contains(".pr_g0 0x00300000 :\n    {\n"), "{s}");
        assert!(s.contains("*(\".rodata.str1.1\")"), "{s}");
        assert!(!s.contains("*(\".rodata.cst4\")"), "{s}");
    }

    /// The line before a cluster's first line is filled with code from no
    /// cluster (largest first), the clusters stay line-aligned and their
    /// extents unchanged, and the rest follows in its order.
    #[test]
    fn code_before_a_cluster_start_fills_the_line() {
        let g = Graph::from_parts(
            vec![
                node(".text", ".text.a1", 700, 4, Some("app::a1")),
                node(".text", ".text.b1", 900, 4, Some("app::b1")),
                node(".text", ".text.c200", 200, 2, Some("app::c200")),
                node(".text", ".text.c300", 300, 4, Some("app::c300")),
                node(".text", ".text.c2000", 2000, 2, Some("app::c2000")),
                node(".text", ".text.c30", 30, 8, Some("app::c30")),
            ],
            &[],
        );
        let r = Rules::parse("roots = []\n", "t").unwrap();
        let (p, _) = classify(&g, &r, &[]);
        let cl = |name: &str, sym: &str| Cluster {
            name: name.into(),
            symbols: vec![sym.into()],
            ..Cluster::default()
        };
        let hot = vec![cl("a", "app::a1"), cl("b", "app::b1")];
        let plan = pack(&g, &p, &hot, &crates(&g)).unwrap();
        // a: 0..700; 324 B left: c300 (700..1000), then c200 does not fit
        // (24 B left) nor c30 at 1000 (8-aligned 1000 + 30 > 1024); b at
        // 1024; the rest after b in its order: c200, c2000, c30.
        assert_eq!(
            plan.code,
            vec![
                (0, 0, true),
                (3, 700, false),
                (1, 1024, true),
                (2, 1924, false),
                (4, 2124, false),
                (5, 4128, false)
            ]
        );
        assert_eq!(plan.filler, 300);
        assert_eq!(
            (plan.clusters[0].bytes, plan.clusters[1].va, plan.clusters[1].bytes),
            (700, CODE_WIN + 1024, 900)
        );
        let s = script(&g, &plan, Path::new("/l"), "t");
        assert!(s.contains("*(\".text.a1\")\n        *(\".text.c300\")\n        . = ALIGN(0x400);\n        *(\".text.b1\")"), "{s}");
    }

    #[test]
    fn classification() {
        let g = sample();
        let r = rules();
        let roots = resolve_roots(&g, &r);
        assert_eq!(roots, vec![("root_fn".to_string(), vec![0])]);
        let (p, w) = classify(&g, &r, &roots);
        let reason = |i: usize| match &p[i] {
            Place::Resident(s) | Place::Paged(s) => s.clone(),
            Place::Ram => "ram".into(),
        };
        assert_eq!(reason(0), "closure:root_fn");
        assert_eq!(reason(1), "closure:root_fn -> app::helper");
        assert_eq!(reason(2), "closure:root_fn -> app::helper -> .rodata..Lanon.1");
        // Reached through RAM data (a pointer in a static).
        assert_eq!(reason(3), "closure:root_fn -> app::STATE -> app::cold");
        assert!(matches!(p[3], Place::Resident(_)));
        assert_eq!(reason(4), "fixed:larger than a rodata group");
        assert_eq!(reason(5), "fixed:mergeable");
        assert_eq!(reason(6), "rule:symbol:orpheus_pager::*");
        assert_eq!(p[7], Place::Paged("forced:symbol:*planted".into()));
        assert_eq!(p[8], Place::Ram);
        assert_eq!(p[9], Place::Paged("default".into()));
        assert_eq!(w.len(), 2, "{w:?}");
    }

    /// `fits_with`: clusters that run together must not share a physical
    /// line of the direct-mapped code cache (AU-PERF: the old `play-block`
    /// was 77 lines, and its hot lines 32 KiB apart evicted each other).
    #[test]
    fn regress_clusters_that_run_together_share_no_cache_line() {
        let ci = |name: &str, line: u32, lines: u32| ClusterInfo {
            name: name.into(),
            sections: 1,
            va: CODE_WIN + line * LINE_SIZE,
            bytes: lines * LINE_SIZE - 10,
        };
        let cl = |name: &str, with: &[&str]| Cluster {
            name: name.into(),
            bins: vec![],
            symbols: vec![],
            sections: vec![],
            fits_with: with.iter().map(|s| (*s).to_string()).collect(),
            ..Cluster::default()
        };
        let hot = [
            cl("dec", &["block", "fill"]),
            cl("block", &[]),
            cl("fill", &[]),
            cl("other", &["block", "fill"]),
        ];
        // 11 + 15 + 6 = 32 lines from line 71: every physical line once;
        // `other` 32 lines on lands on `dec`'s lines only.
        let ok = [ci("dec", 71, 11), ci("block", 82, 15), ci("fill", 97, 6), ci("other", 103, 11)];
        assert_eq!(fit_warnings(&hot, &ok), Vec::<String>::new());
        // One line more in `block` pushes `fill` onto `dec`'s first line,
        // and `other` onto `block`'s.
        let bad = [ci("dec", 71, 11), ci("block", 82, 16), ci("fill", 98, 6), ci("other", 104, 11)];
        let w = fit_warnings(&hot, &bad);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[0].contains("dec and fill share 1"), "{w:?}");
        assert!(w[1].contains("other and block share 1"), "{w:?}");
        // A cluster longer than the cache, and a name that is not placed.
        let w = fit_warnings(&[cl("big", &["gone"])], &[ci("big", 0, 40)]);
        assert_eq!(w.len(), 2, "{w:?}");
    }

    #[test]
    fn packing_and_script() {
        let g = sample();
        let r = rules();
        let roots = resolve_roots(&g, &r);
        let (p, _) = classify(&g, &r, &roots);
        let hot = vec![Cluster {
            name: "hot".into(),
            bins: vec![],
            symbols: vec!["app::hot_a".into(), "app::hot_b".into()],
            sections: vec![],
            fits_with: vec![],
            ..Cluster::default()
        }];
        let crates = crates(&g);
        let plan = pack(&g, &p, &hot, &crates).unwrap();
        // Cluster first (pattern order), line-aligned; then the rest.
        let order: Vec<(usize, u32, bool)> = plan.code.clone();
        assert_eq!(order, vec![(10, 0, true), (9, 700, false), (7, 1200, false)]);
        assert_eq!(plan.code_size, 1210);
        assert_eq!(plan.ncode, 1);
        assert_eq!(plan.clusters.len(), 1);
        assert_eq!((plan.clusters[0].sections, plan.clusters[0].bytes), (2, 1200));
        // Rodata first fit: T1 (9000) and T2 cannot share a 16 KiB group, T3
        // fits after T1.
        let groups: Vec<Vec<(usize, u32)>> = plan.groups.iter().map(|(s, _)| s.clone()).collect();
        assert_eq!(groups, vec![vec![(11, 0), (13, 9000)], vec![(12, 0)]]);
        // Both groups use both of their pages (9000 B > 8 KiB): pages 0..=3.
        assert_eq!(plan.nro, 4);
        let s = script(&g, &plan, Path::new("/l"), "t");
        assert!(s.contains("INCLUDE \"/l/memory.ld\""));
        assert!(s.contains("__orpheus_code_pages = 1;"));
        assert!(s.contains("__orpheus_rodata_pages = 4;"));
        assert!(
            s.contains(".pg_code 0x00200000 :\n    {\n        . = ALIGN(0x400);\n        *(\".text.hot_a\")")
        );
        assert!(s.contains(".pr_g1 0x00308000 :"));
        assert!(s.contains("ASSERT(SIZEOF(.pr_g0) == 0x3a98"));
        let addrs = plan.addresses();
        assert_eq!(addrs[&13], 0x30_0000 + 9000);
        assert_eq!(addrs[&12], 0x30_8000);
    }

    /// `starts_over`: the cluster starts on the physical line of the one it
    /// names (one alias block later here), the window between left empty;
    /// the script places it there.
    #[test]
    fn starts_over_pads_to_the_named_clusters_line() {
        let g = sample();
        let r = rules();
        let roots = resolve_roots(&g, &r);
        let (p, _) = classify(&g, &r, &roots);
        let cl = |name: &str, sym: &str, over: Option<&str>| Cluster {
            name: name.into(),
            symbols: vec![sym.into()],
            starts_over: over.map(str::to_string),
            ..Cluster::default()
        };
        let hot = vec![cl("a", "app::hot_a", None), cl("b", "app::hot_b", Some("a"))];
        let plan = pack(&g, &p, &hot, &crates(&g)).unwrap();
        // a: line 0 (700 B, then the cold `planted` in the rest of the
        // line); b would start on line 1: 31 lines of padding (not filled).
        assert_eq!(plan.code, vec![(10, 0, true), (7, 700, false), (9, 0x8000, true)]);
        assert_eq!(plan.padding, 31 * LINE_SIZE);
        assert_eq!(phys_lines(&plan.clusters[1]), vec![0]);
        let s = script(&g, &plan, Path::new("/l"), "t");
        assert!(
            s.contains("        . = ALIGN(0x400);\n        . += 0x7c00;\n        *(\".text.hot_b\")"),
            "{s}"
        );
        // Not placed before it: an error.
        let hot = vec![cl("b", "app::hot_b", Some("a")), cl("a", "app::hot_a", None)];
        assert!(pack(&g, &p, &hot, &crates(&g)).is_err());
    }

    #[test]
    fn slot_image_layout() {
        let resident = vec![7u8; 5000];
        let store = vec![1u8; 3 * PAGE_SIZE as usize];
        let (img, h) = slot_image(&resident, 0x10_0001, &store, (2, 1, 0), LAYOUT_DUAL);
        assert_eq!(h.resident_off, 0x1000);
        assert_eq!(h.pages_off, 0x3000);
        assert_eq!(img.len(), 0x3000 + 3 * 0x2000);
        let back = SlotHeader::from_bytes(&img[..HEADER_SIZE]).unwrap();
        assert_eq!(back, h);
        assert_eq!(back.resident_sum, 7 * 5000);
        assert_eq!(back.pages_crc, crc32(&store));
        assert!(img[HEADER_SIZE..0x1000].iter().all(|&b| b == 0xFF));
        assert!(img[0x1000 + 5000..0x3000].iter().all(|&b| b == 0xFF));
        // Deterministic build id.
        assert_eq!(slot_image(&resident, 0x10_0001, &store, (2, 1, 0), LAYOUT_DUAL).1.build_id, h.build_id);
    }

    #[test]
    fn page_store_places_groups() {
        let x = Extracted {
            code: vec![0xAA; 0x2100],
            groups: [(0, vec![0xBB; 0x2004]), (1, vec![0xCC; 4])].into_iter().collect(),
            ..Default::default()
        };
        let s = page_store(&x, 2, 3).unwrap();
        assert_eq!(s.len(), 5 * 0x2000);
        assert_eq!(s[0x20FF], 0xAA);
        assert_eq!(s[0x2100], 0xFF);
        assert_eq!(s[0x4000], 0xBB); // rodata page 0 = store page 2
        assert_eq!(s[0x6003], 0xBB); // rodata page 1
        assert_eq!(s[0x6004], 0xFF);
        assert_eq!(s[0x8000], 0xCC); // rodata page 2 (group 1)
        assert!(page_store(&x, 1, 3).is_err());
    }

    /// Cold code (PG-2): its own groups after the constants', executable
    /// pages counted at the end, placed clear of the lines of a constant
    /// it refers to where a group has room there, else first fit; the
    /// script places each section at the planned address.
    #[test]
    fn cold_code_goes_to_its_own_groups_clear_of_its_constants() {
        let g = Graph::from_parts(
            vec![
                node(".text", ".text.hot", 600, 2, Some("app::hot")),
                node(".text", ".text.cold_a", 3000, 2, Some("books::a")),
                node(".text", ".text.cold_x", 15_000, 2, Some("books::x")),
                node(".text", ".text.cold_b", 900, 4, Some("books::b")),
                node(".rodata", ".rodata.t", 9000, 4, Some("app::T")),
                node(".text", ".text.too_big", 17_000, 2, Some("books::big")),
                node(".text", ".text.cold_c", 600, 4, Some("books::c")),
            ],
            // `cold_b` and `cold_c` read `T` (lines 0–8 of group 0).
            &[(3, 4), (6, 4)],
        );
        let r = Rules::parse("roots = []\n[cold]\nsymbols = [\"books::*\"]\n", "t").unwrap();
        let (p, _) = classify(&g, &r, &[]);
        let (cold, w) = cold_set(&g, &p, &r, &[]);
        assert_eq!(cold.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3, 6]);
        assert!(w[0].contains("books::big (17000 B) is cold but larger than a rodata group"), "{w:?}");
        let plan = pack_cold(&g, &p, &[], &crates(&g), &cold).unwrap();
        assert_eq!(plan.code.iter().map(|&(n, _, _)| n).collect::<Vec<_>>(), vec![0, 5]);
        assert_eq!(plan.groups.len(), 1);
        // `cold_a` at 0 of the first cold group, `cold_x` too large for its
        // rest: a second group. `cold_b` would land on lines 2–3 of the
        // first (T's) but finds line 14 of the second clear; `cold_c` has
        // room at an end only on T's lines (the first group) and goes to
        // the first line after them there (PG-2b), line 9: no conflict.
        assert_eq!(plan.cold, vec![(vec![(1, 0), (6, 9216)], 9816), (vec![(2, 0), (3, 15_000)], 15_900)]);
        assert_eq!(plan.cold_conflicts, 0);
        // Pages: constants 0–1, cold 2 (one page) and 4–5: 6 in all, the
        // last 4 code.
        assert_eq!((plan.nro, plan.nro_code), (6, 4));
        let a = plan.addresses();
        assert_eq!((a[&1], a[&3], a[&6]), (ro_page_va(2), ro_page_va(4) + 15_000, ro_page_va(3) + 1024));
        let s = script(&g, &plan, Path::new("/l"), "t");
        assert!(
            s.contains(".px_g1 0x00308000 :\n    {\n        . = 0x00308000;\n        *(\".text.cold_a\")"),
            "{s}"
        );
        assert!(s.contains("        . = 0x00313a98;\n        *(\".text.cold_b\")"), "{s}");
        assert!(s.contains("__orpheus_rodata_code_pages = 4;"), "{s}");
        assert!(s.contains("ASSERT(SIZEOF(.px_g2) == 0x3e1c"), "{s}");
    }

    /// BOOK-1/2 (2026-11-27): with the rodata window full, cold code that
    /// the code window's reserve page could hold stayed cold and the build
    /// failed ("need 33 groups of 16 KiB, the window holds 32"); the reserve
    /// now gives way.
    #[test]
    fn regress_reserve_page_kept_while_the_rodata_window_overflowed() {
        let names: Vec<String> = (0..MAX_RO_GROUPS - 1).map(|i| format!(".rodata.t{i}")).collect();
        let mut nodes = vec![
            node(".text", ".text.big", 1_035_000, 2, Some("app::big")),
            node(".text", ".text.a", 10_000, 2, Some("books::a")),
            node(".text", ".text.b", 10_000, 2, Some("books::b")),
        ];
        for n in &names {
            nodes.push(node(".rodata", n, RO_GROUP_SIZE, 4, None));
        }
        let g = Graph::from_parts(nodes, &[]);
        let r = Rules::parse("roots = []\n[cold]\nsymbols = [\"books::*\"]\n", "t").unwrap();
        let (p, _) = classify(&g, &r, &[]);
        let (all, _) = cold_set(&g, &p, &r, &[]);
        assert_eq!(all.len(), 2);
        // Kept, the reserve leaves both cold: 31 + 2 groups.
        let (kept, _) = place_cold_reserving(&g, &p, &[], &crates(&g), &r, &all, CODE_RESERVE_PAGES, false).unwrap();
        assert!(check_rodata(&kept).is_err());
        let (plan, cold) = place_cold(&g, &p, &[], &crates(&g), &r, &all).unwrap();
        assert_eq!(cold.len(), 1);
        assert_eq!(plan.code_size, 1_045_000);
        assert!(check_rodata(&plan).is_ok());
    }

    /// BOOK-1/2 (2026-11-27): speech and Books did not fit (33 of the 32
    /// rodata groups, `xtask build --features books`: "paged rodata and cold
    /// code need 33 groups"): the padding PG-2b leaves before a section moved
    /// clear of its constants' lines was never used again (32 KB of 23 cold
    /// pages). Later sections now fill those holes first, and the script
    /// still places each group's sections in address order.
    #[test]
    fn regress_cold_padding_was_never_filled() {
        let g = Graph::from_parts(
            vec![
                node(".text", ".text.hot", 600, 2, Some("app::hot")),
                node(".text", ".text.cold_a", 3000, 2, Some("books::a")),
                node(".rodata", ".rodata.t", 9000, 4, Some("app::T")),
                node(".text", ".text.cold_c", 600, 4, Some("books::c")),
                node(".text", ".text.cold_d", 2000, 4, Some("books::d")),
                node(".text", ".text.cold_e", 7000, 4, Some("books::e")),
            ],
            // `cold_c` reads `T` (lines 0–8): after `cold_a` it moves to
            // line 9, leaving 3000..9216 empty.
            &[(3, 2)],
        );
        let r = Rules::parse("roots = []\n[cold]\nsymbols = [\"books::*\"]\n", "t").unwrap();
        let (p, _) = classify(&g, &r, &[]);
        let (cold, _) = cold_set(&g, &p, &r, &[]);
        let plan = pack_cold(&g, &p, &[], &crates(&g), &cold).unwrap();
        // `cold_d` in the hole after `cold_a`; `cold_e` (7,000 B) fits
        // neither the rest of the hole nor the group's end: a new group.
        assert_eq!(plan.cold[0].0, vec![(1, 0), (4, 3000), (3, 9216)]);
        assert_eq!(plan.cold.len(), 2);
        assert_eq!(plan.cold[1].0, vec![(5, 0)]);
        assert_eq!(plan.cold_conflicts, 0);
        let s = script(&g, &plan, Path::new("/l"), "t");
        let (d, c) = (s.find(".text.cold_d").unwrap(), s.find(".text.cold_c").unwrap());
        assert!(d < c, "{s}");
    }

    /// Only as much cold code leaves the code window as it needs: the
    /// warmest (the last `[cold]` patterns) comes back while a page of the
    /// window stays free.
    #[test]
    fn cold_code_leaves_the_code_window_only_as_needed() {
        let g = Graph::from_parts(
            vec![
                node(".text", ".text.big", 1_030_000, 2, Some("app::big")),
                node(".text", ".text.a", 6000, 2, Some("conv::a")),
                node(".text", ".text.b", 4000, 2, Some("lib::b")),
                node(".text", ".text.c", 3000, 2, Some("dev::c")),
            ],
            &[],
        );
        let r = Rules::parse("roots = []\n[cold]\nsymbols = [\"conv::*\", \"lib::*\", \"dev::*\"]\n", "t")
            .unwrap();
        let (p, _) = classify(&g, &r, &[]);
        let (all, _) = cold_set(&g, &p, &r, &[]);
        assert_eq!(all.len(), 3);
        let (plan, cold) = place_cold(&g, &p, &[], &crates(&g), &r, &all).unwrap();
        // 127 pages = 1,040,384 B: `dev::c` and `lib::b` come back
        // (1,037,000 B), `conv::a` would not leave a page free.
        assert_eq!(cold.into_iter().collect::<Vec<_>>(), vec![1]);
        assert_eq!(plan.code_size, 1_037_000);
        assert_eq!(plan.cold.len(), 1);
        assert!(plan.code_size <= (MAX_CODE_PAGES - CODE_RESERVE_PAGES) * PAGE_SIZE);
    }

    /// PG-2b, 2026-11-27: MIDI ("Etude") underran more after PG-2 (3–10
    /// underruns where ≈ 1 had been, `ub: dec=`): the synth's per-piece
    /// code read small anonymous constants (clock steps, field tables,
    /// switch tables) that PG-2's new rodata order put on physical lines
    /// 5, 10 and 11, inside the wavetable's 14 lines: 1,170 rodata
    /// faults/s, 2,297 line loads/s. `resident_constants` makes a
    /// cluster's small plain constants resident; the real `hot.toml` asks
    /// it for the synth's per-piece and outer-loop clusters.
    #[test]
    fn regress_synth_piece_constants_on_the_wavetable_lines() {
        let g = Graph::from_parts(
            vec![
                node(".text", ".text.plan", 400, 2, Some("synth::plan_piece")),
                node(".rodata", ".rodata..Lanon.1", 12, 4, None),
                node(".rodata", ".rodata.waves", 14_336, 4, Some("synth::WAVES")),
                node(".rodata", ".rodata.ptrs", 16, 4, None),
                node(".text", ".text.other", 100, 2, Some("ui::draw")),
                node(".rodata", ".rodata..Lanon.2", 12, 4, None),
            ],
            // plan reads the 12-byte constant, the table, a table of
            // pointers (to `other`); `other` reads its own constant.
            &[(0, 1), (0, 2), (0, 3), (3, 4), (4, 5)],
        );
        let r = Rules::parse("roots = []\n", "t").unwrap();
        let (mut p, _) = classify(&g, &r, &[]);
        let hot = vec![Cluster {
            name: "synth-inner".into(),
            symbols: vec!["synth::plan_piece".into()],
            resident_constants: Some(256),
            ..Cluster::default()
        }];
        assert_eq!(cluster_constants(&g, &mut p, &hot), (1, 12));
        assert!(matches!(&p[1], Place::Resident(r) if r == "cluster-constants:synth-inner"));
        assert!(matches!(p[2], Place::Paged(_)), "the wavetable is larger than the limit");
        assert!(matches!(p[3], Place::Paged(_)), "a table with pointers stays paged");
        assert!(matches!(p[5], Place::Paged(_)), "another cluster's constant");
        let real = rules::load_hot(&root().join("hot.toml"), "firmware").unwrap();
        for name in ["synth-inner", "synth-block"] {
            let c = real.iter().find(|c| c.name == name).unwrap();
            assert!(c.resident_constants.is_some_and(|n| n >= 24), "{name}: resident_constants");
        }
    }

    /// PG-2b (2026-11-27): a cold section that shares a physical line with
    /// a constant it reads comes back into the code window before warmer
    /// ones (SHA-256's rounds over its `K` ran an update at 2.5 KB/s).
    #[test]
    fn cold_code_that_conflicts_with_its_constants_comes_back_first() {
        let g = Graph::from_parts(
            vec![
                node(".text", ".text.big", 1_030_000, 2, Some("app::big")),
                node(".text", ".text.warm", 6000, 2, Some("dev::warm")),
                node(".text", ".text.rounds", 6000, 2, Some("sha::rounds")),
                // A whole group of constants: every cold line conflicts.
                node(".rodata", ".rodata.k", 16_384, 4, Some("sha::K")),
            ],
            &[(2, 3)],
        );
        let r = Rules::parse("roots = []\n[cold]\nsymbols = [\"sha::*\", \"dev::*\"]\n", "t").unwrap();
        let (p, _) = classify(&g, &r, &[]);
        let (all, _) = cold_set(&g, &p, &r, &[]);
        assert_eq!(all.iter().copied().collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(pack_cold(&g, &p, &[], &crates(&g), &all).unwrap().cold_conflicts, 1);
        // Room for one of the two (10,384 B): the conflict, not the warmer.
        let (plan, cold) = place_cold(&g, &p, &[], &crates(&g), &r, &all).unwrap();
        assert_eq!(cold.into_iter().collect::<Vec<_>>(), vec![1]);
        assert_eq!(plan.cold_conflicts, 0);
        assert!(plan.code_size <= (MAX_CODE_PAGES - CODE_RESERVE_PAGES) * PAGE_SIZE);
    }

    /// Bug 2026-11-26/27 (TTS-3, BOOK-1/2 wiring): the speech voice with
    /// the screen reader (133 pages) and Books (139) did not fit the
    /// 128-page code window; both together needed 148 pages (1,206,358 B,
    /// `xtask build --features speech,books`: "paged code needs 148
    /// pages"), so neither could go into the image. PG-2 runs cold code
    /// from the rodata window, which now covers all of SRAM3's aliases
    /// (64 pages). The input is that build's paged sections
    /// (`tests/regress/pagelink/speech-books-2026-11-27.txt`, replayed
    /// through `pack_cold` without the relocation graph, so without the
    /// constants' lines): without cold code the window overflows; with it
    /// both windows keep room.
    #[test]
    fn regress_speech_and_books_did_not_fit_the_code_window() {
        let text = std::fs::read_to_string(root().join("tests/regress/pagelink/speech-books-2026-11-27.txt"))
            .expect("fixture");
        let mut crate_names: Vec<String> = Vec::new();
        let mut hot = Vec::new();
        let mut nodes = Vec::new();
        let mut node_crates = Vec::new();
        let mut cold = BTreeSet::new();
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = line.split_whitespace().collect();
            match f[0] {
                "crates" => crate_names = f[1..].iter().map(|s| (*s).to_string()).collect(),
                "cluster" => hot.push(Cluster {
                    name: f[2].into(),
                    symbols: vec![format!("cl{}::*", f[1])],
                    starts_over: (f[3] != "-").then(|| f[3].to_string()),
                    ..Cluster::default()
                }),
                k => {
                    let n = nodes.len();
                    let size = u32::from_str_radix(f[1], 16).unwrap();
                    let align = u32::from_str_radix(f[2], 16).unwrap();
                    let sym = if f[4] == "-" { format!("s::x{n}") } else { format!("cl{}::m{n}", f[4]) };
                    let (out, name) = if k == "c" {
                        (".text", format!(".text.s{n}"))
                    } else {
                        (".rodata", format!(".rodata.s{n}"))
                    };
                    let mut s = node(out, &name, size, align, Some(&sym));
                    s.addr = 0x1000_0000 + 4 * n as u32;
                    nodes.push(s);
                    node_crates.push(crate_names[f[3].parse::<usize>().unwrap()].clone());
                    if f[5] == "1" {
                        cold.insert(n);
                    }
                }
            }
        }
        let g = Graph::from_parts(nodes, &[]);
        let places = vec![Place::Paged("default".into()); g.nodes.len()];
        let code: u32 =
            (0..g.nodes.len()).filter(|&n| g.kinds[n] == Kind::Code).map(|n| g.nodes[n].size).sum();
        assert!(code > 1_200_000, "{code} B of paged code");
        let err = pack_cold(&g, &places, &hot, &node_crates, &BTreeSet::new()).unwrap_err();
        // (148 pages in the build; the replay's fillers differ a little.)
        assert!(err.to_string().contains("paged code needs 14"), "{err}");
        let plan = pack_cold(&g, &places, &hot, &node_crates, &cold).unwrap();
        assert!(plan.ncode <= MAX_CODE_PAGES - 3, "code window: {} pages", plan.ncode);
        assert!(plan.nro < MAX_RO_PAGES, "rodata window: {} pages", plan.nro);
        assert!(plan.nro_code >= 20 && plan.cold.len() as u32 * 2 >= plan.nro_code, "{}", plan.nro_code);
        assert_eq!(plan.cold_conflicts, 0, "no graph, no constants' lines");
    }
}
