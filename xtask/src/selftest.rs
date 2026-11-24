//! `xtask test pagelink`: the paged toy (`fw/bringup/pagetoy`) end to end.
//!
//! Builds the toy, pagelinks it, and checks: page and group geometry and
//! alignment, the closure (helpers no rule names became resident), the hot
//! cluster, a code section crossing pages and a rodata section spanning a
//! group, the slot header, determinism (a second pagelink gives identical
//! files), `check-resident` passing, a replay of every paged byte through
//! the simulated MPU and pager against the ELF, and a planted violation
//! that `check-resident` must report with its chain.

use anyhow::{Context, Result, bail, ensure};
use orpheus_pager::sim::{Kind as Access, Machine};
use orpheus_pager::{PAGE_SIZE, Page, RO_GROUP_SIZE, RO_WIN};
use orpheus_proto::slot::{LAYOUT_DUAL, SlotHeader};
use sha2::{Digest, Sha256};

use crate::linkgraph::Graph;
use crate::pagelink::{self, Layout, Options, SRAM0, artefacts};
use crate::util::log;
use crate::{linkmap, resident};

const BIN: &str = "bringup-pagetoy";
const PLANTED: &str = "bringup-pagetoy-planted";

fn digest(path: &std::path::Path) -> Result<Vec<u8>> {
    Ok(Sha256::digest(std::fs::read(path).with_context(|| format!("read {}", path.display()))?).to_vec())
}

fn section<'a>(l: &'a Layout, symbol: &str) -> Result<&'a pagelink::SectionInfo> {
    l.sections.iter().find(|s| s.symbol == symbol).with_context(|| format!("{symbol} not in layout.json"))
}

