//! Identification and setup headers (Vorbis I §4.2, §3, §6–§8), parsed in
//! two passes over the same packet: the first only counts (so the arena
//! need is known exactly), the second fills arena-resident tables.
//!
//! Codebooks keep a binary decode tree (`[u16; 2]` per internal node, leaf
//! = `0x8000 | entry`) and the VQ multiplicands unexpanded: no per-entry
//! value vectors, so a set-up of ~40 books costs ~15–20 KiB.

use bytemuck::{Pod, Zeroable};

use crate::bits::{Bits, ilog};

pub(crate) const MAX_CHANNELS: usize = 8;
pub(crate) const MAX_X: usize = 65;
const LEAF: u16 = 0x8000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupError {
    Invalid,
    /// Valid but outside what this decoder supports (floor 0, huge books).
    Unsupported,
}

pub(crate) type R<T> = Result<T, SetupError>;

fn check(c: bool) -> R<()> {
    if c { Ok(()) } else { Err(SetupError::Invalid) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ident {
    pub channels: u8,
    pub rate: u32,
    pub bitrate: u32,
    /// log2 of the short and long block sizes.
    pub bs: [u32; 2],
}

pub(crate) fn ident(b: &mut Bits<'_>) -> R<Ident> {
    check(b.read(8) == 1)?;
    for &c in b"vorbis" {
        check(b.read(8) == u32::from(c))?;
    }
    check(b.read(32) == 0)?;
    let channels = b.read(8) as u8;
    let rate = b.read(32);
    let _max = b.read(32);
    let nominal = b.read(32);
    let _min = b.read(32);
    let bs0 = b.read(4);
    let bs1 = b.read(4);
    check(b.bit() && !b.eop)?;
    check(channels > 0 && rate > 0 && (6..=13).contains(&bs0) && (6..=13).contains(&bs1) && bs0 <= bs1)?;
    if usize::from(channels) > MAX_CHANNELS {
        return Err(SetupError::Unsupported);
    }
    Ok(Ident { channels, rate, bitrate: if (nominal as i32) > 0 { nominal } else { 0 }, bs: [bs0, bs1] })
}

#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct Book {
    pub entries: u32,
    pub nodes: u32,
    pub nnodes: u32,
    /// The only used entry of a one-entry book, else `u32::MAX`.
    pub single: u32,
    pub vals: u32,
    pub nvals: u32,
    pub min: f32,
    pub delta: f32,
    pub dims: u16,
    pub lookup: u8,
    pub seq_p: u8,
}

#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct Floor1 {
    pub subbooks: [[i16; 8]; 16],
    pub x: [u16; MAX_X],
    pub values: u8,
    pub partitions: u8,
    pub class: [u8; 32],
    pub dims: [u8; 16],
    pub subs: [u8; 16],
    pub master: [u8; 16],
    pub sorted: [u8; MAX_X],
    pub low: [u8; MAX_X],
    pub high: [u8; MAX_X],
    pub multiplier: u8,
    pub _pad: [u8; 2],
}

#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct Residue {
    pub books: [[i16; 8]; 64],
    pub begin: u32,
    pub end: u32,
    pub psize: u32,
    pub kind: u8,
    pub classes: u8,
    pub classbook: u8,
    pub _pad: u8,
}

#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct Mapping {
    pub mag: [u8; 16],
    pub ang: [u8; 16],
    pub mux: [u8; MAX_CHANNELS],
    pub floor: [u8; 16],
    pub residue: [u8; 16],
    pub submaps: u8,
    pub steps: u8,
    pub _pad: [u8; 2],
}

#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct Mode {
    pub long: u8,
    pub mapping: u8,
}

/// What the set-up needs (pass 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sizes {
    /// Used entries per codebook.
    pub used: [u16; 256],
    pub books: usize,
    pub nodes: usize,
    pub vals: usize,
    pub floors: usize,
    pub residues: usize,
    pub mappings: usize,
    pub modes: usize,
    /// Residue classification scratch bytes.
    pub classes: usize,
}

