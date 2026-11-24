//! Host-side block devices and a tiny executor (feature `std`).
//!
//! * [`RamDisk`]: an image in memory.
//! * [`FileDisk`]: an image file; optionally skips writing all-zero sectors
//!   onto ranges that already read as zero, which keeps sparse test images
//!   sparse.
//! * [`FaultyDisk`]: wraps a device and simulates a power cut: after a
//!   chosen number of sector writes every write fails (a multi-sector write
//!   that crosses the point is torn), and reads keep working.
//! * [`block_on`]: polls a future to completion with a no-op waker (the host
//!   devices never return `Pending`).

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::vec::Vec;

use crate::block::{BlockDevice, SECTOR_SIZE, Sector};

/// Errors of the host devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostError {
    /// Host I/O failed.
    Io(std::io::ErrorKind),
    /// Access beyond the end of the image.
    OutOfRange,
    /// The simulated power is off.
    PowerCut,
}

/// Run a future to completion on the current thread.
pub fn block_on<F: core::future::Future>(f: F) -> F::Output {
    let mut f = core::pin::pin!(f);
    let waker = core::task::Waker::noop();
    let mut cx = core::task::Context::from_waker(waker);
    loop {
        if let core::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

/// An in-memory image.
#[derive(Clone)]
pub struct RamDisk {
    /// Image bytes (length a multiple of 512).
    pub data: Vec<u8>,
    /// Sector reads/writes issued.
    pub reads: u64,
    /// Sector writes issued.
    pub writes: u64,
}

impl RamDisk {
    /// Zero-filled image of `sectors` sectors.
    #[must_use]
    pub fn new(sectors: u32) -> Self {
        RamDisk { data: std::vec![0; sectors as usize * SECTOR_SIZE], reads: 0, writes: 0 }
    }
    /// Wrap existing bytes (padded to whole sectors).
    #[must_use]
    pub fn from_bytes(mut data: Vec<u8>) -> Self {
        let pad = data.len().next_multiple_of(SECTOR_SIZE);
        data.resize(pad, 0);
        RamDisk { data, reads: 0, writes: 0 }
    }
    /// Load an image file.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        Ok(Self::from_bytes(fs::read(path)?))
    }
    /// Save to an image file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        fs::write(path, &self.data)
    }
    fn range(&self, lba: u32, n: usize) -> Result<core::ops::Range<usize>, HostError> {
        let s = lba as usize * SECTOR_SIZE;
        let e = s + n * SECTOR_SIZE;
        if e > self.data.len() {
            return Err(HostError::OutOfRange);
        }
        Ok(s..e)
    }
}

impl BlockDevice for RamDisk {
    type Error = HostError;
    async fn read(&mut self, lba: u32, buf: &mut [Sector]) -> Result<(), HostError> {
        let r = self.range(lba, buf.len())?;
        buf.as_flattened_mut().copy_from_slice(&self.data[r]);
        self.reads += buf.len() as u64;
        Ok(())
    }
    async fn write(&mut self, lba: u32, buf: &[Sector]) -> Result<(), HostError> {
        let r = self.range(lba, buf.len())?;
        self.data[r].copy_from_slice(buf.as_flattened());
        self.writes += buf.len() as u64;
        Ok(())
    }
    fn sector_count(&self) -> u32 {
        (self.data.len() / SECTOR_SIZE) as u32
    }
}

/// An image file used as a block device.
pub struct FileDisk {
    file: fs::File,
    sectors: u32,
    /// Skip writes of all-zero data over ranges that already read as zero.
    pub elide_zero_writes: bool,
    scratch: Vec<u8>,
}

impl FileDisk {
    /// Open an image file read/write.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = fs::OpenOptions::new().read(true).write(true).open(path)?;
        let len = file.metadata()?.len();
        Ok(FileDisk {
            file,
            sectors: u32::try_from(len / SECTOR_SIZE as u64).unwrap_or(u32::MAX),
            elide_zero_writes: false,
            scratch: Vec::new(),
        })
    }
    fn seek(&mut self, lba: u32, n: usize) -> Result<(), HostError> {
        if u64::from(lba) + n as u64 > u64::from(self.sectors) {
            return Err(HostError::OutOfRange);
        }
        self.file
            .seek(SeekFrom::Start(u64::from(lba) * SECTOR_SIZE as u64))
            .map_err(|e| HostError::Io(e.kind()))?;
        Ok(())
    }
}

