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
//!    block), first fit.
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

/// Classify every node of the pass-1 graph. Returns the placements and
/// warnings.
pub fn classify(g: &Graph, rules: &Rules, roots: &[(String, Vec<usize>)]) -> (Vec<Place>, Vec<String>) {
    let unique = g.unique_names();
    let root_nodes: Vec<usize> = roots.iter().flat_map(|(_, v)| v.iter().copied()).collect();
    let parent = g.walk(&root_nodes);
    let mut warnings = Vec::new();
    let places = (0..g.nodes.len())
        .map(|n| {
            let s = &g.nodes[n];
            let kind = g.kinds[n];
            if !matches!(kind, Kind::Code | Kind::Rodata) {
                return Place::Ram;
            }
            if let Some(why) = unmovable(g, n, &unique) {
                return Place::Resident(format!("fixed:{why}"));
            }
            let too_big = kind == Kind::Rodata && s.size > RO_GROUP_SIZE;
            if let Some(p) = rules.paged.select(s) {
                if too_big {
                    warnings.push(format!("{} forced paged but larger than a rodata group", g.label(n)));
                    return Place::Resident("fixed:larger than a rodata group".into());
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
                return Place::Resident("fixed:larger than a rodata group".into());
            }
            Place::Paged("default".into())
        })
        .collect();
    (places, warnings)
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
    pub ncode: u32,
    pub nro: u32,
    /// Where the `hot.toml` clusters of code went.
    pub clusters: Vec<ClusterInfo>,
    /// Bytes left empty before `starts_over` clusters.
    pub padding: u32,
}

impl Plan {
    /// Final address of every paged node.
    pub fn addresses(&self) -> BTreeMap<usize, u32> {
        let mut m = BTreeMap::new();
        for &(n, off, _) in &self.code {
            m.insert(n, code_page_va(0) + off);
        }
        for (g, (secs, _)) in self.groups.iter().enumerate() {
            for &(n, off) in secs {
                m.insert(n, ro_page_va(2 * g as u32) + off);
            }
        }
        m
    }
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

/// Order and pack the paged sections.
pub fn pack(g: &Graph, places: &[Place], hot: &[Cluster], crates: &[String]) -> Result<Plan> {
    let paged = |k: Kind| -> Vec<usize> {
        (0..g.nodes.len()).filter(|&n| g.kinds[n] == k && matches!(places[n], Place::Paged(_))).collect()
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
    for (n, cluster) in order(paged(Kind::Code)) {
        let start = cluster.is_some() && cluster != prev;
        if start {
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
    if plan.groups.len() as u32 > MAX_RO_GROUPS {
        bail!("paged rodata needs {} groups of 16 KiB, the window holds {MAX_RO_GROUPS}", plan.groups.len());
    }
    plan.nro = match plan.groups.last() {
        Some((_, used)) => 2 * (plan.groups.len() as u32 - 1) + used.div_ceil(PAGE_SIZE),
        None => 0,
    };
    Ok(plan)
}

/// The pass-2 linker script.
pub fn script(g: &Graph, plan: &Plan, script_dir: &Path, bin: &str) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "/* Generated by `cargo xtask pagelink` for {bin}; do not edit. */");
    let _ = writeln!(s, "INCLUDE \"{}\"", script_dir.join("memory.ld").display());
    let _ = writeln!(s, "__orpheus_code_pages = {};", plan.ncode);
    let _ = writeln!(s, "__orpheus_rodata_pages = {};", plan.nro);
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
    for (gi, (secs, _)) in plan.groups.iter().enumerate() {
        let _ = writeln!(s, "    .pr_g{gi} {:#010x} :\n    {{", ro_page_va(2 * gi as u32));
        for &(n, _) in secs {
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
    for (gi, (_, used)) in plan.groups.iter().enumerate() {
        let _ = writeln!(
            s,
            "ASSERT(SIZEOF(.pr_g{gi}) == {used:#x}, \"pagelink: .pr_g{gi} differs from the plan\");"
        );
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
        } else if let Some(gi) = name.strip_prefix(".pr_g") {
            x.groups.insert(gi.parse().context("group index")?, sec.data()?.to_vec());
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
    ncode: u32,
    nro: u32,
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
    let (places, mut warnings) = classify(&g1, &rules, &roots);
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
    let plan = pack(&g1, &places, &hot, &crates1)?;
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

    // Images.
    let x = extract(&a.elf)?;
    let store = page_store(&x, plan.ncode, plan.nro)?;
    let (slot, header) = slot_image(&x.resident, x.entry, &store, plan.ncode, plan.nro, o.layout);
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
    let crates2 = crates(&g2);
    let sections = (0..g2.nodes.len())
        .map(|m| {
            let s = &g2.nodes[m];
            let kind = g2.kinds[m];
            let (placement, reason) = match reasons.get(s.name.as_str()) {
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
    for (gi, (_, used)) in plan.groups.iter().enumerate() {
        groups.push(GroupInfo {
            name: format!(".pr_g{gi}"),
            kind: "rodata".into(),
            va: ro_page_va(2 * gi as u32),
            size: *used,
            capacity: RO_GROUP_SIZE,
            first_page: 2 * gi as u32,
            pages: used.div_ceil(PAGE_SIZE),
        });
    }
    let code_bytes = plan.code.iter().map(|&(n, _, _)| g1.nodes[n].size).sum();
    let rodata_bytes = plan.groups.iter().flat_map(|(s, _)| s).map(|&(n, _)| g1.nodes[n].size).sum();
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
        assert!(s.contains(".pr_g1 0x00348000 :"));
        assert!(s.contains("ASSERT(SIZEOF(.pr_g0) == 0x3a98"));
        let addrs = plan.addresses();
        assert_eq!(addrs[&13], 0x34_0000 + 9000);
        assert_eq!(addrs[&12], 0x34_8000);
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
        // a: line 0 (700 B); b would start on line 1: 31 lines of padding.
        assert_eq!(plan.code, vec![(10, 0, true), (9, 0x8000, true), (7, 0x8000 + 500, false)]);
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
        let (img, h) = slot_image(&resident, 0x10_0001, &store, 2, 1, LAYOUT_DUAL);
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
        assert_eq!(slot_image(&resident, 0x10_0001, &store, 2, 1, LAYOUT_DUAL).1.build_id, h.build_id);
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
}
