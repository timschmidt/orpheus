//! Fuzz-target body of `orpheus-index` (VER-4), shared by the cargo-fuzz
//! target in `fuzz_targets/` (nightly) and the stable replay of the
//! committed corpus (`tests/fuzz_corpus.rs`, every `cargo test`).
//!
//! [`index_open`] — a directory index file as found on the card (damaged,
//! truncated, written by another version, or hostile), opened and queried
//! the way the Files app does. Byte 0 of the input can repair the header
//! CRC and the file length so that the fuzzer reaches the queries with
//! arbitrary header fields. Oracles: no panic, no hang; `open` accepts a
//! file exactly when its header is valid for the directory and spec and the
//! length matches; every query stays inside the listing (row windows,
//! single rows and positions agree with each other; letters and letter
//! jumps name rows of the listing; the RAM letter table answers like the
//! file); a file that passes `verify` lists its directories first.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::convert::Infallible;

use orpheus_index::{
    FileName, HEADER, Index, LETTER_RUN, LetterTable, Locator, NameSource, Probe, RECORD, Row, SortBy, Spec,
    Storage, crc32_update,
};

/// Index files in memory.
#[derive(Default)]
pub struct Mem {
    pub files: HashMap<FileName, Vec<u8>>,
}

impl Storage for Mem {
    type Error = Infallible;
    fn len(&mut self, f: FileName) -> Result<Option<u32>, Infallible> {
        Ok(self.files.get(&f).map(|v| v.len() as u32))
    }
    fn read_at(&mut self, f: FileName, off: u32, buf: &mut [u8]) -> Result<usize, Infallible> {
        let Some(v) = self.files.get(&f) else { return Ok(0) };
        let o = (off as usize).min(v.len());
        let n = buf.len().min(v.len() - o);
        buf[..n].copy_from_slice(&v[o..o + n]);
        Ok(n)
    }
    fn write_at(&mut self, f: FileName, off: u32, data: &[u8]) -> Result<(), Infallible> {
        let v = self.files.entry(f).or_default();
        let o = off as usize;
        assert!(o <= v.len(), "write past the end");
        if v.len() < o + data.len() {
            v.resize(o + data.len(), 0);
        }
        v[o..o + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn truncate(&mut self, f: FileName, len: u32) -> Result<(), Infallible> {
        self.files.entry(f).or_default().truncate(len as usize);
        Ok(())
    }
}

/// Names for [`Index::find`]: every entry is called `name`.
struct Names<'a>(&'a str);

impl NameSource for Names<'_> {
    type Error = Infallible;
    fn name_of(&mut self, _loc: Locator, name: &mut [u8]) -> Result<Option<usize>, Infallible> {
        let b = self.0.as_bytes();
        name[..b.len()].copy_from_slice(b);
        Ok(Some(b.len()))
    }
}

fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// The spec of a header's spec byte (`None` if the byte is not one).
fn spec_of(c: u8) -> Option<Spec> {
    let sort = match c & 3 {
        0 => SortBy::Name,
        1 => SortBy::Date,
        2 => SortBy::Size,
        _ => return None,
    };
    (c & !0x1F == 0).then_some(Spec {
        sort,
        show_hidden: c & 4 != 0,
        audio_only: c & 8 != 0,
        images_only: c & 0x10 != 0,
    })
}

/// Largest file the target builds when it repairs the length.
const MAX_FILE: u64 = 1 << 20;

fn empty_row() -> Row {
    Row {
        loc: Locator { dir: 0, index: 0 },
        size: 0,
        mtime: 0,
        letter: None,
        kind: orpheus_index::EntryKind::Other,
        hidden: false,
    }
}

