//! The exception lexicon: the words whose pronunciation the rules (or the
//! stress heuristic) get wrong, chosen by frequency within a size budget
//! from CMUdict by `cargo xtask tts-lexicon` and committed as
//! `src/lexicon.bin` (CMUdict's notice: `THIRD-PARTY.md`).
//!
//! # Format (version 1)
//!
//! ```text
//! "OTL1"
//! u16 entries, u16 blocks, u8 entries per block, u8 block alignment (log2)
//! code: u8 symbols, then per symbol (u8 symbol, u8 code length)  — letters
//! code: the same                                                  — phones
//! u16 × blocks: byte offset of each block in the data, shifted right by the alignment
//! data
//! ```
//!
//! Words are sorted (bytewise, lower-case `a`–`z` and `'`) and cut into
//! blocks; a block starts on a byte. An entry is a 4-bit count of letters
//! shared with the previous word of the block (0 for a block's first
//! word), the remaining letters in the letter code and [`END`], then the
//! pronunciation in the phone code: [`Phone::to_u8`] symbols and [`END`],
//! or [`STRESS_ONLY`] and a 4-bit vowel index (the rules' phonemes are right
//! and only the primary stress moves to that vowel). Codes are canonical
//! Huffman codes (at most 15 bits, stored sorted by length then symbol),
//! bits read most significant first.
//!
//! A lookup is a binary search over the blocks' first words and a scan of
//! at most one block.
//!
//! On the player read-only data is paged in groups of 16 KiB, and a
//! static larger than that cannot be paged (`xtask pagelink` refuses it):
//! the built-in lexicon (32,766 B) is linked as three statics of at most
//! [`PIECE_MAX`] bytes cut at block starts ([`split_points`]); a block
//! never straddles two pieces ([`Lexicon::builtin`], TTS-3).

use crate::phoneme::{Ph, Phone, Stress};

/// End of a word or a pronunciation (both codes).
pub const END: u8 = 0xFF;
/// A stress-only entry follows (phone code).
pub const STRESS_ONLY: u8 = 0xFE;
/// Longest word the lexicon stores.
pub const MAX_WORD: usize = 32;
/// Longest pronunciation the lexicon stores.
pub const MAX_PHONES: usize = 40;

/// The committed lexicon as one slice (the tests and the tool; the
/// firmware links only the pieces of [`Lexicon::builtin`]).
pub static DATA: &[u8] = RAW;

const RAW: &[u8] = include_bytes!("lexicon.bin");

/// Largest piece of the built-in lexicon (a rodata group on the player).
pub const PIECE_MAX: usize = 16 * 1024;

const fn rd16(d: &[u8], at: usize) -> usize {
    d[at] as usize | (d[at + 1] as usize) << 8
}

/// Where the data of a lexicon file starts (after the header, the two
/// codes and the block index).
const fn data_offset(d: &[u8]) -> usize {
    let nl = d[10] as usize;
    let np = d[11 + 2 * nl] as usize;
    12 + 2 * nl + 2 * np + 2 * rd16(d, 6)
}

/// Where a lexicon file is cut into its three pieces (offsets in the
/// file): each cut at the start of a block, each piece at most
/// [`PIECE_MAX`]. Fails the build when that cannot be done.
#[must_use]
pub const fn split_points(d: &[u8]) -> [usize; 2] {
    if d.len() <= PIECE_MAX {
        return [d.len(), d.len()];
    }
    let blocks = rd16(d, 6);
    let shift = d[9];
    let nl = d[10] as usize;
    let np = d[11 + 2 * nl] as usize;
    let index = 12 + 2 * nl + 2 * np;
    let data = data_offset(d);
    let mut cuts = [0usize; 2];
    let mut from = 0;
    let mut c = 0;
    while c < 2 {
        let mut best = from;
        let mut b = 0;
        while b < blocks {
            let at = data + (rd16(d, index + 2 * b) << shift);
            if at > best && at <= from + PIECE_MAX {
                best = at;
            }
            b += 1;
        }
        if d.len() <= from + PIECE_MAX {
            best = d.len();
        }
        assert!(best > from, "a lexicon block is larger than a piece");
        cuts[c] = best;
        from = best;
        c += 1;
    }
    assert!(d.len() - from <= PIECE_MAX, "the lexicon needs more than three pieces");
    cuts
}

