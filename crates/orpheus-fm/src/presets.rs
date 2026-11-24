//! Radio presets (05 §7.3): up to 30 frequencies with an optional name
//! (the station's RDS service name, raw RDS bytes), kept in the byte form
//! they are stored in (settings store key [`STORE_KEY`]), so saving needs
//! no second buffer:
//!
//! ```text
//! version u8 (1) | count u8 | count × (khz/10 u16 LE | name_len u8 | name [8])
//! ```

/// Most presets (05 §7.3: auto-scan fills up to 30).
pub const MAX: usize = 30;
/// Name bytes kept (an RDS service name).
pub const NAME: usize = 8;
const ENTRY: usize = 3 + NAME;
const HEAD: usize = 2;
/// Bytes of a full list.
pub const BYTES: usize = HEAD + MAX * ENTRY;
const VERSION: u8 = 1;
/// Settings-store key of the list (group 7 = Radio, past the settings).
pub const STORE_KEY: u16 = 0x0710;

/// One preset, by value.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Preset {
    pub khz: u32,
    name: [u8; NAME],
    name_len: u8,
}

impl Preset {
    #[must_use]
    pub fn new(khz: u32, name: &[u8]) -> Preset {
        let mut p = Preset { khz, ..Preset::default() };
        for (o, &c) in p.name.iter_mut().zip(name) {
            *o = c;
        }
        p.name_len = name.len().min(NAME) as u8;
        p
    }

    /// The name in RDS characters (`rds::to_utf8` makes text of it).
    #[must_use]
    pub fn name(&self) -> &[u8] {
        self.name.get(..usize::from(self.name_len)).unwrap_or(&[])
    }
}

/// The list in its stored form.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Presets {
    data: [u8; BYTES],
}

impl Default for Presets {
    fn default() -> Self {
        Presets::new()
    }
}

impl Presets {
    #[must_use]
    pub const fn new() -> Presets {
        let mut data = [0u8; BYTES];
        data[0] = VERSION;
        Presets { data }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.data[1]).min(MAX)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn entry(&self, i: usize) -> Option<&[u8]> {
        if i >= self.len() {
            return None;
        }
        self.data.get(HEAD + i * ENTRY..HEAD + (i + 1) * ENTRY)
    }

    fn put(&mut self, i: usize, p: &Preset) {
        let Some(e) = self.data.get_mut(HEAD + i * ENTRY..HEAD + (i + 1) * ENTRY) else { return };
        let k = u16::try_from(p.khz / 10).unwrap_or(0).to_le_bytes();
        let mut bytes = [0u8; ENTRY];
        bytes[0] = k[0];
        bytes[1] = k[1];
        bytes[2] = p.name_len;
        for (o, &c) in bytes.iter_mut().skip(3).zip(&p.name) {
            *o = c;
        }
        e.copy_from_slice(&bytes);
    }

    #[must_use]
    pub fn get(&self, i: usize) -> Option<Preset> {
        let e = self.entry(i)?;
        let khz = u32::from(u16::from_le_bytes([*e.first()?, *e.get(1)?])) * 10;
        let n = usize::from(*e.get(2)?).min(NAME);
        Some(Preset::new(khz, e.get(3..3 + n)?))
    }

    /// The preset on `khz`.
    #[must_use]
    pub fn find(&self, khz: u32) -> Option<usize> {
        (0..self.len()).find(|&i| self.get(i).is_some_and(|p| p.khz == khz))
    }

    /// Save `khz` (with `name` if it has one): an existing preset on it
    /// gets the name, else a new one is appended. `None` when full.
    pub fn save(&mut self, khz: u32, name: &[u8]) -> Option<usize> {
        if let Some(i) = self.find(khz) {
            if !name.is_empty() {
                self.set_name(i, name);
            }
            return Some(i);
        }
        let n = self.len();
        if n >= MAX {
            return None;
        }
        self.data[1] = (n + 1) as u8;
        self.put(n, &Preset::new(khz, name));
        Some(n)
    }

    /// Name preset `i`; true if it changed.
    pub fn set_name(&mut self, i: usize, name: &[u8]) -> bool {
        let Some(p) = self.get(i) else { return false };
        if p.name() == name.get(..name.len().min(NAME)).unwrap_or(&[]) {
            return false;
        }
        self.put(i, &Preset::new(p.khz, name));
        true
    }

    pub fn remove(&mut self, i: usize) -> bool {
        let n = self.len();
        if i >= n {
            return false;
        }
        for k in i..n - 1 {
            if let Some(p) = self.get(k + 1) {
                self.put(k, &p);
            }
        }
        self.data[1] = (n - 1) as u8;
        true
    }

    /// Exchange presets `i` and `j` (reordering).
    pub fn swap(&mut self, i: usize, j: usize) -> bool {
        let (Some(a), Some(b)) = (self.get(i), self.get(j)) else { return false };
        self.put(i, &b);
        self.put(j, &a);
        true
    }

    /// Replace the list (an auto-scan's stations, in their order).
    pub fn replace(&mut self, khz: impl Iterator<Item = u32>) {
        self.data[1] = 0;
        for k in khz.take(MAX) {
            let _ = self.save(k, &[]);
        }
    }

