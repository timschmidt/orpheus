//! Reference graph of a linked image at input-section granularity.
//!
//! Nodes are the input sections of the link (from the lld map: Rust gives
//! every function and static its own section, so a node is one function or
//! one object). Edges come from the relocations the link was asked to keep
//! (`--emit-relocs`): each relocation site lies in one node and its target
//! is decoded from the *linked* bytes (branch offsets, literal words,
//! `MOVW`/`MOVT` pairs), so the target address is exact and no disassembly
//! heuristics are needed to tell code from literal pools. References that
//! are not relocations (an address computed at run time, an indirect call)
//! are not edges; `check-resident` lists the indirect branches it finds.
//!
//! Used by `pagelink` on the pass-1 link (to compute the resident closure)
//! and by `check-resident` on the final link.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use object::{
    Object, ObjectSection, ObjectSymbol, RelocationFlags, RelocationTarget, SectionFlags, SymbolKind,
    elf::SHF_ALLOC,
};

use crate::linkmap::{InputSection, LinkMap};
use crate::thumb::{self, rel};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Code,
    Rodata,
    Ram,
    Other,
}

impl Kind {
    pub fn of_output(out: &str) -> Self {
        let is = |p: &str| out == p || out.starts_with(&format!("{p}."));
        if is(".text") || out.starts_with(".pg_") {
            Self::Code
        } else if is(".rodata") || out.starts_with(".pr_") {
            Self::Rodata
        } else if [".data", ".bss", ".uninit", ".stack", ".framebuffer"].iter().any(|p| is(p)) {
            Self::Ram
        } else {
            Self::Other
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Rodata => "rodata",
            Self::Ram => "ram",
            Self::Other => "other",
        }
    }
}

pub struct Graph {
    pub nodes: Vec<InputSection>,
    pub kinds: Vec<Kind>,
    /// Outgoing edges per node, sorted, without duplicates or self-edges.
    pub edges: Vec<Vec<usize>>,
    /// Direct branches between code nodes: `(callee, tail)` per node; tail
    /// for `B`/`B<cond>` (a tail call: the caller's frame is gone), not for
    /// `BL`/`BLX`. Sorted; self-calls kept (recursion).
    pub calls: Vec<Vec<(usize, bool)>>,
    /// Node indices sorted by address (for lookups).
    by_addr: Vec<usize>,
    /// Allocated section contents of the ELF: (address, bytes).
    image: Vec<(u32, Vec<u8>)>,
    /// Mapping symbols `$t` (false) / `$d` (true), sorted by address.
    mapping: Vec<(u32, bool)>,
    /// Relocations whose target could not be decoded.
    pub unresolved: Vec<String>,
}

/// A `MOVW`/`MOVT` relocation waiting for its partner.
struct MovHalf {
    place: u32,
    movt: bool,
    rd: u8,
    imm: u16,
    sym: usize,
    sym_value: Option<u32>,
}

impl Graph {
    pub fn load(elf: &Path, map: LinkMap) -> Result<Self> {
        let data = std::fs::read(elf).with_context(|| format!("read {}", elf.display()))?;
        let file = object::File::parse(&*data)?;
        let mut nodes: Vec<InputSection> = map
            .inputs
            .into_iter()
            .filter(|s| s.size > 0 && s.addr != 0 && Kind::of_output(&s.out) != Kind::Other)
            .collect();
        nodes.sort_by_key(|s| (s.addr, s.size));
        let kinds = nodes.iter().map(|n| Kind::of_output(&n.out)).collect();
        let by_addr = (0..nodes.len()).collect();
        let mut image = Vec::new();
        for sec in file.sections() {
            let alloc =
                matches!(sec.flags(), SectionFlags::Elf { sh_flags } if sh_flags & u64::from(SHF_ALLOC) != 0);
            if alloc
                && let Ok(d) = sec.data()
                && !d.is_empty()
            {
                image.push((u32::try_from(sec.address())?, d.to_vec()));
            }
        }
        let mut mapping = Vec::new();
        for sym in file.symbols() {
            match sym.name() {
                Ok(n) if n == "$t" || n.starts_with("$t.") => mapping.push((sym.address() as u32, false)),
                Ok(n) if n == "$d" || n.starts_with("$d.") => mapping.push((sym.address() as u32, true)),
                _ => {}
            }
        }
        mapping.sort_unstable();
        let mut g = Self {
            edges: vec![Vec::new(); nodes.len()],
            calls: vec![Vec::new(); nodes.len()],
            nodes,
            kinds,
            by_addr,
            image,
            mapping,
            unresolved: Vec::new(),
        };
        g.load_edges(&file)?;
        Ok(g)
    }

