//! The simulator's Sequencer back end (M17, 05 §7.16, SEQ-1, SEQ-2): the
//! `orpheus_seq::Studio` the screens read and edit, the synth engine it
//! plays on, its project files on the simulated card under
//! `/.orpheus/seq/` (`SEQnn.OSQ`) and the song's exports next to them
//! (`SEQnn.WAV`, `SEQnn.MID`), written through `orpheus-fs` as the
//! firmware's card task would (the WAV in 4096-frame pieces as it is
//! rendered).
//!
//! Audio: while the project plays or notes from the keys sound (and a
//! second after, for the tails) the simulated time is rendered through
//! `Studio::render_i16`, in 1024-frame blocks, whatever screen is shown
//! and whether the screen is on — the screen-off path of the firmware's
//! synth task.
//!
//! Script hooks (`set:`): `seq=on|off` puts the Sequencer on Home (off by
//! default: not on the player yet); `seqdemo=1` writes the demo project
//! ("Night Bus", `orpheus_seq::demo::beat`) to the card as the next free
//! file, `seqsong=1` the SEQ-2 demo song ("Tram Lights",
//! `demo::song`); `seqcapture=on|off` keeps the rendered audio
//! (`SimSeq::capture`); `seqfail=on|off` makes card writes fail;
//! `seqdry=on|off` plays without the engine (the notes only: the settings
//! matrix's fast journeys).

use orpheus_apps::seq::SeqResult;
use orpheus_fs::host::block_on;
use orpheus_fs::{DirEntry, OpenMode};
use orpheus_seq::export::{self, WavExport};
use orpheus_seq::file::{self, MAX_FILE};
use orpheus_seq::{DIR, Project, StepLen, Studio, demo, file_name, midi_name, parse_file_name, wav_name};
use orpheus_synth::Engine;

use crate::media::Media;

/// The simulator's synth rate.
pub const RATE: u32 = 48_000;
/// Frames rendered after a stop (the drums' tails).
const TAIL: u64 = RATE as u64;
/// Render block, frames.
const BLOCK: usize = 1024;

pub struct SimSeq {
    pub studio: Box<Studio>,
    engine: Box<Engine>,
    /// The last request's outcome.
    pub result: SeqResult,
    /// Frames rendered (tests).
    pub frames: u64,
    /// `set:seqcapture=on`: the rendered audio (interleaved stereo).
    pub capture: Option<Vec<i16>>,
    /// Card writes of project files (tests).
    pub writes: u32,
    /// `set:seqfail=on`: writes fail.
    pub fail: bool,
    /// `set:seqdry=on`: no engine, the notes only.
    pub dry: bool,
    /// `set:seqdry=on`: the notes played (tests).
    pub notes: Vec<orpheus_seq::Hit>,
    /// `set:seqnote=1`: the open project's kit, first pattern length and
    /// swing at that moment (the settings matrix's probe).
    pub noted: Option<String>,
    ms: u64,
    tail: u64,
    block: Vec<i16>,
}

impl Default for SimSeq {
    fn default() -> Self {
        SimSeq {
            studio: Box::new(Studio::new(RATE)),
            engine: Box::new(Engine::new(RATE, 1)),
            result: SeqResult::None,
            frames: 0,
            capture: None,
            writes: 0,
            fail: false,
            dry: false,
            notes: Vec::new(),
            noted: None,
            ms: 0,
            tail: 0,
            block: vec![0; 2 * BLOCK],
        }
    }
}

fn path(no: u8) -> String {
    named(&file_name(no))
}

fn named(name: &[u8; 9]) -> String {
    format!("{DIR}/{}", std::str::from_utf8(name).unwrap_or("SEQ00.OSQ"))
}

impl SimSeq {
    /// Let `ms` of simulated time pass: render it while playing.
    pub fn advance(&mut self, ms: u64) {
        let start = self.ms * u64::from(RATE) / 1000;
        self.ms += ms;
        let end = self.ms * u64::from(RATE) / 1000;
        let mut n = end - start;
        if self.studio.seq.busy() {
            self.tail = TAIL;
        } else if self.tail == 0 {
            return;
        } else {
            n = n.min(self.tail);
            self.tail -= n;
        }
        while n > 0 {
            let k = (n as usize).min(BLOCK);
            if self.dry {
                let notes = &mut self.notes;
                let st = &mut *self.studio;
                st.seq.run_dry(&st.project, k, |h| notes.push(h));
            } else {
                let out = &mut self.block[..2 * k];
                self.studio.render_i16(&mut self.engine, out);
                if let Some(c) = self.capture.as_mut() {
                    c.extend_from_slice(out);
                }
            }
            self.frames += k as u64;
            n -= k as u64;
        }
    }