    /// The preset to tune to from `khz`: Down = the next (`next`), Up = the
    /// previous, wrapping; off a preset Down starts at the first, Up at
    /// the last.
    #[must_use]
    pub fn neighbour(&self, khz: u32, next: bool) -> Option<usize> {
        let n = self.len();
        if n == 0 {
            return None;
        }
        Some(match (self.find(khz), next) {
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
            (None, true) => 0,
            (None, false) => n - 1,
        })
    }

    /// The bytes to store (used part only).
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.data.get(..HEAD + self.len() * ENTRY).unwrap_or(&self.data)
    }

    /// The buffer a stored list is read into; then [`Presets::loaded`].
    pub fn raw_mut(&mut self) -> &mut [u8; BYTES] {
        &mut self.data
    }

    /// Check a list read into [`Presets::raw_mut`] (`n` bytes): a wrong
    /// version or length empties it, entries from the first implausible
    /// one on are dropped.
    pub fn loaded(&mut self, n: usize) {
        if n < HEAD || self.data[0] != VERSION || usize::from(self.data[1]) > MAX {
            *self = Presets::new();
            return;
        }
        let count = usize::from(self.data[1]).min((n - HEAD) / ENTRY);
        let good = (0..count)
            .take_while(|&i| {
                self.entry(i).is_some_and(|e| match *e {
                    [k0, k1, len, ..] => {
                        let k = u32::from(u16::from_le_bytes([k0, k1])) * 10;
                        (64_000..=108_000).contains(&k) && usize::from(len) <= NAME
                    }
                    _ => false,
                })
            })
            .count();
        self.data[1] = good as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_find_name_remove_swap() {
        let mut p = Presets::new();
        assert!(p.is_empty());
        assert_eq!(p.save(101_900, b"PARADISE"), Some(0));
        assert_eq!(p.save(94_700, b""), Some(1));
        assert_eq!(p.save(101_900, b""), Some(0), "already there, name kept");
        assert_eq!(p.get(0).unwrap().name(), b"PARADISE");
        assert_eq!(p.save(94_700, b"ROCK FM LONGER"), Some(1));
        assert_eq!(p.get(1).unwrap().name(), b"ROCK FM ", "cut to 8");
        assert!(!p.set_name(1, b"ROCK FM "));
        assert_eq!(p.find(94_700), Some(1));
        assert!(p.swap(0, 1));
        assert_eq!(p.get(0).unwrap().khz, 94_700);
        assert!(p.remove(0));
        assert_eq!(p.len(), 1);
        assert_eq!(p.get(0).unwrap().khz, 101_900);
        assert!(!p.remove(5));
        assert!(p.get(1).is_none());
    }

    #[test]
    fn full_list_and_replace() {
        let mut p = Presets::new();
        for i in 0..MAX as u32 {
            assert!(p.save(87_500 + i * 100, b"").is_some());
        }
        assert_eq!(p.save(107_000, b""), None);
        p.replace([88_300, 91_500].into_iter());
        assert_eq!(p.len(), 2);
        assert_eq!(p.get(1).unwrap().khz, 91_500);
        // 50 kHz steps survive the 10 kHz storage.
        p.save(87_550, b"");
        assert_eq!(p.get(2).unwrap().khz, 87_550);
    }

    #[test]
    fn neighbours_wrap() {
        let mut p = Presets::new();
        assert_eq!(p.neighbour(90_000, true), None);
        p.replace([88_300, 91_500, 99_900].into_iter());
        assert_eq!(p.neighbour(91_500, true), Some(2));
        assert_eq!(p.neighbour(99_900, true), Some(0));
        assert_eq!(p.neighbour(88_300, false), Some(2));
        assert_eq!(p.neighbour(100_000, true), Some(0));
        assert_eq!(p.neighbour(100_000, false), Some(2));
    }

    #[test]
    fn stored_form_round_trips_and_bad_data_is_dropped() {
        let mut p = Presets::new();
        p.save(101_900, b"PARADISE");
        p.save(68_300, b"");
        let bytes = p.bytes().to_vec();
        assert_eq!(bytes.len(), 2 + 2 * 11);
        let mut q = Presets::new();
        q.raw_mut()[..bytes.len()].copy_from_slice(&bytes);
        q.loaded(bytes.len());
        assert_eq!(q, p);
        // Truncated: only whole entries.
        let mut t = Presets::new();
        t.raw_mut()[..bytes.len() - 3].copy_from_slice(&bytes[..bytes.len() - 3]);
        t.loaded(bytes.len() - 3);
        assert_eq!(t.len(), 1);
        // Wrong version.
        let mut v = Presets::new();
        v.raw_mut()[..bytes.len()].copy_from_slice(&bytes);
        v.raw_mut()[0] = 9;
        v.loaded(bytes.len());
        assert!(v.is_empty());
        // An implausible frequency ends the list.
        let mut f = Presets::new();
        f.raw_mut()[..bytes.len()].copy_from_slice(&bytes);
        f.raw_mut()[2 + 11] = 0xFF;
        f.raw_mut()[2 + 12] = 0xFF;
        f.loaded(bytes.len());
        assert_eq!(f.len(), 1);
    }
}