/// Bytes `from..from + N` of `d` as an array (a piece of the lexicon).
#[must_use]
pub const fn piece<const N: usize>(d: &[u8], from: usize) -> [u8; N] {
    let mut out = [0u8; N];
    let mut i = 0;
    while i < N {
        out[i] = d[from + i];
        i += 1;
    }
    out
}

const CUTS: [usize; 2] = split_points(RAW);
static PART0: [u8; CUTS[0]] = piece(RAW, 0);
static PART1: [u8; CUTS[1] - CUTS[0]] = piece(RAW, CUTS[0]);
static PART2: [u8; RAW.len() - CUTS[1]] = piece(RAW, CUTS[1]);

/// The entries' bytes, in up to three pieces.
#[derive(Clone, Copy)]
struct Data<'a> {
    parts: [&'a [u8]; 3],
    /// Offset of the second and third piece in the data.
    at: [usize; 2],
}

impl<'a> Data<'a> {
    const EMPTY: Data<'static> = Data { parts: [&[], &[], &[]], at: [0, 0] };

    fn one(d: &'a [u8]) -> Data<'a> {
        Data { parts: [d, &[], &[]], at: [d.len(), d.len()] }
    }

    fn len(&self) -> usize {
        self.at[1] + self.parts[2].len()
    }

    #[inline]
    fn get(&self, i: usize) -> Option<u8> {
        if i < self.at[0] {
            self.parts[0].get(i).copied()
        } else if i < self.at[1] {
            self.parts[1].get(i - self.at[0]).copied()
        } else {
            self.parts[2].get(i - self.at[1]).copied()
        }
    }
}

/// What a lookup found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// The full pronunciation, `n` phones written to the caller's buffer.
    Full(usize),
    /// The rules' phonemes with the primary stress on this vowel (0-based).
    Stress(u8),
}

struct Bits<'a> {
    d: Data<'a>,
    pos: usize, // bit position
}

impl Bits<'_> {
    fn bit(&mut self) -> u32 {
        let b = self.d.get(self.pos / 8).unwrap_or(0);
        let r = (b >> (7 - (self.pos % 8))) & 1;
        self.pos += 1;
        u32::from(r)
    }

    fn bits(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit();
        }
        v
    }
}

/// A canonical Huffman code as stored: (symbol, length) pairs sorted by
/// (length, symbol), with the count of codes per length precomputed.
#[derive(Clone, Copy)]
struct Code<'a> {
    table: &'a [u8],
    count: [u8; 16],
}

impl<'a> Code<'a> {
    const EMPTY: Code<'static> = Code { table: &[], count: [0; 16] };

    fn new(table: &'a [u8]) -> Option<Code<'a>> {
        let mut count = [0u8; 16];
        let mut prev = 0;
        for pair in table.chunks_exact(2) {
            let len = pair[1];
            if len == 0 || len > 15 || len < prev {
                return None;
            }
            prev = len;
            count[usize::from(len)] = count[usize::from(len)].checked_add(1)?;
        }
        Some(Code { table, count })
    }

    fn decode(&self, r: &mut Bits) -> Option<u8> {
        // Past the data: stop (zero bits would decode forever).
        if r.pos >= 8 * r.d.len() {
            return None;
        }
        let mut code: u32 = 0;
        let mut first: u32 = 0;
        let mut index = 0usize; // index of the first symbol of this length
        for len in 1..=15 {
            code |= r.bit();
            let count = u32::from(self.count[len]);
            let k = code.wrapping_sub(first);
            if k < count {
                return self.table.get(2 * (index + k as usize)).copied();
            }
            index += count as usize;
            first = (first + count) << 1;
            code <<= 1;
        }
        None
    }
}

/// A parsed lexicon over its bytes.
#[derive(Clone, Copy)]
pub struct Lexicon<'a> {
    entries: u16,
    blocks: u16,
    per_block: u16,
    shift: u8,
    letters: Code<'a>,
    phones: Code<'a>,
    index: &'a [u8],
    data: Data<'a>,
}

