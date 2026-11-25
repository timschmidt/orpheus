//! The simulator's USB Drive back end (USB-6, 05 §7.10): the firmware's
//! checks (`orpheus_usb::msc::image`), the image file mapped on the
//! simulated card into the same extent table the firmware fills, and a
//! fake computer that reads the whole medium through `ImageDevice` (and,
//! with `set:drivewrite=N`, writes N blocks at its start).
//!
//! Script keys: `set:drivehost=1` (a computer uses the drive),
//! `set:drivewrite=N`.

use orpheus_apps::browse::FileRef;
use orpheus_apps::services::{DriveInfo, DrivePhase, DriveRefusal, DriveRequest};
use orpheus_fs::OpenMode;
use orpheus_fs::host::{OverlayDisk, block_on};
use orpheus_usb::msc::BlockDevice;
use orpheus_usb::msc::extent::{Extent, ExtentTable, ImageDevice, POOL_EXTENTS};
use orpheus_usb::msc::image::{self, Request};

use crate::media::Media;

/// Time the card task takes to map the file and hand the card over.
const PREPARE_MS: u64 = 300;
/// The fake computer's read rate (bytes per ms: 512 KiB/s, slow enough
/// for a golden image to catch it reading).
const RATE: u64 = 512;

/// The card as the image's block device (raw sectors of the overlay).
struct Card<'a>(&'a mut OverlayDisk<'static>);

impl BlockDevice for Card<'_> {
    type Error = ();
    async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
        let (s, _) = buf.as_chunks_mut::<512>();
        orpheus_fs::BlockDevice::read(self.0, u32::try_from(lba).map_err(|_| ())?, s).await.map_err(|_| ())
    }
    async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), ()> {
        let (s, _) = buf.as_chunks::<512>();
        orpheus_fs::BlockDevice::write(self.0, u32::try_from(lba).map_err(|_| ())?, s).await.map_err(|_| ())
    }
}

pub struct SimDrive {
    pub info: DriveInfo,
    pub name: String,
    /// The presented file's path.
    pub path: String,
    table: Vec<Extent>,
    runs: usize,
    since_ms: u64,
    /// `set:drivehost=1`: a computer has the drive and reads it.
    pub host_present: bool,
    /// `set:drivewrite=N`: after reading, the computer writes N blocks of
    /// `0xA5` at the start (refused unless the export is writable).
    pub write_blocks: u32,
    /// Writes the device refused.
    pub refused_writes: u32,
    /// Everything the computer read, in order (the whole medium once).
    pub read_back: Vec<u8>,
    /// Something changed for the UI.
    changed: bool,
}

impl Default for SimDrive {
    fn default() -> Self {
        SimDrive {
            info: DriveInfo::default(),
            name: String::new(),
            path: String::new(),
            table: vec![Extent::default(); POOL_EXTENTS],
            runs: 0,
            since_ms: 0,
            host_present: false,
            write_blocks: 0,
            refused_writes: 0,
            read_back: Vec::new(),
            changed: false,
        }
    }
}

impl SimDrive {
    fn fail(&mut self, r: DriveRefusal) {
        self.info.phase = DrivePhase::Failed;
        self.info.refusal = Some(r);
        self.changed = true;
    }