/// The tables (pass 2).
pub(crate) struct Setup<'a> {
    pub books: &'a mut [Book],
    pub nodes: &'a mut [[u16; 2]],
    pub vals: &'a mut [u16],
    pub floors: &'a mut [Floor1],
    pub residues: &'a mut [Residue],
    pub mappings: &'a mut [Mapping],
    pub modes: &'a mut [Mode],
}

fn lookup1_values(entries: u32, dims: u32) -> u32 {
    if dims == 0 {
        return 0;
    }
    // Largest r with r^dims <= entries.
    let mut r = 1u32;
    loop {
        let mut p: u64 = 1;
        for _ in 0..dims {
            p = p.saturating_mul(u64::from(r + 1));
            if p > u64::from(entries) {
                break;
            }
        }
        if p > u64::from(entries) {
            return r;
        }
        r += 1;
    }
}

/// Insert codeword `cw` (MSB first, `len` bits) for `entry` into `tree`.
fn insert(tree: &mut [[u16; 2]], used: &mut usize, cw: u32, len: u32, entry: u32) -> R<()> {
    let mut node = 0usize;
    for i in (0..len).rev() {
        let bit = ((cw >> i) & 1) as usize;
        let slot = tree.get(node).map(|n| n[bit]).ok_or(SetupError::Invalid)?;
        if i == 0 {
            check(slot == 0)?;
            tree[node][bit] = LEAF | entry as u16;
            return Ok(());
        }
        if slot == 0 {
            check(*used < tree.len())?;
            let new = *used;
            *used += 1;
            tree[node][bit] = new as u16;
            node = new;
        } else {
            check(slot & LEAF == 0)?;
            node = usize::from(slot);
        }
    }
    Ok(())
}

/// Codeword assignment (Vorbis I §3.2.1): each used entry takes the
/// lowest free codeword of its length.
struct Assign {
    avail: [u32; 33],
    first: bool,
}

impl Assign {
    fn new() -> Self {
        Self { avail: [0; 33], first: true }
    }

    /// The next codeword (left-aligned in 32 bits) for length `len`.
    fn next(&mut self, len: u32) -> R<u32> {
        let len = len as usize;
        if self.first {
            self.first = false;
            for i in 1..=len {
                self.avail[i] = if i == 32 { 1 } else { 1u32 << (32 - i) };
            }
            return Ok(0);
        }
        let mut z = len;
        while z > 0 && self.avail[z] == 0 {
            z -= 1;
        }
        check(z > 0)?;
        let res = self.avail[z];
        self.avail[z] = 0;
        let mut y = len;
        while y > z {
            self.avail[y] = res.wrapping_add(if y == 32 { 1 } else { 1u32 << (32 - y) });
            y -= 1;
        }
        Ok(res)
    }
}

impl Default for Sizes {
    fn default() -> Self {
        Self {
            used: [0; 256],
            books: 0,
            nodes: 0,
            vals: 0,
            floors: 0,
            residues: 0,
            mappings: 0,
            modes: 0,
            classes: 0,
        }
    }
}

/// Nodes of a book's tree for `used` entries.
fn tree_nodes(used: u16) -> usize {
    usize::from(used.max(2)) - 1
}