impl core::fmt::Debug for Lexicon<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Lexicon").field("entries", &self.entries).field("blocks", &self.blocks).finish()
    }
}

impl<'a> Lexicon<'a> {
    /// Parses the header; `None` if it is malformed.
    pub fn new(d: &'a [u8]) -> Option<Lexicon<'a>> {
        Self::parse(d, [&[], &[]])
    }

    /// Parses a lexicon cut into pieces at block starts: `head` holds the
    /// header, the index and the first blocks, `rest` the others in order.
    pub fn new_split(head: &'a [u8], rest: [&'a [u8]; 2]) -> Option<Lexicon<'a>> {
        Self::parse(head, rest)
    }

    fn parse(d: &'a [u8], rest: [&'a [u8]; 2]) -> Option<Lexicon<'a>> {
        if d.get(..4)? != b"OTL1" {
            return None;
        }
        let entries = u16::from_le_bytes([*d.get(4)?, *d.get(5)?]);
        let blocks = u16::from_le_bytes([*d.get(6)?, *d.get(7)?]);
        let per_block = u16::from(*d.get(8)?);
        if per_block == 0 || usize::from(entries) > usize::from(blocks) * usize::from(per_block) {
            return None;
        }
        let shift = *d.get(9)?;
        if shift > 7 {
            return None;
        }
        let mut p = 10;
        let nl = usize::from(*d.get(p)?);
        let letters = Code::new(d.get(p + 1..p + 1 + 2 * nl)?)?;
        p += 1 + 2 * nl;
        let np = usize::from(*d.get(p)?);
        let phones = Code::new(d.get(p + 1..p + 1 + 2 * np)?)?;
        p += 1 + 2 * np;
        let index = d.get(p..p + 2 * usize::from(blocks))?;
        let first = d.get(p + 2 * usize::from(blocks)..)?;
        let mut data = Data::one(first);
        data.parts[1] = rest[0];
        data.parts[2] = rest[1];
        data.at[1] = first.len() + rest[0].len();
        let lex = Lexicon { entries, blocks, per_block, shift, letters, phones, index, data };
        if (0..usize::from(blocks)).any(|b| lex.block_start(b) > data.len()) {
            return None;
        }
        Some(lex)
    }

    /// A lexicon without words.
    pub const fn empty() -> Lexicon<'static> {
        Lexicon {
            entries: 0,
            blocks: 0,
            per_block: 1,
            shift: 0,
            letters: Code::EMPTY,
            phones: Code::EMPTY,
            index: &[],
            data: Data::EMPTY,
        }
    }

    /// The committed lexicon (checked by the tests; empty if it were ever
    /// malformed).
    pub fn builtin() -> Lexicon<'static> {
        Lexicon::new_split(&PART0, [&PART1, &PART2]).unwrap_or(Lexicon::empty())
    }

    /// Number of words.
    pub fn len(&self) -> usize {
        usize::from(self.entries)
    }

    /// Whether it holds no words.
    pub fn is_empty(&self) -> bool {
        self.entries == 0
    }

    fn block_start(&self, b: usize) -> usize {
        usize::from(u16::from_le_bytes([self.index[2 * b], self.index[2 * b + 1]])) << self.shift
    }

    /// Entries in block `b`.
    fn block_len(&self, b: usize) -> usize {
        let per = usize::from(self.per_block);
        let all = usize::from(self.entries);
        (all - (b * per).min(all)).min(per)
    }

    /// Reads one entry's word into `word` (which holds the previous word
    /// for the shared prefix); returns its length, or `None` past the data.
    fn read_word(&self, r: &mut Bits, word: &mut [u8; MAX_WORD], prev_len: usize) -> Option<usize> {
        let shared = (r.bits(4) as usize).min(prev_len);
        let mut n = shared;
        loop {
            let s = self.letters.decode(r)?;
            if s == END {
                return Some(n);
            }
            if n < MAX_WORD {
                word[n] = s;
                n += 1;
            }
        }
    }

    /// Reads (or skips, with `out` empty) one pronunciation.
    fn read_pron(&self, r: &mut Bits, out: &mut [Phone]) -> Option<Entry> {
        let mut n = 0;
        loop {
            let s = self.phones.decode(r)?;
            if s == END {
                return Some(Entry::Full(n));
            }
            if s == STRESS_ONLY {
                return Some(Entry::Stress(r.bits(4) as u8));
            }
            if n < out.len()
                && let Some(p) = Phone::from_u8(s)
            {
                out[n] = p;
                n += 1;
            }
        }
    }

    fn first_word(&self, b: usize, word: &mut [u8; MAX_WORD]) -> Option<usize> {
        let mut r = Bits { d: self.data, pos: 8 * self.block_start(b) };
        self.read_word(&mut r, word, 0)
    }

    /// Looks `key` (lower case) up; on a full entry the phones are written
    /// to `out`.
    pub fn lookup(&self, key: &[u8], out: &mut [Phone]) -> Option<Entry> {
        if self.blocks == 0 || key.is_empty() || key.len() > MAX_WORD {
            return None;
        }
        let mut word = [0u8; MAX_WORD];
        // The last block whose first word is <= key.
        let (mut lo, mut hi) = (0usize, usize::from(self.blocks));
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            let n = self.first_word(mid, &mut word)?;
            if &word[..n] <= key {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let mut r = Bits { d: self.data, pos: 8 * self.block_start(lo) };
        let mut len = 0;
        let mut skip: [Phone; 0] = [];
        for _ in 0..self.block_len(lo) {
            len = self.read_word(&mut r, &mut word, len)?;
            match word[..len].cmp(key) {
                core::cmp::Ordering::Less => {
                    self.read_pron(&mut r, &mut skip)?;
                }
                core::cmp::Ordering::Equal => return self.read_pron(&mut r, out),
                core::cmp::Ordering::Greater => return None,
            }
        }
        None
    }

    /// Calls `f(word, entry, phones)` for every entry in order (tests and
    /// the tool's statistics).
    pub fn for_each(&self, mut f: impl FnMut(&[u8], Entry, &[Phone])) {
        let mut word = [0u8; MAX_WORD];
        let mut out = [Phone::new(Ph::AH, Stress::None); MAX_PHONES];
        for b in 0..usize::from(self.blocks) {
            let mut r = Bits { d: self.data, pos: 8 * self.block_start(b) };
            let mut len = 0;
            for _ in 0..self.block_len(b) {
                let Some(l) = self.read_word(&mut r, &mut word, len) else { return };
                len = l;
                let Some(e) = self.read_pron(&mut r, &mut out) else { return };
                let n = match e {
                    Entry::Full(n) => n,
                    Entry::Stress(_) => 0,
                };
                f(&word[..len], e, &out[..n]);
            }
        }
    }
}

