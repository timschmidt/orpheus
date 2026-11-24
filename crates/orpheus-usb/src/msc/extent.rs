//! Image files exported as a disk or CD (06-connectivity §3.1): the
//! file's runs on the card are resolved once into a sorted extent table;
//! a logical block maps to a card LBA by binary search, and data moves
//! between the card and USB without the file system.

use super::BlockDevice;

/// Card sector size the table counts in.
pub const SECTOR: u32 = 512;

/// One run: image sectors `[file_sector, file_sector + count)` live at card
/// sectors `[lba, lba + count)`. 12 bytes, so the USB pool's 8 KiB table
/// holds [`POOL_EXTENTS`] runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Extent {
    pub file_sector: u32,
    pub lba: u32,
    pub count: u32,
}

/// Runs that fit an 8 KiB table.
pub const POOL_EXTENTS: usize = 8192 / core::mem::size_of::<Extent>();

/// The image has more runs than the table holds (05 §7.10's "too
/// fragmented" message).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooFragmented;

/// Sorted, gap-free run table over caller-provided storage.
pub struct ExtentTable<'a> {
    runs: &'a mut [Extent],
    len: usize,
    sectors: u32,
}

impl<'a> ExtentTable<'a> {
    pub fn new(storage: &'a mut [Extent]) -> Self {
        Self { runs: storage, len: 0, sectors: 0 }
    }

    /// Append the next run of the file (in file order): `count` sectors at
    /// card `lba`. A run that continues the previous one on the card is
    /// merged.
    pub fn push(&mut self, lba: u32, count: u32) -> Result<(), TooFragmented> {
        if count == 0 {
            return Ok(());
        }
        let file_sector = self.sectors;
        let end = file_sector.checked_add(count).ok_or(TooFragmented)?;
        if let Some(last) = self.len.checked_sub(1).map(|i| &mut self.runs[i])
            && last.lba.checked_add(last.count) == Some(lba)
        {
            last.count += count;
            self.sectors = end;
            return Ok(());
        }
        if self.len == self.runs.len() {
            return Err(TooFragmented);
        }
        self.runs[self.len] = Extent { file_sector, lba, count };
        self.len += 1;
        self.sectors = end;
        Ok(())
    }

    #[must_use]
    pub fn runs(&self) -> &[Extent] {
        &self.runs[..self.len]
    }

    /// Image sectors covered.
    #[must_use]
    pub fn sectors(&self) -> u32 {
        self.sectors
    }

    /// Card LBA of image sector `s` and how many sectors follow
    /// contiguously from there (including `s`).
    #[must_use]
    pub fn map(&self, s: u32) -> Option<(u32, u32)> {
        if s >= self.sectors {
            return None;
        }
        let runs = self.runs();
        let i = runs.partition_point(|e| e.file_sector <= s) - 1;
        let e = runs[i];
        let k = s - e.file_sector;
        Some((e.lba + k, e.count - k))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageError<E> {
    Device(E),
    /// Access past the image (a bug in the caller: capacity comes from
    /// [`ImageDevice::blocks`]).
    OutOfRange,
    /// The table does not cover the file.
    NotCovered,
}

/// A [`BlockDevice`] over an image file: `block_size` (512 for disks, 2048
/// for CD) logical blocks mapped through the extent table onto the card.
/// Bytes past the end of the file read as zeros (an ISO whose size is not
/// a multiple of 2048 is padded); writes there are refused.
pub struct ImageDevice<'t, D> {
    pub card: D,
    table: ExtentTable<'t>,
    file_bytes: u64,
    block_size: u32,
}

impl<'t, D: BlockDevice> ImageDevice<'t, D> {
    pub fn new(
        card: D,
        table: ExtentTable<'t>,
        file_bytes: u64,
        block_size: u32,
    ) -> Result<Self, ImageError<D::Error>> {
        if u64::from(table.sectors()) * u64::from(SECTOR) < file_bytes
            || block_size == 0
            || block_size % SECTOR != 0
        {
            return Err(ImageError::NotCovered);
        }
        Ok(Self { card, table, file_bytes, block_size })
    }

    /// Blocks to report: whole blocks for a disk (the tail is not
    /// addressable), rounded up for a CD (the tail is padded).
    #[must_use]
    pub fn blocks(&self, round_up: bool) -> u64 {
        let bs = u64::from(self.block_size);
        if round_up { self.file_bytes.div_ceil(bs) } else { self.file_bytes / bs }
    }

    #[must_use]
    pub fn table(&self) -> &ExtentTable<'t> {
        &self.table
    }

