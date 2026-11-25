//! The simulator's firmware update (UPD-1): the real installer
//! (`orpheus_update::install`) over a simulated NOR, fed from fake `.ofw`
//! files at the speed of the player's card and flash, so the Update screen
//! shows real phases and progress.
//!
//! Script keys: `set:upd=two` (default: a newer version on the card's top
//! folder, the running one in `/.orpheus/update/`), `set:upd=one`,
//! `set:upd=none`, `set:upd=bad` (the newer file is damaged: its SHA-256
//! check fails while checking), `set:upd=layout` (built for the dual-boot
//! layout).

use orpheus_apps::services::{UPDATE_FILES, UpdError, UpdPhase, UpdateFile, UpdateInfo, UpdateRequest};
use orpheus_proto::layout::full;
use orpheus_update::install::{Installer, Phase};
use orpheus_update::sim::{SimNor, image, ofw_header};
use orpheus_update::{Error, ofw, slots};

/// Simulated time for the search.
const SEARCH_MS: u64 = 400;
/// Card read while checking, flash write, read-back: bytes per ms.
const CHECK_BPMS: u64 = 64;
const WRITE_BPMS: u64 = 24;
const VERIFY_BPMS: u64 = 96;

struct File {
    name: &'static str,
    version: &'static str,
    changes: &'static str,
    header: [u8; ofw::HEADER_SIZE],
    image: Vec<u8>,
    installed: bool,
    damaged: bool,
}

pub struct SimUpdate {
    pub mode: &'static str,
    files: Vec<File>,
    phase: UpdPhase,
    error: UpdError,
    chosen: Option<usize>,
    details: Option<usize>,
    /// ms into the current phase (search, check).
    t: u64,
    pos: u64,
    nor: SimNor,
    inst: Option<Installer>,
    seq: u32,
}

impl Default for SimUpdate {
    fn default() -> Self {
        let mut nor = SimNor::new();
        // The running firmware in slot A.
        nor.load(full::SLOT_A, &image(1, 0x5000, 2));
        SimUpdate {
            mode: "two",
            files: Vec::new(),
            phase: UpdPhase::Idle,
            error: UpdError::None,
            chosen: None,
            details: None,
            t: 0,
            pos: 0,
            nor,
            inst: None,
            seq: 0,
        }
    }
}

const CHANGES: &str = "Update firmware from the card\nThe browser installer can update over USB\nFaster folder listing\nFixes: alarm after midnight";

fn file(name: &'static str, version: &'static str, changes: &'static str, seed: u8) -> File {
    let image = image(seed, 0x9000, 4);
    let header = ofw_header(&image, version, changes);
    File { name, version, changes, header, image, installed: false, damaged: false }
}

impl SimUpdate {
    fn changed(&mut self) {
        self.seq = self.seq.wrapping_add(1);
    }

    fn fail(&mut self, e: UpdError) {
        self.phase = UpdPhase::Failed;
        self.error = e;
        self.inst = None;
        self.changed();
    }

    fn found(&mut self) {
        self.files = match self.mode {
            "none" => Vec::new(),
            _ => {
                let mut newer = file("orpheus-0.2.0.ofw", "0.2.0", CHANGES, 2);
                newer.damaged = self.mode == "bad";
                if self.mode == "layout" {
                    let mut h = ofw::Header::parse(&newer.header).unwrap_or_else(|_| unreachable!());
                    h.layout = orpheus_proto::slot::LAYOUT_DUAL;
                    newer.header = h.to_bytes(CHANGES);
                }
                let mut v = vec![newer];
                if self.mode != "one" {
                    let mut cur = file("orpheus-0.1.0-sim.ofw", "0.1.0-sim", "", 1);
                    cur.installed = true;
                    v.push(cur);
                }
                v.truncate(UPDATE_FILES);
                v
            }
        };
    }

    pub fn info(&self) -> UpdateInfo {
        let permille = match self.phase {
            UpdPhase::Checking => {
                let len = self.chosen.and_then(|i| self.files.get(i)).map_or(1, |f| f.image.len() as u64);
                (self.pos * 1000 / len.max(1)).min(1000) as u16
            }
            UpdPhase::Writing | UpdPhase::Verifying => self.inst.as_ref().map_or(0, Installer::permille),
            UpdPhase::Done => 1000,
            _ => 0,
        };
        UpdateInfo {
            phase: self.phase,
            files: self.files.len() as u8,
            permille,
            error: self.error,
            seq: self.seq,
        }
    }