/// The encoder (`build` feature; used by `cargo xtask tts-lexicon`).
#[cfg(feature = "build")]
pub mod build {
    extern crate std;
    use super::{END, MAX_PHONES, MAX_WORD, STRESS_ONLY};
    use crate::phoneme::Phone;
    use std::collections::BTreeMap;
    use std::vec;
    use std::vec::Vec;

    /// One entry to encode.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Pron {
        /// The full pronunciation.
        Full(Vec<Phone>),
        /// Rules' phonemes, primary stress on this vowel.
        Stress(u8),
    }

    /// Code lengths (≤ 15) for symbol frequencies, Huffman's algorithm with
    /// the frequencies flattened until the longest code fits; sorted by
    /// (length, symbol).
    pub fn code_lengths(freq: &BTreeMap<u8, u64>) -> Vec<(u8, u8)> {
        let mut scale = 0u32;
        loop {
            let syms: Vec<(u8, u64)> = freq.iter().map(|(&s, &f)| (s, (f >> scale).max(1))).collect();
            if syms.len() == 1 {
                return vec![(syms[0].0, 1)];
            }
            let mut nodes: Vec<(u64, Vec<u8>)> = syms.iter().map(|&(s, f)| (f, vec![s])).collect();
            let mut depth: BTreeMap<u8, u8> = syms.iter().map(|&(s, _)| (s, 0)).collect();
            while nodes.len() > 1 {
                nodes.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
                let (Some(a), Some(b)) = (nodes.pop(), nodes.pop()) else { break };
                for s in a.1.iter().chain(&b.1) {
                    if let Some(d) = depth.get_mut(s) {
                        *d += 1;
                    }
                }
                let mut m = a.1;
                m.extend(b.1);
                nodes.push((a.0 + b.0, m));
            }
            if depth.values().all(|&d| d <= 15) {
                let mut v: Vec<(u8, u8)> = depth.into_iter().collect();
                v.sort_by_key(|&(s, l)| (l, s));
                return v;
            }
            scale += 1;
        }
    }

    /// Canonical codes for a length table sorted by (length, symbol).
    fn codes(lengths: &[(u8, u8)]) -> BTreeMap<u8, (u32, u8)> {
        let mut m = BTreeMap::new();
        let mut code = 0u32;
        let mut prev_len = lengths.first().map_or(0, |x| x.1);
        for (i, &(s, l)) in lengths.iter().enumerate() {
            if i > 0 {
                code = (code + 1) << (l - prev_len);
            }
            prev_len = l;
            m.insert(s, (code, l));
        }
        m
    }

    struct BitWriter {
        out: Vec<u8>,
        n: usize,
    }

    impl BitWriter {
        fn put(&mut self, v: u32, len: u8) {
            for k in (0..len).rev() {
                if self.n % 8 == 0 {
                    self.out.push(0);
                }
                let bit = ((v >> k) & 1) as u8;
                let last = self.out.len() - 1;
                self.out[last] |= bit << (7 - (self.n % 8));
                self.n += 1;
            }
        }

        fn align(&mut self) {
            self.n = self.out.len() * 8;
        }
    }

    fn pron_symbols(p: &Pron) -> Vec<u8> {
        match p {
            Pron::Full(v) => v.iter().take(MAX_PHONES).map(|p| p.to_u8()).chain([END]).collect(),
            Pron::Stress(_) => vec![STRESS_ONLY],
        }
    }

    fn shared_len(a: &[u8], b: &[u8]) -> usize {
        a.iter().zip(b).take_while(|(x, y)| x == y).count().min(15)
    }

    /// Encodes `entries` (words lower case `a`–`z`/`'`; longer than
    /// [`MAX_WORD`] are left out) with `per_block` (1–255) entries per block.
    pub fn encode(entries: &BTreeMap<Vec<u8>, Pron>, per_block: usize) -> Vec<u8> {
        let per_block = per_block.clamp(1, 255);
        let words: Vec<(&Vec<u8>, &Pron)> =
            entries.iter().filter(|(w, _)| !w.is_empty() && w.len() <= MAX_WORD).collect();
        let shared = |i: usize| if i % per_block == 0 { 0 } else { shared_len(words[i - 1].0, words[i].0) };
        // Symbol statistics for both codes.
        let mut lf: BTreeMap<u8, u64> = BTreeMap::new();
        let mut pf: BTreeMap<u8, u64> = BTreeMap::new();
        for (i, (w, p)) in words.iter().enumerate() {
            for &c in &w[shared(i)..] {
                *lf.entry(c).or_default() += 1;
            }
            *lf.entry(END).or_default() += 1;
            for s in pron_symbols(p) {
                *pf.entry(s).or_default() += 1;
            }
        }
        lf.entry(END).or_insert(1);
        pf.entry(END).or_insert(1);
        let ll = code_lengths(&lf);
        let pl = code_lengths(&pf);
        let lc = codes(&ll);
        let pc = codes(&pl);
        // The smallest block alignment whose offsets fit 16 bits.
        let mut shift = 0u8;
        loop {
            let (data, index) = encode_data(&words, per_block, shift, &lc, &pc);
            if index.last().is_none_or(|&o| (o >> shift) <= 0xFFFF) || shift == 7 {
                return assemble(&words, per_block, shift, &ll, &pl, &index, &data);
            }
            shift += 1;
        }
    }

    type Codes = BTreeMap<u8, (u32, u8)>;

    fn encode_data(
        words: &[(&Vec<u8>, &Pron)],
        per_block: usize,
        shift: u8,
        lc: &Codes,
        pc: &Codes,
    ) -> (Vec<u8>, Vec<usize>) {
        let shared = |i: usize| if i % per_block == 0 { 0 } else { shared_len(words[i - 1].0, words[i].0) };
        let mut w = BitWriter { out: Vec::new(), n: 0 };
        let mut index = Vec::new();
        for (i, (word, p)) in words.iter().enumerate() {
            if i % per_block == 0 {
                w.align();
                while w.out.len() % (1 << shift) != 0 {
                    w.out.push(0);
                    w.align();
                }
                index.push(w.out.len());
            }
            let sh = shared(i);
            w.put(sh as u32, 4);
            for &c in &word[sh..] {
                let (code, len) = lc[&c];
                w.put(code, len);
            }
            let (code, len) = lc[&END];
            w.put(code, len);
            for s in pron_symbols(p) {
                let (code, len) = pc[&s];
                w.put(code, len);
            }
            if let Pron::Stress(v) = p {
                w.put(u32::from(*v).min(15), 4);
            }
        }
        (w.out, index)
    }

    fn assemble(
        words: &[(&Vec<u8>, &Pron)],
        per_block: usize,
        shift: u8,
        ll: &[(u8, u8)],
        pl: &[(u8, u8)],
        index: &[usize],
        data: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"OTL1");
        out.extend_from_slice(&(words.len() as u16).to_le_bytes());
        out.extend_from_slice(&(index.len() as u16).to_le_bytes());
        out.push(per_block as u8);
        out.push(shift);
        for table in [ll, pl] {
            out.push(table.len() as u8);
            for &(s, l) in table.iter() {
                out.push(s);
                out.push(l);
            }
        }
        for &o in index {
            out.extend_from_slice(&((o >> shift) as u16).to_le_bytes());
        }
        out.extend_from_slice(data);
        out
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn builtin_parses() {
        assert!(Lexicon::new(DATA).is_some());
        assert!(!Lexicon::builtin().is_empty());
    }

    /// TTS-3: the built-in lexicon in three pieces of at most 16 KiB (a
    /// rodata group: a larger static could not be paged on the player)
    /// cut at block starts, and every entry reads as from the whole file.
    #[test]
    fn regress_lexicon_larger_than_a_rodata_group() {
        let cuts = split_points(DATA);
        assert!(DATA.len() > PIECE_MAX, "the case this is for");
        let sizes = [cuts[0], cuts[1] - cuts[0], DATA.len() - cuts[1]];
        assert!(sizes.iter().all(|&n| n > 0 && n <= PIECE_MAX), "{sizes:?}");
        assert_eq!(PART0.len() + PART1.len() + PART2.len(), DATA.len());
        let whole = Lexicon::new(DATA).unwrap();
        let split = Lexicon::builtin();
        assert_eq!(split.len(), whole.len());
        let mut a = std::vec::Vec::new();
        whole.for_each(|w, e, p| a.push((w.to_vec(), e, p.to_vec())));
        let mut b = std::vec::Vec::new();
        split.for_each(|w, e, p| b.push((w.to_vec(), e, p.to_vec())));
        assert_eq!(a, b);
        let mut out = [Phone::new(Ph::AH, Stress::None); MAX_PHONES];
        let mut o2 = out;
        for (w, ..) in a.iter().step_by(37) {
            assert_eq!(split.lookup(w, &mut out), whole.lookup(w, &mut o2));
            assert_eq!(out, o2);
        }
        // A small file is one piece.
        assert_eq!(split_points(&DATA[..100]), [100, 100]);
    }

    #[test]
    fn malformed_headers() {
        assert!(Lexicon::new(b"").is_none());
        assert!(Lexicon::new(b"OTL1\x01\x00\x01\x00\x10\xff").is_none());
        assert!(Lexicon::new(b"XXXX\x00\x00\x00\x00\x10\x00\x00").is_none());
        assert!(Lexicon::new(b"OTL1\x05\x00\x01\x00\x02\x00\x00").is_none());
        // Truncations of the real file never panic.
        for n in 0..64.min(DATA.len()) {
            let _ = Lexicon::new(&DATA[..n]);
        }
        let mut out = [Phone::new(Ph::AH, Stress::None); MAX_PHONES];
        if let Some(l) = Lexicon::new(&DATA[..DATA.len() / 2]) {
            for w in [&b"the"[..], b"zebra", b"a", b"mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm"] {
                let _ = l.lookup(w, &mut out);
            }
        }
        let empty = Lexicon::empty();
        assert_eq!(empty.lookup(b"the", &mut out), None);
    }
}