    /// Export the open project's song (saved first if it is new) as
    /// `SEQnn.WAV` (`wav`) or `SEQnn.MID` next to it, with `step_len`.
    pub fn export(&mut self, media: &mut Media, wav: bool, step_len: StepLen) -> SeqResult {
        if self.studio.open.is_none() && self.save(media) != SeqResult::Done {
            return SeqResult::Failed;
        }
        let Some(no) = self.studio.open else { return SeqResult::Failed };
        if self.fail {
            return SeqResult::Failed;
        }
        let p = &self.studio.project;
        if !wav {
            let mut buf = vec![0u8; 4 << 20];
            let Ok(n) = export::midi(p, step_len, &mut buf) else { return SeqResult::Failed };
            return if write(media, &named(&midi_name(no)), &buf[..n]) {
                SeqResult::Done
            } else {
                SeqResult::Failed
            };
        }
        // Offline: its own player and engine, in pieces, as the card task
        // would write them.
        let mut x = WavExport::new(p, RATE, step_len);
        let mut e = Box::new(Engine::new(RATE, 1));
        let vol = media.volume();
        let _ = block_on(vol.create_dir("/.orpheus"));
        let _ = block_on(vol.create_dir(DIR));
        let Ok(mut f) = block_on(vol.open(&named(&wav_name(no)), OpenMode::Truncate)) else {
            return SeqResult::Failed;
        };
        let mut ok = block_on(vol.write(&mut f, &x.header())).is_ok();
        let mut pcm = vec![0i16; 2 * 4096];
        let mut bytes = Vec::with_capacity(4 * 4096);
        while ok && !x.finished() {
            let k = x.render(p, &mut e, &mut pcm);
            bytes.clear();
            for s in &pcm[..2 * k] {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            ok = block_on(vol.write(&mut f, &bytes)).is_ok_and(|n| n == bytes.len());
        }
        let _ = block_on(vol.close(f));
        let _ = block_on(vol.flush());
        if ok { SeqResult::Done } else { SeqResult::Failed }
    }

    /// Write the SEQ-2 demo song as the next free file (`set:seqsong=1`).
    pub fn add_song(&mut self, media: &mut Media) -> Option<u8> {
        self.list(media);
        let no = self.studio.list.free_no()?;
        let mut p = Box::new(Project::NEW);
        demo::song(&mut p);
        let mut buf = vec![0u8; MAX_FILE];
        let n = file::save(&p, &mut buf).ok()?;
        write(media, &path(no), &buf[..n]).then_some(no)
    }

    /// Read the project list from the card.
    pub fn list(&mut self, media: &mut Media) {
        self.studio.list.clear();
        let vol = media.volume();
        let Ok(mut cur) = block_on(vol.open_dir(DIR)) else { return };
        let mut e = DirEntry::new();
        let mut nos = Vec::new();
        while let Ok(true) = block_on(vol.read_dir(&mut cur, &mut e)) {
            if !e.is_dir()
                && let Some(no) = parse_file_name(e.name())
            {
                nos.push(no);
            }
        }
        for no in nos {
            if let Some(d) = crate::games::read(media, &path(no))
                && let Ok(name) = file::peek_name(&d)
            {
                self.studio.list.put(no, name);
            }
        }
    }

    /// Open project file `no`.
    pub fn open(&mut self, media: &mut Media, no: u8) -> SeqResult {
        self.studio.seq.stop();
        let Some(d) = crate::games::read(media, &path(no)) else { return SeqResult::Failed };
        let mut p = Box::new(Project::NEW);
        if file::load_into(&mut p, &d).is_err() {
            return SeqResult::Failed;
        }
        self.studio.project = *p;
        self.studio.open = Some(no);
        self.studio.saved_rev = self.studio.project.rev();
        SeqResult::Done
    }

    /// Save the open project (a new one takes the lowest free number).
    pub fn save(&mut self, media: &mut Media) -> SeqResult {
        let Some(no) = self.studio.open.or_else(|| self.studio.list.free_no()) else {
            return SeqResult::Failed;
        };
        let mut buf = vec![0u8; MAX_FILE];
        let Ok(n) = file::save(&self.studio.project, &mut buf) else { return SeqResult::Failed };
        if self.fail || !write(media, &path(no), &buf[..n]) {
            return SeqResult::Failed;
        }
        self.writes += 1;
        self.studio.open = Some(no);
        self.studio.saved_rev = self.studio.project.rev();
        let name = file::clean(self.studio.project.name().as_bytes());
        self.studio.list.put(no, name);
        SeqResult::Done
    }

    /// Delete project file `no`.
    pub fn delete(&mut self, media: &mut Media, no: u8) -> SeqResult {
        let vol = media.volume();
        let p = path(no);
        let ok = block_on(vol.remove(&p)).is_ok();
        let _ = block_on(vol.flush());
        self.studio.list.remove(no);
        if self.studio.open == Some(no) {
            self.studio.seq.stop();
            self.studio.open = None;
            self.studio.project.reset();
        }
        if ok { SeqResult::Done } else { SeqResult::Failed }
    }

    /// Write the demo project as the next free file (`set:seqdemo=1`).
    pub fn add_demo(&mut self, media: &mut Media) -> Option<u8> {
        self.list(media);
        let no = self.studio.list.free_no()?;
        let mut p = Box::new(Project::NEW);
        demo::beat(&mut p);
        let mut buf = vec![0u8; MAX_FILE];
        let n = file::save(&p, &mut buf).ok()?;
        write(media, &path(no), &buf[..n]).then_some(no)
    }
}

/// Write a file of the simulated card (only under `/.orpheus/seq`; tests
/// put older project files there with it).
pub fn write(media: &mut Media, path: &str, data: &[u8]) -> bool {
    assert!(path.starts_with(DIR), "the Sequencer writes only under {DIR}");
    let vol = media.volume();
    let _ = block_on(vol.create_dir("/.orpheus"));
    let _ = block_on(vol.create_dir(DIR));
    let Ok(mut f) = block_on(vol.open(path, OpenMode::Truncate)) else { return false };
    let ok = block_on(vol.write(&mut f, data)).is_ok_and(|n| n == data.len());
    let _ = block_on(vol.close(f));
    let _ = block_on(vol.flush());
    ok
}