pub fn run() -> Result<String> {
    crate::build::cargo_build(BIN, BIN, true, None)?;
    let opts = |name: &str, paged: &[&str]| Options {
        bin: BIN.into(),
        release: true,
        layout: LAYOUT_DUAL,
        force_paged: paged.iter().map(|s| (*s).to_string()).collect(),
        name: Some(name.into()),
    };
    let l = pagelink::run(&opts(BIN, &[]))?;
    pagelink::print_summary(&l);
    let a = artefacts(BIN);

    // Determinism: link again, same bytes.
    let first: Vec<Vec<u8>> = [&a.yfw, &a.layout, &a.elf].iter().map(|p| digest(p)).collect::<Result<_>>()?;
    pagelink::run(&opts(BIN, &[]))?;
    let second: Vec<Vec<u8>> =
        [&a.yfw, &a.layout, &a.elf].iter().map(|p| digest(p)).collect::<Result<_>>()?;
    ensure!(first == second, "pagelink is not deterministic");

    // Geometry.
    ensure!(l.pages.code >= 4 && l.pages.rodata >= 4, "toy too small: {:?}", l.pages);
    ensure!(l.warnings.is_empty(), "warnings: {:?}", l.warnings);
    let g = Graph::load(&a.elf, linkmap::parse(&std::fs::read_to_string(&a.map)?)?)?;
    let mut crosses_page = false;
    let mut spans_group_pages = false;
    for s in l.sections.iter().filter(|s| s.placement == "paged") {
        let n = g.node_at(s.addr).context("paged section not in the final map")?;
        ensure!(s.addr % g.nodes[n].align.max(1) == 0, "{} misaligned", s.symbol);
        let (first, last) = (Page::of(s.addr), Page::of(s.addr + s.size - 1));
        match (s.kind.as_str(), first, last) {
            ("code", Some(Page::Code(p)), Some(Page::Code(q))) => {
                ensure!(u32::from(q) < l.pages.code, "{} beyond the code pages", s.symbol);
                crosses_page |= p != q;
            }
            ("rodata", Some(Page::Rodata(p)), Some(Page::Rodata(q))) => {
                ensure!(u32::from(q) < l.pages.rodata, "{} beyond the rodata pages", s.symbol);
                let off = (s.addr - RO_WIN) % 0x8000;
                ensure!(off + s.size <= RO_GROUP_SIZE, "{} leaves its 16 KiB group", s.symbol);
                spans_group_pages |= p != q;
            }
            _ => bail!("{} ({}) at {:#x} is not in its window", s.symbol, s.kind, s.addr),
        }
        let page = Page::of(s.addr).context("page")?;
        ensure!(
            s.store_offset == Some(page.store_index(l.pages.code) * PAGE_SIZE + (s.addr - page.va())),
            "{}: store offset",
            s.symbol
        );
    }
    ensure!(crosses_page, "no code section crosses a page boundary (work_17 should)");
    ensure!(spans_group_pages, "no rodata section spans both pages of a group (TABLE_11 should)");
    for s in l.sections.iter().filter(|s| s.placement != "paged") {
        ensure!(Page::of(s.addr).is_none(), "{} is not paged but lies in a page window", s.symbol);
    }

    // Closure and rules.
    for sym in ["toy_memmanage", "bringup_pagetoy::handler_helper", "bringup_pagetoy::HELPER_TABLE"] {
        let s = section(&l, sym)?;
        ensure!(s.placement == "resident" && s.reason.starts_with("closure:"), "{sym}: {}", s.reason);
    }
    ensure!(
        section(&l, "<orpheus_pager::PagerCore>::fault")?.placement == "resident",
        "PagerCore::fault paged"
    );
    ensure!(section(&l, "bringup_pagetoy::generated::work_17")?.placement == "paged", "work_17 not paged");

    // Hot cluster: adjacent, in pattern order, starting on a line.
    let hot: Vec<&pagelink::SectionInfo> = ["work_40", "work_3", "work_9"]
        .iter()
        .map(|f| section(&l, &format!("bringup_pagetoy::generated::{f}")))
        .collect::<Result<_>>()?;
    ensure!(hot[0].addr.is_multiple_of(0x400), "hot cluster does not start on a line");
    for w in hot.windows(2) {
        ensure!(w[1].addr - (w[0].addr + w[0].size) < 8, "hot cluster not adjacent");
    }

    // Slot image.
    let slot = std::fs::read(&a.yfw)?;
    let h = SlotHeader::from_bytes(&slot).map_err(|e| anyhow::anyhow!("slot header: {e:?}"))?;
    let resident = std::fs::read(&a.resident)?;
    let store = std::fs::read(&a.pages)?;
    ensure!(h.load_addr == SRAM0.start && h.entry == l.resident.entry && h.entry & 1 == 1, "entry/load");
    ensure!((h.code_pages, h.rodata_pages) == (l.pages.code, l.pages.rodata), "page counts");
    let r0 = h.resident_off as usize;
    ensure!(slot[r0..r0 + resident.len()] == resident[..], "resident image in slot");
    ensure!(slot[h.pages_off as usize..] == store[..], "page store in slot");
    ensure!(h.pages_crc == orpheus_proto::hash::crc32(&store), "pages crc");
    ensure!(h.resident_sum == orpheus_proto::hash::sum32(&resident), "resident sum");
    ensure!(store.len() == ((l.pages.code + l.pages.rodata) * PAGE_SIZE) as usize, "store size");

    // check-resident on the real layout.
    let r = resident::check(&a.elf, &a.map, BIN)?;
    resident::print(&r);
    ensure!(r.violations.is_empty(), "check-resident: {:?}", r.violations);
    ensure!(r.panics.is_empty(), "panic paths reachable from the roots: {:?}", r.panics);

    // Replay: every paged byte through the simulated MPU + pager, compared
    // with the ELF.
    let mut m = Box::new(Machine::new(&store, l.pages.code, l.pages.rodata));
    let pc = SRAM0.start + 0x400;
    let mut checked = 0u64;
    for s in l.sections.iter().filter(|s| s.placement == "paged") {
        let want = g.bytes(s.addr, s.size).context("ELF bytes")?;
        let code = s.kind == "code";
        let step = if code { 2 } else { 4 };
        let mut off = 0;
        while off < s.size {
            let n = step.min(s.size - off);
            let addr = s.addr + off;
            let got = if code && n == 2 {
                m.access(addr, 2, Access::Fetch, addr)
            } else {
                m.access(addr, n, Access::Read, pc)
            }
            .map_err(|e| anyhow::anyhow!("{} +{off:#x}: {e:?}", s.symbol))?;
            let exp = want[off as usize..(off + n) as usize]
                .iter()
                .rev()
                .fold(0u64, |v, &b| (v << 8) | u64::from(b));
            ensure!(got == exp, "{} +{off:#x}: pager returned {got:#x}, ELF has {exp:#x}", s.symbol);
            checked += u64::from(n);
            off += n;
        }
    }
    m.check().map_err(|e| anyhow::anyhow!("pager invariant: {e}"))?;
    let stats = m.core.stats();
    ensure!(stats.fatal == 0, "fatal faults in the replay");
    // Every line holding paged bytes was read from the store at least once.
    ensure!(
        u64::from(stats.loads) * 1024 >= checked,
        "replay loaded {} lines for {checked} bytes",
        stats.loads
    );
    log(&format!(
        "  replay: {checked} paged bytes match the ELF; {} faults, {} line loads",
        m.faults, stats.loads
    ));

    // A planted violation must be caught with its chain.
    let p = pagelink::run(&opts(PLANTED, &["*planted_violation*"]))?;
    ensure!(section(&p, "bringup_pagetoy::planted_violation")?.placement == "paged", "plant not paged");
    let pa = artefacts(PLANTED);
    let r = resident::check(&pa.elf, &pa.map, BIN)?;
    resident::print(&r);
    ensure!(r.violations.len() == 1, "expected one violation, got {:?}", r.violations);
    let chain = r.violations[0].join(" -> ");
    ensure!(
        chain.starts_with("toy_memmanage -> bringup_pagetoy::planted_violation"),
        "unexpected chain: {chain}"
    );
    log(&format!("  planted violation caught: {chain}"));

    Ok(format!(
        "pagelink self-test passed ({} code + {} rodata pages, {checked} bytes replayed)",
        l.pages.code, l.pages.rodata
    ))
}