    /// A graph from parts (tests of the planning logic).
    #[cfg(test)]
    pub fn from_parts(mut nodes: Vec<InputSection>, edges: &[(usize, usize)]) -> Self {
        for (i, n) in nodes.iter_mut().enumerate() {
            if n.addr == 0 {
                n.addr = 0x1000_0000 + 0x1_0000 * i as u32;
            }
        }
        let kinds = nodes.iter().map(|n| Kind::of_output(&n.out)).collect();
        let mut e = vec![Vec::new(); nodes.len()];
        for &(a, b) in edges {
            e[a].push(b);
        }
        let mut by_addr: Vec<usize> = (0..nodes.len()).collect();
        by_addr.sort_by_key(|&i| nodes[i].addr);
        Self {
            nodes,
            kinds,
            calls: e.iter().map(|v| v.iter().map(|&t| (t, false)).collect()).collect(),
            edges: e,
            by_addr,
            image: Vec::new(),
            mapping: Vec::new(),
            unresolved: Vec::new(),
        }
    }

    /// Bytes of the linked image at `addr`.
    pub fn bytes(&self, addr: u32, len: u32) -> Option<&[u8]> {
        self.image.iter().find_map(|(a, d)| {
            let off = addr.checked_sub(*a)? as usize;
            d.get(off..off.checked_add(len as usize)?)
        })
    }