/// See the crate docs. Byte 0: bit 0 repairs the header CRC, bit 1 the
/// file length; the rest is the file.
pub fn index_open(data: &[u8]) {
    let Some((&sel, file)) = data.split_first() else { return };
    let mut file = file.to_vec();
    if sel & 1 != 0 && file.len() >= 68 {
        let c = crc32_update(0, &file[..64]);
        file[64..68].copy_from_slice(&c.to_le_bytes());
    }
    let (dir, spec, rows, runs) = if file.len() >= 68 {
        (rd32(&file, 8), spec_of(file[12]), rd32(&file, 16), rd32(&file, 24))
    } else {
        (0, None, 0, 0)
    };
    let implied = HEADER as u64 + u64::from(rows) * RECORD as u64 + u64::from(runs) * LETTER_RUN as u64;
    if sel & 2 != 0 && implied <= MAX_FILE {
        file.resize(implied as usize, 0);
    }
    let spec = spec.unwrap_or_default();
    let name = FileName::index(dir, spec);
    let mut store = Mem::default();
    let len = file.len() as u64;
    store.files.insert(name, file);
    let ix = Index::open(&mut store, dir, spec).expect("memory storage does not fail");
    // A different directory or spec never opens this file.
    assert!(Index::open(&mut store, dir ^ 1, spec).unwrap().is_none());
    let Some(ix) = ix else { return };
    assert_eq!(ix.file_len(), len, "opened with a length that does not match");
    assert_eq!((ix.spec(), ix.count(), ix.letter_runs()), (spec, rows, runs));
    assert!(ix.dirs() <= ix.count());
    assert_eq!(ix.files(), ix.count() - ix.dirs());
    let n = ix.count();

    // Row windows against single rows.
    let mut win = vec![empty_row(); 37];
    for first in [0, n / 2, n.saturating_sub(5), n, n.saturating_add(1), u32::MAX] {
        let got = ix.rows(&mut store, first, &mut win).expect("rows of a file of the right length");
        assert_eq!(got, (n.saturating_sub(first) as usize).min(win.len()));
        for (k, r) in win[..got].iter().enumerate() {
            let i = first + k as u32;
            assert_eq!(ix.row(&mut store, i).unwrap(), Some(*r));
            if n <= 4096 {
                let p = ix.position_of(&mut store, r.loc).unwrap().expect("the row's locator is found");
                assert!(p <= i);
                assert_eq!(ix.row(&mut store, p).unwrap().map(|x| x.loc), Some(r.loc));
            }
        }
    }
    assert_eq!(ix.row(&mut store, n).unwrap(), None);

    // Letters and letter jumps never leave the listing. On a file whose
    // runs are in order (it loads into the RAM table, or passes `verify`)
    // the target row has the letter; on a verified file the RAM table
    // answers like the file (a damaged one may hold letters that are not
    // characters, which the table reads as "no letter").
    let mut buf = [0u8; 512];
    // "Verified": the section CRCs match and the header was not repaired by
    // the target (a repaired header may carry any counts).
    let verified = sel & 1 == 0 && ix.verify(&mut store, &mut buf).unwrap_or(false);
    let mut table = LetterTable::<64>::EMPTY;
    let loaded = table.load(&ix, &mut store).unwrap_or(false);
    for i in [0, 1, n / 3, n / 2, n.saturating_sub(1), n] {
        let l = ix.letter(&mut store, i);
        if i >= n {
            assert_eq!(l, Ok(None));
        }
        if loaded && verified {
            assert_eq!(Ok(table.letter(i)), l, "letter table and file disagree at {i}");
        }
        for fwd in [true, false] {
            let t = if loaded { table.seek_letter(i, fwd) } else { None };
            if let Some((j, _)) = t {
                assert!(j < n, "letter table jump to {j} of {n}");
            }
            let r = ix.seek_letter(&mut store, i, fwd);
            if let Ok(Some((j, c))) = r {
                assert!(j < n, "letter jump to {j} of {n}");
                if loaded || verified {
                    assert_eq!(ix.letter(&mut store, j), Ok(Some(c)), "jump target letter");
                }
            }
            if loaded && verified {
                assert_eq!(r, Ok(t), "letter table and file jump differently from {i}");
            }
        }
    }
    let mut lr = [(0u32, None); 20];
    if let Ok(k) = ix.read_letter_runs(&mut store, runs / 2, &mut lr) {
        assert_eq!(k, (runs - runs / 2).min(20) as usize);
    }

    // Lookup of an entry.
    let probe = Probe {
        name: "name",
        loc: Locator { dir, index: 3 },
        is_dir: sel & 4 != 0,
        hidden: false,
        size: 9,
        mtime: 7,
    };
    let mut nb = vec![0u8; orpheus_index::MAX_NAME];
    match ix.find(&mut store, &mut Names("name"), &probe, &mut nb) {
        Ok(Ok(i)) => assert!(i < n),
        Ok(Err(i)) => assert!(i <= n),
        Err(_) => {}
    }

    // A verified file lists its directories first.
    if verified && n <= 4096 {
        for i in 0..n {
            let r = ix.row(&mut store, i).unwrap().unwrap();
            assert_eq!(r.is_dir(), i < ix.dirs(), "row {i} of a verified index");
        }
    }
}