    /// The service's `drive_start`: the checks, then the map (as the
    /// firmware's card task).
    pub fn start(&mut self, req: DriveRequest, name: &str, test_mode: bool, media: &mut Media) {
        if self.info.phase.is_active() {
            return;
        }
        *self = SimDrive {
            host_present: self.host_present,
            write_blocks: self.write_blocks,
            ..SimDrive::default()
        };
        self.name = name.to_string();
        self.info.medium = req.medium;
        self.info.read_only = !req.write;
        let Some(path) = media.file_path(req.file) else { return self.fail(DriveRefusal::NotFound) };
        self.path = path;
        let vol = media.volume();
        let Ok(f) = block_on(vol.open(&self.path, OpenMode::Read)) else {
            return self.fail(DriveRefusal::NotFound);
        };
        let r = Request { medium: req.medium, write: req.write };
        let export =
            match image::check(r, image::under_orpheus(&self.path), test_mode, f.size(), f.valid_length()) {
                Ok(e) => e,
                Err(e) => return self.fail(e),
            };
        let mapped = {
            let mut t = ExtentTable::new(&mut self.table);
            block_on(image::map_file(vol, &f, &mut t)).map(|()| t.len())
        };
        match mapped {
            Ok(n) => self.runs = n,
            Err(e) => return self.fail(e),
        }
        self.info.runs = u16::try_from(self.runs).unwrap_or(u16::MAX);
        self.info.size = export.bytes;
        self.info.read_only = export.read_only;
        self.info.phase = DrivePhase::Preparing;
        self.changed = true;
    }

    pub fn stop(&mut self, media: &mut Media) {
        if self.info.phase == DrivePhase::Off {
            return;
        }
        // The card comes back: what the computer wrote is file data the
        // volume's cache must not hide.
        let vol = media.volume();
        for e in &self.table[..self.runs] {
            vol.invalidate_cached(e.lba, e.count);
        }
        self.info.phase = DrivePhase::Off;
        self.info.host = false;
        self.changed = true;
    }

    /// The fake computer's work for `ms` milliseconds.
    pub fn advance(&mut self, ms: u64, media: &mut Media) {
        match self.info.phase {
            DrivePhase::Preparing => {
                self.since_ms += ms;
                if self.since_ms >= PREPARE_MS {
                    self.info.phase = DrivePhase::Attached;
                    self.changed = true;
                }
            }
            DrivePhase::Attached => {
                if self.info.host != self.host_present {
                    self.info.host = self.host_present;
                    self.changed = true;
                }
                if self.host_present {
                    self.computer(ms, media);
                }
            }
            DrivePhase::Off | DrivePhase::Failed => {}
        }
    }

    fn computer(&mut self, ms: u64, media: &mut Media) {
        let bs = u64::from(self.info.medium.block_size());
        let Some(table) = ExtentTable::from_raw(&mut self.table, self.runs) else { return };
        let Ok(mut dev) = ImageDevice::new(
            Card(media.volume().device()),
            table,
            self.info.size,
            self.info.medium.block_size(),
        ) else {
            return;
        };
        dev.set_writable(!self.info.read_only);
        let total = dev.blocks(self.info.medium.is_cd());
        let mut budget = (ms * RATE).div_ceil(bs).max(1);
        let mut lba = self.read_back.len() as u64 / bs;
        while lba < total && budget > 0 {
            let n = budget.min(total - lba).min(64);
            let mut buf = vec![0u8; usize::try_from(n * bs).unwrap_or(0)];
            if block_on(dev.read(lba, &mut buf)).is_err() {
                break;
            }
            self.read_back.extend(buf);
            self.info.bytes_read += n * bs;
            lba += n;
            budget -= n;
            self.changed = true;
        }
        if lba >= total && self.write_blocks > 0 {
            let n = u64::from(self.write_blocks).min(dev.blocks(false));
            let buf = vec![0xA5u8; usize::try_from(n * bs).unwrap_or(0)];
            if block_on(dev.write(0, &buf)).is_ok() {
                self.info.bytes_written += n * bs;
            } else {
                self.refused_writes += 1;
            }
            self.write_blocks = 0;
            self.changed = true;
        }
    }

    /// The table as mapped (tests).
    #[must_use]
    pub fn runs(&self) -> &[Extent] {
        &self.table[..self.runs]
    }

    pub fn take_changed(&mut self) -> bool {
        core::mem::take(&mut self.changed)
    }

    /// The file a request names (tests).
    #[must_use]
    pub fn request(file: FileRef, medium: image::Medium, write: bool) -> DriveRequest {
        DriveRequest { file, medium, write }
    }
}