impl BlockDevice for FileDisk {
    type Error = HostError;
    async fn read(&mut self, lba: u32, buf: &mut [Sector]) -> Result<(), HostError> {
        self.seek(lba, buf.len())?;
        self.file.read_exact(buf.as_flattened_mut()).map_err(|e| HostError::Io(e.kind()))
    }
    async fn write(&mut self, lba: u32, buf: &[Sector]) -> Result<(), HostError> {
        let bytes = buf.as_flattened();
        if self.elide_zero_writes && bytes.iter().all(|&b| b == 0) {
            self.seek(lba, buf.len())?;
            self.scratch.resize(bytes.len(), 0);
            let mut tmp = core::mem::take(&mut self.scratch);
            let r = self.file.read_exact(&mut tmp);
            let zero = r.is_ok() && tmp.iter().all(|&b| b == 0);
            self.scratch = tmp;
            if zero {
                return Ok(());
            }
        }
        self.seek(lba, buf.len())?;
        self.file.write_all(bytes).map_err(|e| HostError::Io(e.kind()))
    }
    async fn sync(&mut self) -> Result<(), HostError> {
        self.file.flush().map_err(|e| HostError::Io(e.kind()))
    }
    fn sector_count(&self) -> u32 {
        self.sectors
    }
}

/// Power-cut simulator.
pub struct FaultyDisk<D> {
    /// The wrapped device (holds the state "on the medium").
    pub inner: D,
    /// Sector writes still accepted; `None` = unlimited.
    pub writes_left: Option<u64>,
    /// Sector writes performed so far.
    pub writes_done: u64,
}

impl<D> FaultyDisk<D> {
    /// Cut the power after `writes` sector writes (`None`: never).
    pub fn new(inner: D, writes: Option<u64>) -> Self {
        FaultyDisk { inner, writes_left: writes, writes_done: 0 }
    }
    /// True once writes are being refused.
    pub fn is_cut(&self) -> bool {
        self.writes_left == Some(0)
    }
}

impl<D: BlockDevice<Error = HostError>> BlockDevice for FaultyDisk<D> {
    type Error = HostError;
    async fn read(&mut self, lba: u32, buf: &mut [Sector]) -> Result<(), HostError> {
        self.inner.read(lba, buf).await
    }
    async fn write(&mut self, lba: u32, buf: &[Sector]) -> Result<(), HostError> {
        let Some(left) = self.writes_left else {
            self.writes_done += buf.len() as u64;
            return self.inner.write(lba, buf).await;
        };
        let n = (buf.len() as u64).min(left) as usize;
        if n > 0 {
            self.inner.write(lba, &buf[..n]).await?;
        }
        self.writes_done += n as u64;
        self.writes_left = Some(left - n as u64);
        if n < buf.len() {
            return Err(HostError::PowerCut);
        }
        Ok(())
    }
    async fn sync(&mut self) -> Result<(), HostError> {
        if self.is_cut() {
            return Err(HostError::PowerCut);
        }
        self.inner.sync().await
    }
    fn sector_count(&self) -> u32 {
        self.inner.sector_count()
    }
}

/// Walk a whole volume (iteratively, bounded) reading every file; used by
/// the fuzz target and robustness tests. Returns (dirs, files, bytes).
/// Errors are returned, never panics.
pub fn exercise<D: BlockDevice>(
    vol: &mut crate::Volume<'_, D>,
    budget: u32,
) -> Result<(u32, u32, u64), crate::Error<D::Error>> {
    block_on(async {
        let mut stack: Vec<crate::DirCursor> = std::vec![vol.root()];
        let mut e = crate::DirEntry::new();
        let mut buf = std::vec![0u8; 4096];
        let (mut dirs, mut files, mut bytes) = (0u32, 0u32, 0u64);
        let mut steps = 0u32;
        while let Some(mut cur) = stack.pop() {
            dirs += 1;
            while vol.read_dir(&mut cur, &mut e).await? {
                steps += 1;
                if steps > budget {
                    return Ok((dirs, files, bytes));
                }
                let _ = e.name();
                if e.is_dir() {
                    if stack.len() < 16 {
                        stack.push(vol.dir_cursor(&e)?);
                    }
                } else {
                    files += 1;
                    let mut f = vol.open_entry(&e)?;
                    let mut it = vol.extents(&f);
                    let mut n = 0;
                    while vol.next_extent(&mut it).await?.is_some() {
                        n += 1;
                        if n > 64 {
                            break;
                        }
                    }
                    let mut total = 0u64;
                    loop {
                        let k = vol.read(&mut f, &mut buf).await?;
                        if k == 0 || total > 1 << 20 {
                            break;
                        }
                        total += k as u64;
                    }
                    bytes += total;
                }
            }
        }
        let _ = vol.free_clusters().await?;
        Ok((dirs, files, bytes))
    })
}

/// The read-only base of an [`OverlayDisk`]: borrowed, or shared and owned
/// (a simulator that builds its card at run time, e.g. the browser build).
#[derive(Clone, Debug)]
pub enum Base<'a> {
    /// A borrowed image.
    Borrowed(&'a [u8]),
    /// An image owned by everyone holding a clone.
    Shared(std::sync::Arc<[u8]>),
}

impl core::ops::Deref for Base<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Base::Borrowed(b) => b,
            Base::Shared(b) => b,
        }
    }
}

/// A read-only base image plus an in-memory overlay of written sectors.
/// Cheap to create per fuzz run even for large bases.
pub struct OverlayDisk<'a> {
    base: Base<'a>,
    sectors: u32,
    written: std::collections::HashMap<u32, Sector>,
}