    fn hw(&self, addr: u32) -> Option<u16> {
        self.bytes(addr, 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    fn word(&self, addr: u32) -> Option<u32> {
        self.bytes(addr, 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// The node containing `addr`.
    pub fn node_at(&self, addr: u32) -> Option<usize> {
        let i = self.by_addr.partition_point(|&n| self.nodes[n].addr <= addr);
        let n = *self.by_addr.get(i.checked_sub(1)?)?;
        let s = &self.nodes[n];
        (addr - s.addr < s.size).then_some(n)
    }

    /// The node a reference to `addr` means: Thumb code addresses have bit
    /// 0 set.
    fn target_node(&self, addr: u32) -> Option<usize> {
        match self.node_at(addr) {
            Some(n) if self.kinds[n] != Kind::Code || addr & 1 == 0 => Some(n),
            _ => self.node_at(addr & !1),
        }
    }

    fn load_edges(&mut self, file: &object::File<'_>) -> Result<()> {
        let mut edges: Vec<(usize, usize)> = Vec::new();
        let mut calls: Vec<(usize, usize, bool)> = Vec::new();
        let mut movs: Vec<MovHalf> = Vec::new();
        for sec in file.sections() {
            let alloc =
                matches!(sec.flags(), SectionFlags::Elf { sh_flags } if sh_flags & u64::from(SHF_ALLOC) != 0);
            if !alloc {
                continue;
            }
            for (off, r) in sec.relocations() {
                let RelocationFlags::Elf { r_type } = r.flags() else { continue };
                let place = u32::try_from(off)?;
                let Some(from) = self.node_at(place) else { continue };
                let (sym, sym_value) = match r.target() {
                    RelocationTarget::Symbol(i) => {
                        let s = file.symbol_by_index(i)?;
                        let named = s.kind() != SymbolKind::Section && !s.is_undefined();
                        (i.0, named.then(|| s.address() as u32))
                    }
                    _ => (usize::MAX, None),
                };
                let target = match r_type {
                    rel::NONE | rel::V4BX | rel::PREL31 => continue,
                    rel::ABS32 | rel::TARGET1 => self.word(place),
                    rel::REL32 => self.word(place).map(|w| w.wrapping_add(place)),
                    rel::THM_CALL | rel::THM_JUMP24 => {
                        self.hw(place).zip(self.hw(place + 2)).map(|(a, b)| thumb::bl_target(place, a, b))
                    }
                    rel::THM_JUMP19 => self
                        .hw(place)
                        .zip(self.hw(place + 2))
                        .map(|(a, b)| thumb::bcond_w_target(place, a, b)),
                    rel::THM_JUMP11 => self.hw(place).map(|a| thumb::b16_target(place, a)),
                    rel::THM_JUMP8 => self.hw(place).map(|a| thumb::bcond16_target(place, a)),
                    rel::THM_PC8 => self.hw(place).map(|a| thumb::ldr16_lit_target(place, a)),
                    rel::THM_PC12 => self
                        .hw(place)
                        .zip(self.hw(place + 2))
                        .map(|(a, b)| thumb::ldr_w_lit_target(place, a, b)),
                    rel::THM_ALU_PREL_11_0 => {
                        self.hw(place).zip(self.hw(place + 2)).map(|(a, b)| thumb::adr_w_target(place, a, b))
                    }
                    rel::THM_MOVW_ABS_NC | rel::THM_MOVT_ABS => {
                        if let Some((a, b)) = self.hw(place).zip(self.hw(place + 2)) {
                            let (imm, rd) = thumb::mov_imm16(a, b);
                            movs.push(MovHalf {
                                place,
                                movt: r_type == rel::THM_MOVT_ABS,
                                rd,
                                imm,
                                sym,
                                sym_value,
                            });
                        }
                        continue;
                    }
                    _ => sym_value,
                };
                match target {
                    Some(t) => {
                        if let Some(to) = self.target_node(t) {
                            edges.push((from, to));
                            let branch = matches!(
                                r_type,
                                rel::THM_CALL
                                    | rel::THM_JUMP24
                                    | rel::THM_JUMP19
                                    | rel::THM_JUMP11
                                    | rel::THM_JUMP8
                            );
                            if branch && self.kinds[from] == Kind::Code && self.kinds[to] == Kind::Code {
                                calls.push((from, to, r_type != rel::THM_CALL));
                            }
                        }
                    }
                    None => self.unresolved.push(format!("type {r_type} at {place:#x}")),
                }
            }
        }
        // Pair each MOVW with the nearest MOVT of the same register and
        // symbol in the same node.
        movs.sort_by_key(|m| m.place);
        const WINDOW: u32 = 512;
        for (i, m) in movs.iter().enumerate().filter(|(_, m)| !m.movt) {
            let node = self.node_at(m.place);
            let lo = movs.partition_point(|o| o.place + WINDOW < m.place);
            let hi = movs.partition_point(|o| o.place <= m.place + WINDOW);
            let partner = movs[lo..hi]
                .iter()
                .enumerate()
                .filter(|(j, o)| {
                    lo + j != i && o.movt && o.rd == m.rd && o.sym == m.sym && self.node_at(o.place) == node
                })
                .min_by_key(|(_, o)| o.place.abs_diff(m.place));
            let target = match partner {
                Some((_, hi)) => Some((u32::from(hi.imm) << 16) | u32::from(m.imm)),
                None => m.sym_value,
            };
            match (target, node) {
                (Some(t), Some(from)) => {
                    if let Some(to) = self.target_node(t) {
                        edges.push((from, to));
                    }
                }
                _ => self.unresolved.push(format!("unpaired movw at {:#x}", m.place)),
            }
        }
        for (from, to, tail) in calls {
            self.calls[from].push((to, tail));
        }
        for c in &mut self.calls {
            c.sort_unstable();
            c.dedup();
        }
        for (from, to) in edges {
            if from != to {
                self.edges[from].push(to);
            }
        }
        for e in &mut self.edges {
            e.sort_unstable();
            e.dedup();
        }
        Ok(())
    }

    /// Breadth-first walk from `roots`; returns the parent of every reached
    /// node (`Some(usize::MAX)` for a root).
    pub fn walk(&self, roots: &[usize]) -> Vec<Option<usize>> {
        let mut parent = vec![None; self.nodes.len()];
        let mut queue = std::collections::VecDeque::new();
        for &r in roots {
            if parent[r].is_none() {
                parent[r] = Some(usize::MAX);
                queue.push_back(r);
            }
        }
        while let Some(n) = queue.pop_front() {
            for &m in &self.edges[n] {
                if parent[m].is_none() {
                    parent[m] = Some(n);
                    queue.push_back(m);
                }
            }
        }
        parent
    }

    /// Chain of labels from a root to `n` along `parent`.
    pub fn chain(&self, parent: &[Option<usize>], mut n: usize) -> Vec<String> {
        let mut out = vec![self.label(n)];
        while let Some(p) = parent[n] {
            if p == usize::MAX || out.len() > 64 {
                break;
            }
            out.push(self.label(p));
            n = p;
        }
        out.reverse();
        out
    }

    /// Readable name: the first symbol (demangled, without hash), else the
    /// section name.
    pub fn label(&self, n: usize) -> String {
        let s = &self.nodes[n];
        match s.symbols.first() {
            Some(sym) => demangle(&sym.name),
            None => s.name.clone(),
        }
    }

    /// Nodes the vector table at symbol `__vectors` points to, skipping
    /// the entries in `skip` and empty entries: `(entry, node)`.
    pub fn vector_targets(&self, skip: &[usize]) -> Vec<(usize, usize)> {
        let Some(v) = self.find_symbol("__vectors") else { return Vec::new() };
        let s = &self.nodes[v];
        let mut out = Vec::new();
        for i in 0..(s.size / 4) as usize {
            if skip.contains(&i) {
                continue;
            }
            let Some(w) = self.word(s.addr + 4 * i as u32) else { continue };
            if w == 0 {
                continue;
            }
            if let Some(n) = self.target_node(w) {
                out.push((i, n));
            }
        }
        out
    }

    pub fn find_symbol(&self, name: &str) -> Option<usize> {
        self.nodes.iter().position(|s| s.symbols.iter().any(|y| y.name == name))
    }

    /// Indirect branches in code node `n` (skipping literal data by the
    /// mapping symbols): `(address, instruction)`.
    pub fn indirect_branches(&self, n: usize) -> Vec<(u32, String)> {
        let s = &self.nodes[n];
        let end = s.addr + s.size;
        let mut out = Vec::new();
        let mut pc = s.addr;
        while pc + 2 <= end {
            let i = self.mapping.partition_point(|&(a, _)| a <= pc);
            if i > 0 && self.mapping[i - 1].1 {
                // In data: continue at the next mapping symbol.
                match self.mapping.get(i) {
                    Some(&(a, _)) if a < end => {
                        pc = a;
                        continue;
                    }
                    _ => break,
                }
            }
            let Some(hw1) = self.hw(pc) else { break };
            let len = thumb::insn_len(hw1);
            let hw2 = if len == 4 { self.hw(pc + 2).unwrap_or(0) } else { 0 };
            if let Some(d) = thumb::indirect(hw1, hw2) {
                out.push((pc, d));
            }
            pc += len;
        }
        out
    }

    /// Index of nodes by section name, for names that occur once.
    pub fn unique_names(&self) -> BTreeMap<&str, usize> {
        let mut count: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for (i, s) in self.nodes.iter().enumerate() {
            let e = count.entry(s.name.as_str()).or_insert((0, i));
            e.0 += 1;
        }
        count.into_iter().filter(|(_, (c, _))| *c == 1).map(|(k, (_, i))| (k, i)).collect()
    }
}

/// Demangle a Rust symbol without its hash (names lld already demangled
/// pass through).
pub fn demangle(name: &str) -> String {
    match rustc_demangle::try_demangle(name) {
        Ok(d) => format!("{d:#}"),
        Err(_) => name.to_string(),
    }
}

/// The crate a demangled symbol belongs to (`<a::B as c::D>::f` → `a`).
pub fn crate_of(demangled: &str) -> Option<&str> {
    let s = demangled.trim_start_matches('<').trim_start_matches('&').trim_start_matches("mut ");
    let end = s.find("::")?;
    let c = &s[..end];
    (!c.is_empty() && c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')).then_some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_names() {
        assert_eq!(crate_of("atj2157_hal::rt::irq_dispatch"), Some("atj2157_hal"));
        assert_eq!(
            crate_of("<embassy_executor::raw::util::UninitCell<x::Y>>::write_in_place"),
            Some("embassy_executor")
        );
        assert_eq!(crate_of("memcpy"), None);
        assert_eq!(
            demangle("_ZN11atj2157_hal2rt12irq_dispatch17h0123456789abcdefE"),
            "atj2157_hal::rt::irq_dispatch"
        );
        assert_eq!(demangle("toy_memmanage"), "toy_memmanage");
    }
}