    /// Card run for image sector `s` (as [`ExtentTable::map`]).
    fn map(&self, s: u64) -> Option<(u32, u32)> {
        self.table.map(u32::try_from(s).ok()?)
    }

    fn check(&self, lba: u64, len: usize, blocks: u64) -> Result<u64, ImageError<D::Error>> {
        let bs = u64::from(self.block_size);
        let off = lba.checked_mul(bs).ok_or(ImageError::OutOfRange)?;
        if len % SECTOR as usize != 0 || off.saturating_add(len as u64) > blocks * bs {
            return Err(ImageError::OutOfRange);
        }
        Ok(off / u64::from(SECTOR))
    }
}

impl<D: BlockDevice> BlockDevice for ImageDevice<'_, D> {
    type Error = ImageError<D::Error>;

    async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        let first = self.check(lba, buf.len(), self.blocks(true))?;
        let total = (buf.len() / SECTOR as usize) as u64;
        let mut done = 0u64;
        while done < total {
            let Some((clba, run)) = self.map(first + done) else {
                // Past the table: padding of the last block.
                buf[done as usize * SECTOR as usize..].fill(0);
                break;
            };
            let n = u64::from(run).min(total - done);
            let (s, e) = (done as usize * SECTOR as usize, (done + n) as usize * SECTOR as usize);
            self.card.read(u64::from(clba), &mut buf[s..e]).await.map_err(ImageError::Device)?;
            done += n;
        }
        // Bytes past the end of the file inside its last card sector.
        let off = first * u64::from(SECTOR);
        if off + buf.len() as u64 > self.file_bytes {
            let from = self.file_bytes.saturating_sub(off) as usize;
            buf[from..].fill(0);
        }
        Ok(())
    }

    async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), Self::Error> {
        let first = self.check(lba, buf.len(), self.blocks(false))?;
        let total = (buf.len() / SECTOR as usize) as u64;
        let mut done = 0u64;
        while done < total {
            let (clba, run) = self.map(first + done).ok_or(ImageError::OutOfRange)?;
            let n = u64::from(run).min(total - done);
            let (s, e) = (done as usize * SECTOR as usize, (done + n) as usize * SECTOR as usize);
            self.card.write(u64::from(clba), &buf[s..e]).await.map_err(ImageError::Device)?;
            done += n;
        }
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        self.card.flush().await.map_err(ImageError::Device)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::convert::Infallible;
    use embassy_futures::block_on;
    use std::vec::Vec;

    struct Card(Vec<u8>);
    impl BlockDevice for Card {
        type Error = Infallible;
        async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Infallible> {
            let o = lba as usize * 512;
            buf.copy_from_slice(&self.0[o..o + buf.len()]);
            Ok(())
        }
        async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), Infallible> {
            let o = lba as usize * 512;
            self.0[o..o + buf.len()].copy_from_slice(buf);
            Ok(())
        }
    }

    fn rnd(x: &mut u64) -> u64 {
        *x ^= *x << 13;
        *x ^= *x >> 7;
        *x ^= *x << 17;
        *x
    }

    /// Scatter a file over a card in `runs` random runs; returns the card,
    /// the file and the runs in file order.
    fn scatter(file_bytes: usize, runs: usize, seed: u64) -> (Card, Vec<u8>, Vec<(u32, u32)>) {
        let mut x = seed;
        let file: Vec<u8> = (0..file_bytes).map(|_| rnd(&mut x) as u8).collect();
        let sectors = file_bytes.div_ceil(512);
        let card_sectors = sectors * 3 + 64;
        let mut card = Card((0..card_sectors * 512).map(|_| 0xEE).collect());
        // Split into runs, place each run at a free spot in reverse order
        // with gaps so neighbours never merge.
        let mut cuts: Vec<usize> = (1..runs).map(|_| 1 + rnd(&mut x) as usize % (sectors - 1)).collect();
        cuts.sort_unstable();
        cuts.dedup();
        let mut bounds = std::vec![0];
        bounds.extend(cuts);
        bounds.push(sectors);
        let mut out = Vec::new();
        let mut at = card_sectors - 1;
        for w in bounds.windows(2) {
            let n = w[1] - w[0];
            at -= n + 1;
            for s in 0..n {
                let src = (w[0] + s) * 512;
                let end = (src + 512).min(file_bytes);
                let dst = (at + s) * 512;
                card.0[dst..dst + (end - src)].copy_from_slice(&file[src..end]);
            }
            out.push((at as u32, n as u32));
        }
        (card, file, out)
    }

    #[test]
    fn map_and_merge() {
        let mut st = [Extent::default(); 3];
        let mut t = ExtentTable::new(&mut st);
        t.push(100, 8).unwrap();
        t.push(108, 8).unwrap(); // contiguous: merged
        t.push(50, 4).unwrap();
        t.push(10, 1).unwrap();
        assert_eq!(t.runs().len(), 3);
        assert_eq!(t.push(0, 1), Err(TooFragmented));
        assert_eq!(t.sectors(), 21);
        assert_eq!(t.map(0), Some((100, 16)));
        assert_eq!(t.map(15), Some((115, 1)));
        assert_eq!(t.map(16), Some((50, 4)));
        assert_eq!(t.map(20), Some((10, 1)));
        assert_eq!(t.map(21), None);
        assert_eq!(POOL_EXTENTS, 682);
    }

    #[test]
    fn iso_reads_across_runs_with_padding() {
        // 300 KiB + 100 bytes: not a multiple of 2048 nor of 512.
        let bytes = 300 * 1024 + 100;
        let (card, file, runs) = scatter(bytes, 40, 7);
        let mut st = [Extent::default(); 64];
        let mut t = ExtentTable::new(&mut st);
        for (lba, n) in &runs {
            t.push(*lba, *n).unwrap();
        }
        let mut img = ImageDevice::new(card, t, bytes as u64, 2048).unwrap();
        let blocks = img.blocks(true);
        assert_eq!(blocks, (bytes as u64).div_ceil(2048));
        let mut x = 99;
        for _ in 0..300 {
            let lba = rnd(&mut x) % blocks;
            let n = 1 + rnd(&mut x) % (blocks - lba).min(24);
            let mut buf = std::vec![0x55u8; n as usize * 2048];
            block_on(img.read(lba, &mut buf)).unwrap();
            let o = lba as usize * 2048;
            for (i, b) in buf.iter().enumerate() {
                let want = file.get(o + i).copied().unwrap_or(0);
                assert_eq!(*b, want, "lba {lba} n {n} byte {i}");
            }
        }
        let mut buf = [0u8; 2048];
        assert_eq!(block_on(img.read(blocks, &mut buf)), Err(ImageError::OutOfRange));
    }

    #[test]
    fn disk_image_write_through_runs() {
        let bytes = 64 * 1024 + 300; // the 300-byte tail is not addressable
        let (card, mut file, runs) = scatter(bytes, 20, 3);
        let mut st = [Extent::default(); 32];
        let mut t = ExtentTable::new(&mut st);
        for (lba, n) in &runs {
            t.push(*lba, *n).unwrap();
        }
        let mut img = ImageDevice::new(card, t, bytes as u64, 512).unwrap();
        assert_eq!(img.blocks(false), 128);
        let data: Vec<u8> = (0..40 * 512).map(|i| (i % 253) as u8).collect();
        block_on(img.write(80, &data)).unwrap();
        file[80 * 512..120 * 512].copy_from_slice(&data);
        let mut back = std::vec![0u8; 128 * 512];
        block_on(img.read(0, &mut back)).unwrap();
        assert_eq!(&back[..], &file[..128 * 512]);
        assert_eq!(block_on(img.write(127, &[0u8; 1024])), Err(ImageError::OutOfRange));
    }

    #[test]
    fn too_fragmented_and_not_covered() {
        let (_, _, runs) = scatter(200 * 512, 100, 5);
        let mut st = [Extent::default(); 50];
        let mut t = ExtentTable::new(&mut st);
        let r: Result<(), TooFragmented> = runs.iter().try_for_each(|(l, n)| t.push(*l, *n));
        assert_eq!(r, Err(TooFragmented));
        let mut st = [Extent::default(); 4];
        let mut t = ExtentTable::new(&mut st);
        t.push(0, 3).unwrap();
        assert!(matches!(ImageDevice::new(Card(Vec::new()), t, 4 * 512, 2048), Err(ImageError::NotCovered)));
    }

    #[test]
    fn many_runs_binary_search() {
        let mut st = [Extent::default(); POOL_EXTENTS];
        let mut t = ExtentTable::new(&mut st);
        for i in 0..POOL_EXTENTS as u32 {
            t.push(1_000_000 - i * 10, 4).unwrap();
        }
        assert_eq!(t.push(5, 1), Err(TooFragmented));
        for i in 0..POOL_EXTENTS as u32 {
            for k in 0..4 {
                assert_eq!(t.map(i * 4 + k), Some((1_000_000 - i * 10 + k, 4 - k)));
            }
        }
    }
}