impl<'a> OverlayDisk<'a> {
    /// Overlay over `base` (its length is rounded down to whole sectors).
    #[must_use]
    pub fn new(base: &'a [u8]) -> Self {
        Self::over(Base::Borrowed(base))
    }

    /// Overlay over a borrowed or shared base.
    #[must_use]
    pub fn over(base: Base<'a>) -> Self {
        OverlayDisk {
            sectors: u32::try_from(base.len() / SECTOR_SIZE).unwrap_or(u32::MAX),
            base,
            written: std::collections::HashMap::new(),
        }
    }
    fn sector(&self, lba: u32) -> Sector {
        if let Some(s) = self.written.get(&lba) {
            return *s;
        }
        let mut s = [0u8; SECTOR_SIZE];
        let o = lba as usize * SECTOR_SIZE;
        if let Some(src) = self.base.get(o..o + SECTOR_SIZE) {
            s.copy_from_slice(src);
        }
        s
    }
    /// Change one byte of the image.
    pub fn patch(&mut self, offset: usize, value: u8) {
        let Ok(lba) = u32::try_from(offset / SECTOR_SIZE) else {
            return;
        };
        if lba >= self.sectors {
            return;
        }
        let mut s = self.sector(lba);
        s[offset % SECTOR_SIZE] = value;
        self.written.insert(lba, s);
    }
    /// Number of sectors written or patched so far.
    #[must_use]
    pub fn dirty_sectors(&self) -> usize {
        self.written.len()
    }
}

impl BlockDevice for OverlayDisk<'_> {
    type Error = HostError;
    async fn read(&mut self, lba: u32, buf: &mut [Sector]) -> Result<(), HostError> {
        if u64::from(lba) + buf.len() as u64 > u64::from(self.sectors) {
            return Err(HostError::OutOfRange);
        }
        for (k, s) in buf.iter_mut().enumerate() {
            *s = self.sector(lba + k as u32);
        }
        Ok(())
    }
    async fn write(&mut self, lba: u32, buf: &[Sector]) -> Result<(), HostError> {
        if u64::from(lba) + buf.len() as u64 > u64::from(self.sectors) {
            return Err(HostError::OutOfRange);
        }
        for (k, s) in buf.iter().enumerate() {
            self.written.insert(lba + k as u32, *s);
        }
        Ok(())
    }
    fn sector_count(&self) -> u32 {
        self.sectors
    }
}

/// One fuzz iteration: pick a base image with `data[0]`, apply byte patches
/// (`data[1..]` in 4-byte groups: 24-bit offset into the first 16 MiB, value),
/// mount, walk, optionally mutate (bit 7 of `data[0]`), flush, remount and
/// walk again. Errors are expected; panics and hangs are the bugs.
pub fn fuzz_one(bases: &[&[u8]], data: &[u8]) {
    let Some((&sel, rest)) = data.split_first() else {
        return;
    };
    if bases.is_empty() {
        return;
    }
    let base = bases[usize::from(sel) % bases.len()];
    let mut disk = OverlayDisk::new(base);
    let window = base.len().clamp(1, 1 << 24);
    for p in rest.as_chunks::<4>().0 {
        let off = (usize::from(p[0]) | (usize::from(p[1]) << 8) | (usize::from(p[2]) << 16)) % window;
        disk.patch(off, p[3]);
    }
    let writes = sel & 0x80 != 0;
    let mut cache = [crate::CacheSlot::EMPTY, crate::CacheSlot::EMPTY, crate::CacheSlot::EMPTY];
    let opts = crate::MountOptions { read_only: !writes, ..Default::default() };
    let Ok(mut vol) = block_on(crate::Volume::mount(&mut disk, &mut cache, opts)) else {
        return;
    };
    let _ = exercise(&mut vol, 3000);
    if !writes {
        return;
    }
    let _ = block_on(async {
        let _ = vol.create_dir("/fz").await;
        if let Ok(mut f) = vol.open("/fz/Ünïcødé fuzz name.bin", crate::OpenMode::Create).await {
            let _ = vol.write(&mut f, &[0xA5; 3000]).await;
            let _ = vol.set_len(&mut f, 700).await;
            let _ = vol.close(f).await;
        }
        let _ = vol.rename("/fz/Ünïcødé fuzz name.bin", "/moved.bin").await;
        let mut cur = vol.root();
        let mut e = crate::DirEntry::new();
        if vol.read_dir(&mut cur, &mut e).await.unwrap_or(false) {
            let mut p = std::string::String::from("/");
            p.push_str(e.name());
            let _ = vol.remove(&p).await;
        }
        vol.flush().await
    });
    let mut cache = [crate::CacheSlot::EMPTY, crate::CacheSlot::EMPTY, crate::CacheSlot::EMPTY];
    if let Ok(mut vol) = block_on(crate::Volume::mount(&mut disk, &mut cache, crate::MountOptions::default()))
    {
        let _ = exercise(&mut vol, 3000);
    }
}
