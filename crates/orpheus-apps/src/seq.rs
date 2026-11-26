//! The Sequencer app's service and helpers (05 §7.16, SEQ-1; feature
//! `seq`). The model, the file and the step player are `orpheus-seq`; the
//! screens are `screens::seq`.
//!
//! The service holds an [`orpheus_seq::Studio`] (the open project, the
//! player, the saved projects) the screens read and edit directly; card
//! work goes through [`SeqService::seq_request`] and is answered by
//! changing the studio (the list, the open project) and an
//! `other::SEQ` service event. Rendering is the service's: the firmware's
//! synth task (SYN-3) or the simulator calls `Studio::render_i16` into
//! the output, whatever the screen shows or whether it is on.

pub use orpheus_seq as core;
use orpheus_seq::Studio;
use orpheus_seq::model::{TEMPO_MAX, TEMPO_MIN, note_label};
use orpheus_ui::text::StrBuf;

/// Card work the screens ask for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SeqRequest {
    /// Read the project list (`Studio::list`) from `/.orpheus/seq/`.
    List,
    /// Open saved project file `no` (stopping and saving the open one).
    Open(u8),
    /// Save the open project (a new one takes the lowest free number);
    /// the list follows.
    Save,
    /// Delete saved project file `no` (closing it if open).
    Delete(u8),
}

/// What the last request did (`SeqService::seq_result`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SeqResult {
    /// Nothing asked yet, or still working.
    #[default]
    None,
    /// Done.
    Done,
    /// The file could not be read, written or removed.
    Failed,
}

/// The Sequencer's service (part of `Services` with the feature).
pub trait SeqService {
    fn seq(&self) -> &Studio;
    fn seq_mut(&mut self) -> &mut Studio;
    fn seq_request(&mut self, r: SeqRequest);
    /// The outcome of the last request.
    fn seq_result(&self) -> SeqResult;
}

/// `tempo` (1/100 BPM) as "98" or "93.5" (one decimal when not whole).
pub fn push_tempo<const N: usize>(b: &mut StrBuf<N>, tempo: u16) {
    let t10 = (u32::from(tempo) + 5) / 10;
    b.push_u32(t10 / 10);
    if t10 % 10 != 0 {
        b.push('.');
        b.push_u32(t10 % 10);
    }
}

/// Tempo one step up or down: whole BPM (a fractional tempo goes to the
/// next whole one), `big` 10 BPM.
#[must_use]
pub fn tempo_step(tempo: u16, up: bool, big: bool) -> u16 {
    let d: i32 = if big { 1000 } else { 100 };
    let t = i32::from(tempo);
    let next = if up { (t / 100) * 100 + d } else { ((t + 99) / 100) * 100 - d };
    next.clamp(i32::from(TEMPO_MIN), i32::from(TEMPO_MAX)) as u16
}

/// A track's label: "BD Kick".
pub fn push_track<const N: usize>(b: &mut StrBuf<N>, note: u8) {
    let (short, long) = note_label(note);
    b.push_str(short);
    b.push(' ');
    b.push_str(long);
}

/// A new project's name: "Beat 3" for file number 3.
pub fn new_name(no: u8) -> StrBuf<16> {
    let mut b = StrBuf::new();
    b.push_str("Beat ");
    b.push_u32(u32::from(no));
    b
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn tempo_text_and_steps() {
        let s = |t| {
            let mut b = StrBuf::<8>::new();
            push_tempo(&mut b, t);
            std::string::String::from(b.as_str())
        };
        assert_eq!(s(12_000), "120");
        assert_eq!(s(9_350), "93.5");
        assert_eq!(s(9_996), "100", "rounded to tenths");
        assert_eq!(s(9_904), "99");
        assert_eq!(tempo_step(12_000, true, false), 12_100);
        assert_eq!(tempo_step(9_350, true, false), 9_400);
        assert_eq!(tempo_step(9_350, false, false), 9_300);
        assert_eq!(tempo_step(4_000, false, true), 4_000);
        assert_eq!(tempo_step(23_900, true, true), 24_000);
        assert_eq!(new_name(3).as_str(), "Beat 3");
    }
}