    pub fn file(&self, i: usize) -> Option<UpdateFile<'_>> {
        let f = self.files.get(i)?;
        Some(UpdateFile {
            name: f.name,
            version: f.version,
            size: f.image.len() as u32,
            installed: f.installed,
        })
    }

    pub fn changes(&self) -> &str {
        self.details.and_then(|i| self.files.get(i)).map_or("", |f| f.changes)
    }

    pub fn request(&mut self, r: UpdateRequest) {
        match r {
            UpdateRequest::Scan => {
                if matches!(self.phase, UpdPhase::Checking | UpdPhase::Writing | UpdPhase::Verifying) {
                    return;
                }
                self.phase = UpdPhase::Searching;
                self.error = UpdError::None;
                self.t = 0;
                self.changed();
            }
            UpdateRequest::Details(i) => {
                self.details = Some(usize::from(i));
                self.changed();
            }
            UpdateRequest::Install(i) => {
                let i = usize::from(i);
                let Some(f) = self.files.get(i) else { return };
                if let Err(e) = ofw::check(&f.header, &f.image[..128]) {
                    self.fail(ui_error(e));
                    return;
                }
                self.chosen = Some(i);
                self.phase = UpdPhase::Checking;
                self.pos = 0;
                self.changed();
            }
            UpdateRequest::Cancel => {
                if self.phase == UpdPhase::Checking {
                    self.phase = UpdPhase::Listed;
                    self.changed();
                }
            }
        }
    }

    pub fn advance(&mut self, ms: u64) {
        match self.phase {
            UpdPhase::Searching => {
                self.t += ms;
                if self.t >= SEARCH_MS {
                    self.found();
                    self.phase = UpdPhase::Listed;
                    self.changed();
                }
            }
            UpdPhase::Checking => self.check(ms),
            UpdPhase::Writing => self.write(ms),
            UpdPhase::Verifying => self.verify(ms),
            _ => {}
        }
    }

    fn check(&mut self, ms: u64) {
        let Some(i) = self.chosen else { return };
        let len = self.files[i].image.len() as u64;
        self.pos = (self.pos + ms * CHECK_BPMS).min(len);
        self.changed();
        if self.pos < len {
            return;
        }
        if self.files[i].damaged {
            self.fail(UpdError::Damaged);
            return;
        }
        let Ok(t) = slots::choose(true, Some(0)) else { return self.fail(UpdError::WrongPlayer) };
        let f = &self.files[i];
        match Installer::new(&f.header, &f.image[..128], t) {
            Ok(mut inst) => {
                if inst.start(&mut self.nor).is_err() {
                    return self.fail(UpdError::Write);
                }
                self.inst = Some(inst);
                self.pos = 0;
                self.phase = UpdPhase::Writing;
            }
            Err(e) => self.fail(ui_error(e)),
        }
    }

    fn write(&mut self, ms: u64) {
        let (Some(i), Some(inst)) = (self.chosen, self.inst.as_mut()) else { return };
        let img = &self.files[i].image;
        let end = (self.pos + ms * WRITE_BPMS).min(img.len() as u64) as usize;
        let start = self.pos as usize;
        if let Err(e) = inst.write(&mut self.nor, start as u32, &img[start..end]) {
            return self.fail(ui_error(e));
        }
        self.pos = end as u64;
        if inst.phase() == Phase::Verifying {
            self.phase = UpdPhase::Verifying;
        }
        self.changed();
    }

    fn verify(&mut self, ms: u64) {
        let Some(inst) = self.inst.as_mut() else { return };
        let mut buf = vec![0u8; (ms * VERIFY_BPMS).max(256) as usize];
        match inst.verify_step(&mut self.nor, &mut buf) {
            Ok(true) => match inst.commit(&mut self.nor) {
                Ok(()) => {
                    self.phase = UpdPhase::Done;
                    // The next boot starts slot B.
                    let mut r = self.nor.reader();
                    debug_assert_eq!(slots::stage2_pick(&mut r), Some(1));
                }
                Err(e) => return self.fail(ui_error(e)),
            },
            Ok(false) => {}
            Err(e) => return self.fail(ui_error(e)),
        }
        self.changed();
    }
}

fn ui_error(e: Error) -> UpdError {
    e.into()
}