/// Parse the setup header; with `out`, fill the tables (`pass1` from a
/// previous counting pass over the same bytes).
pub(crate) fn setup(b: &mut Bits<'_>, id: &Ident, mut out: Option<(&mut Setup<'_>, &Sizes)>) -> R<Sizes> {
    let ch = usize::from(id.channels);
    check(b.read(8) == 5)?;
    for &c in b"vorbis" {
        check(b.read(8) == u32::from(c))?;
    }
    let mut sz = Sizes { books: b.read(8) as usize + 1, ..Sizes::default() };
    let (mut node_at, mut val_at) = (0usize, 0usize);
    for bi in 0..sz.books {
        check(b.read(24) == 0x56_4342)?;
        let dims = b.read(16);
        let entries = b.read(24);
        check(!b.eop)?;
        if entries > 0x7FFF && entries != 0 {
            // Leaf indices are 15 bits.
            return Err(SetupError::Unsupported);
        }
        let ordered = b.bit();
        let mut assign = Assign::new();
        let mut used = 0usize;
        let mut single = u32::MAX;
        let known = out.as_ref().map(|(_, p1)| p1.used[bi]);
        let mut tree_used = 1usize;
        let mut emit = |e: u32, len: u32, out: &mut Option<(&mut Setup<'_>, &Sizes)>| -> R<()> {
            check((1..=32).contains(&len))?;
            used += 1;
            single = if used == 1 { e } else { u32::MAX };
            let cw = assign.next(len)?;
            if let (Some((o, _)), Some(k)) = (out.as_mut(), known)
                && k > 1
            {
                let tree = o.nodes.get_mut(node_at..node_at + tree_nodes(k)).ok_or(SetupError::Invalid)?;
                insert(tree, &mut tree_used, if len == 32 { cw } else { cw >> (32 - len) }, len, e)?;
            }
            Ok(())
        };
        if ordered {
            let mut len = b.read(5) + 1;
            let mut e = 0u32;
            while e < entries {
                let n = b.read(ilog(entries - e));
                check(e + n <= entries && len <= 32 && !b.eop)?;
                for k in 0..n {
                    emit(e + k, len, &mut out)?;
                }
                e += n;
                len += 1;
            }
        } else {
            let sparse = b.bit();
            for e in 0..entries {
                if !sparse || b.bit() {
                    let len = b.read(5) + 1;
                    emit(e, len, &mut out)?;
                }
                check(!b.eop)?;
            }
        }
        let lookup = b.read(4);
        check(used <= 0xFFFF)?;
        if bi >= 256 {
            return Err(SetupError::Invalid);
        }
        sz.used[bi] = used as u16;
        if let Some(k) = known {
            check(usize::from(k) == used)?;
        }
        let mut book = Book {
            entries,
            nodes: node_at as u32,
            nnodes: tree_nodes(used as u16) as u32,
            single: if used == 1 { single } else { u32::MAX },
            vals: val_at as u32,
            nvals: 0,
            min: 0.0,
            delta: 0.0,
            dims: dims as u16,
            lookup: lookup as u8,
            seq_p: 0,
        };
        match lookup {
            0 => {}
            1 | 2 => {
                book.min = b.float32();
                book.delta = b.float32();
                let bits = b.read(4) + 1;
                book.seq_p = u8::from(b.bit());
                let n =
                    if lookup == 1 { lookup1_values(entries, dims) } else { entries.saturating_mul(dims) };
                check(dims > 0 && n <= 1 << 16)?;
                book.nvals = n;
                for i in 0..n as usize {
                    let v = b.read(bits);
                    if let Some((o, _)) = out.as_mut() {
                        *o.vals.get_mut(val_at + i).ok_or(SetupError::Invalid)? = v as u16;
                    }
                }
                check(!b.eop && bits <= 16)?;
                val_at += n as usize;
            }
            _ => return Err(SetupError::Invalid),
        }
        if let Some((o, _)) = out.as_mut() {
            *o.books.get_mut(bi).ok_or(SetupError::Invalid)? = book;
        }
        node_at += book.nnodes as usize;
    }
    sz.nodes = node_at;
    sz.vals = val_at;
    // Time-domain transforms: placeholders, all zero.
    let n = b.read(6) + 1;
    for _ in 0..n {
        check(b.read(16) == 0)?;
    }
    let books = sz.books;
    let book_ok = |out: &Option<(&mut Setup<'_>, &Sizes)>, i: u32, need_vq: bool| -> R<()> {
        check((i as usize) < books)?;
        if need_vq && let Some((o, _)) = out.as_ref() {
            check(o.books[i as usize].lookup != 0)?;
        }
        Ok(())
    };
    sz.floors = b.read(6) as usize + 1;
    for fi in 0..sz.floors {
        match b.read(16) {
            1 => {}
            0 => return Err(SetupError::Unsupported),
            _ => return Err(SetupError::Invalid),
        }
        let mut f = Floor1::zeroed();
        f.partitions = b.read(5) as u8;
        let mut max_class: i32 = -1;
        for i in 0..usize::from(f.partitions) {
            f.class[i] = b.read(4) as u8;
            max_class = max_class.max(i32::from(f.class[i]));
        }
        for c in 0..(max_class + 1) as usize {
            f.dims[c] = b.read(3) as u8 + 1;
            f.subs[c] = b.read(2) as u8;
            if f.subs[c] != 0 {
                f.master[c] = b.read(8) as u8;
                book_ok(&out, u32::from(f.master[c]), false)?;
            }
            for j in 0..(1usize << f.subs[c]) {
                let v = b.read(8) as i16 - 1;
                if v >= 0 {
                    book_ok(&out, v as u32, false)?;
                }
                f.subbooks[c][j] = v;
            }
        }
        f.multiplier = b.read(2) as u8 + 1;
        let rbits = b.read(4);
        f.x[0] = 0;
        f.x[1] = (1u32 << rbits) as u16;
        let mut n = 2usize;
        for i in 0..usize::from(f.partitions) {
            let c = usize::from(f.class[i]);
            for _ in 0..f.dims[c] {
                check(n < MAX_X)?;
                f.x[n] = b.read(rbits) as u16;
                n += 1;
            }
        }
        check(!b.eop)?;
        f.values = n as u8;
        // Sorted order and neighbours.
        let mut idx = [0u8; MAX_X];
        for (i, v) in idx.iter_mut().enumerate().take(n) {
            *v = i as u8;
        }
        idx[..n].sort_unstable_by_key(|&i| f.x[usize::from(i)]);
        for w in idx[..n].windows(2) {
            check(f.x[usize::from(w[0])] != f.x[usize::from(w[1])])?;
        }
        f.sorted = idx;
        for i in 2..n {
            let xi = f.x[i];
            let (mut lo, mut hi) = (0usize, 1usize);
            let (mut lx, mut hx) = (0u16, u16::MAX);
            for j in 0..i {
                let xj = f.x[j];
                if xj < xi && xj >= lx {
                    lx = xj;
                    lo = j;
                }
                if xj > xi && xj <= hx {
                    hx = xj;
                    hi = j;
                }
            }
            f.low[i] = lo as u8;
            f.high[i] = hi as u8;
        }
        if let Some((o, _)) = out.as_mut() {
            *o.floors.get_mut(fi).ok_or(SetupError::Invalid)? = f;
        }
    }
    sz.residues = b.read(6) as usize + 1;
    let half_long = 1usize << (id.bs[1] - 1);
    for ri in 0..sz.residues {
        let mut r = Residue::zeroed();
        let kind = b.read(16);
        check(kind <= 2)?;
        r.kind = kind as u8;
        r.begin = b.read(24);
        r.end = b.read(24);
        r.psize = b.read(24) + 1;
        r.classes = b.read(6) as u8 + 1;
        r.classbook = b.read(8) as u8;
        book_ok(&out, u32::from(r.classbook), false)?;
        let mut cascade = [0u8; 64];
        for c in cascade.iter_mut().take(usize::from(r.classes)) {
            let low = b.read(3) as u8;
            let high = if b.bit() { b.read(5) as u8 } else { 0 };
            *c = (high << 3) | low;
        }
        for c in 0..usize::from(r.classes) {
            for j in 0..8 {
                r.books[c][j] = if cascade[c] & (1 << j) != 0 {
                    let v = b.read(8);
                    book_ok(&out, v, true)?;
                    v as i16
                } else {
                    -1
                };
            }
        }
        check(!b.eop)?;
        // Classification scratch: one entry per partition per vector.
        let vec_len = if kind == 2 { half_long * ch } else { half_long };
        let end = (r.end as usize).min(vec_len);
        let begin = (r.begin as usize).min(end);
        let parts = (end - begin) / r.psize as usize + 64;
        let vectors = if kind == 2 { 1 } else { ch };
        sz.classes = sz.classes.max(parts * vectors);
        if let Some((o, _)) = out.as_mut() {
            check(o.books[usize::from(r.classbook)].dims > 0)?;
            *o.residues.get_mut(ri).ok_or(SetupError::Invalid)? = r;
        }
    }
    sz.mappings = b.read(6) as usize + 1;
    for mi in 0..sz.mappings {
        check(b.read(16) == 0)?;
        let mut m = Mapping::zeroed();
        m.submaps = if b.bit() { b.read(4) as u8 + 1 } else { 1 };
        if b.bit() {
            let steps = b.read(8) as usize + 1;
            if steps > m.mag.len() {
                return Err(SetupError::Unsupported);
            }
            m.steps = steps as u8;
            let bits = ilog(ch as u32 - 1);
            for s in 0..steps {
                m.mag[s] = b.read(bits) as u8;
                m.ang[s] = b.read(bits) as u8;
                check(m.mag[s] != m.ang[s] && usize::from(m.mag[s]) < ch && usize::from(m.ang[s]) < ch)?;
            }
        }
        check(b.read(2) == 0)?;
        if m.submaps > 1 {
            for c in 0..ch {
                m.mux[c] = b.read(4) as u8;
                check(m.mux[c] < m.submaps)?;
            }
        }
        for s in 0..usize::from(m.submaps) {
            let _time = b.read(8);
            m.floor[s] = b.read(8) as u8;
            m.residue[s] = b.read(8) as u8;
            check(usize::from(m.floor[s]) < sz.floors && usize::from(m.residue[s]) < sz.residues)?;
        }
        if let Some((o, _)) = out.as_mut() {
            *o.mappings.get_mut(mi).ok_or(SetupError::Invalid)? = m;
        }
    }
    sz.modes = b.read(6) as usize + 1;
    for mo in 0..sz.modes {
        let long = b.read(1) as u8;
        check(b.read(16) == 0 && b.read(16) == 0)?;
        let mapping = b.read(8) as u8;
        check(usize::from(mapping) < sz.mappings)?;
        if let Some((o, _)) = out.as_mut() {
            *o.modes.get_mut(mo).ok_or(SetupError::Invalid)? = Mode { long, mapping };
        }
    }
    check(b.bit() && !b.eop)?;
    Ok(sz)
}

