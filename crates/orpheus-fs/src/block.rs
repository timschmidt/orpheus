//! The block-device seam between the SD driver and the file system.

/// Bytes per sector. Only 512-byte logical sectors are supported.
pub const SECTOR_SIZE: usize = 512;

/// One sector.
pub type Sector = [u8; SECTOR_SIZE];

/// Synchronous pollers running now (a future polled in a loop with
/// whatever context the caller has, e.g. `orpheus_index::card::spin`).
/// While one runs, a block device must not wait for time to pass (a timer
/// polled in such a loop may never fire, and nothing else runs meanwhile):
/// rests and pacing are skipped. Found 2026-11-25: the card's pacing sleep
/// inside a synchronous library session hung the player until the watchdog.
pub static SYNC_POLLERS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Marks a synchronous poller for its lifetime ([`SYNC_POLLERS`]).
pub struct SyncPoll(());

impl SyncPoll {
    /// Start marking; the mark ends when the value is dropped.
    #[must_use]
    pub fn enter() -> Self {
        SYNC_POLLERS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        Self(())
    }

    /// Whether any synchronous poller is running.
    #[must_use]
    pub fn active() -> bool {
        SYNC_POLLERS.load(core::sync::atomic::Ordering::Relaxed) != 0
    }
}

impl Drop for SyncPoll {
    fn drop(&mut self) {
        SYNC_POLLERS.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
    }
}

/// An async block device with 512-byte sectors.
///
/// `lba` is relative to the start of the device (the file system adds the
/// partition offset itself). Multi-sector calls are expected to map to one
/// multi-block transfer (CMD18/CMD25). Buffers carry no alignment guarantee;
/// a DMA driver that needs one checks `as_ptr()` and bounces if necessary.
///
/// File data is transferred directly between the device and the caller's
/// buffer; turn a `&mut [u8]` whose length is a multiple of 512 into
/// `&mut [Sector]` with `as_chunks_mut::<512>()`.
#[allow(async_fn_in_trait)]
pub trait BlockDevice {
    /// Driver error. Returned to the caller inside [`crate::Error::Io`].
    type Error;

    /// Read `buf.len()` sectors starting at `lba`.
    async fn read(&mut self, lba: u32, buf: &mut [Sector]) -> Result<(), Self::Error>;

    /// Write `buf.len()` sectors starting at `lba`.
    async fn write(&mut self, lba: u32, buf: &[Sector]) -> Result<(), Self::Error>;

    /// Make every completed write durable (card busy finished, any write
    /// cache drained). The default does nothing.
    async fn sync(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    /// Number of sectors on the device.
    fn sector_count(&self) -> u32;
}

impl<T: BlockDevice> BlockDevice for &mut T {
    type Error = T::Error;

    async fn read(&mut self, lba: u32, buf: &mut [Sector]) -> Result<(), Self::Error> {
        (**self).read(lba, buf).await
    }

    async fn write(&mut self, lba: u32, buf: &[Sector]) -> Result<(), Self::Error> {
        (**self).write(lba, buf).await
    }

    async fn sync(&mut self) -> Result<(), Self::Error> {
        (**self).sync().await
    }

    fn sector_count(&self) -> u32 {
        (**self).sector_count()
    }
}