#[cfg(all(test, feature = "build"))]
mod build_tests {
    extern crate std;
    use super::build::{Pron, encode};
    use super::*;
    use std::collections::BTreeMap;
    use std::vec::Vec;

    #[test]
    fn round_trip() {
        let mut m: BTreeMap<Vec<u8>, Pron> = BTreeMap::new();
        let ph = |s: &str| s.split(' ').map(|t| Phone::parse(t.as_bytes()).unwrap()).collect::<Vec<_>>();
        let words = [
            ("a", Pron::Full(ph("AH0"))),
            ("about", Pron::Full(ph("AH0 B AW1 T"))),
            ("above", Pron::Full(ph("AH0 B AH1 V"))),
            ("one", Pron::Full(ph("W AH1 N"))),
            ("one's", Pron::Full(ph("W AH1 N Z"))),
            ("record", Pron::Stress(1)),
            ("zebra", Pron::Full(ph("Z IY1 B R AH0"))),
        ];
        for (w, p) in words.iter().cloned() {
            m.insert(w.as_bytes().to_vec(), p);
        }
        for per in [1, 2, 3, 4, 16] {
            let bin = encode(&m, per);
            let lex = Lexicon::new(&bin).unwrap();
            assert_eq!(lex.len(), words.len());
            let mut out = [Phone::new(Ph::AH, Stress::None); MAX_PHONES];
            for (w, p) in &words {
                let e = lex.lookup(w.as_bytes(), &mut out).unwrap_or_else(|| panic!("{w} per {per}"));
                match (e, p) {
                    (Entry::Full(n), Pron::Full(v)) => assert_eq!(&out[..n], &v[..]),
                    (Entry::Stress(a), Pron::Stress(b)) => assert_eq!(a, *b),
                    _ => panic!("{w}: {e:?}"),
                }
            }
            for miss in ["", "aa", "abov", "aboutx", "b", "zz", "on", "ones"] {
                assert_eq!(lex.lookup(miss.as_bytes(), &mut out), None, "{miss}");
            }
            let mut n = 0;
            lex.for_each(|_, _, _| n += 1);
            assert_eq!(n, words.len());
        }
    }
}