impl Book {
    /// Decode one entry number; `None` on an invalid code or end of packet.
    #[inline]
    pub(crate) fn decode(&self, nodes: &[[u16; 2]], b: &mut Bits<'_>) -> Option<u32> {
        if self.single != u32::MAX {
            b.read(1);
            return (!b.eop).then_some(self.single);
        }
        let tree = nodes.get(self.nodes as usize..(self.nodes + self.nnodes) as usize)?;
        let mut node = 0usize;
        for _ in 0..32 {
            let child = tree.get(node)?[usize::from(b.bit())];
            if b.eop || child == 0 {
                return None;
            }
            if child & LEAF != 0 {
                return Some(u32::from(child & !LEAF));
            }
            node = usize::from(child);
        }
        None
    }

    /// Call `f(j, value)` for each of the `dims` components of VQ entry `e`.
    #[inline]
    pub(crate) fn vq(&self, vals: &[u16], e: u32, mut f: impl FnMut(usize, f32)) {
        let Some(v) = vals.get(self.vals as usize..(self.vals + self.nvals) as usize) else { return };
        let mut last = 0.0f32;
        if self.lookup == 1 {
            let n = self.nvals.max(1);
            let mut div = 1u32;
            for j in 0..usize::from(self.dims) {
                let idx = (e / div) % n;
                let x = f32::from(v[idx as usize]) * self.delta + self.min + last;
                if self.seq_p != 0 {
                    last = x;
                }
                f(j, x);
                div = div.saturating_mul(n);
            }
        } else {
            let base = e as usize * usize::from(self.dims);
            for j in 0..usize::from(self.dims) {
                let x = f32::from(v.get(base + j).copied().unwrap_or(0)) * self.delta + self.min + last;
                if self.seq_p != 0 {
                    last = x;
                }
                f(j, x);
            }
        }
    }
}
