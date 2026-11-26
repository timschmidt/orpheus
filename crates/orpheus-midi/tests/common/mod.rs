//! Helpers shared by the integration tests.

#![allow(dead_code)]

use orpheus_midi::write::Writer;
use orpheus_midi::{Division, MidiError, MidiEvent, MidiPlayer, MidiSource, Options, Pull, SliceSource, Smf};

/// A file written by `f` (format, division given).
pub fn file(format: u16, division: Division, f: impl FnOnce(&mut Writer<'_>)) -> Vec<u8> {
    let mut buf = vec![0u8; 256 * 1024];
    let mut w = Writer::new(&mut buf, format, division);
    f(&mut w);
    let n = w.finish().expect("fits");
    buf.truncate(n);
    buf
}

/// A demo song.
pub fn demo(f: fn(&mut [u8]) -> Option<usize>) -> Vec<u8> {
    let mut buf = vec![0u8; 64 * 1024];
    let n = f(&mut buf).expect("fits");
    buf.truncate(n);
    buf
}

/// Every event of `data` in order, or the open error; also the number of
/// `next` calls made (each a bounded step).
pub fn events(data: &[u8]) -> Result<(Vec<MidiEvent>, usize), MidiError> {
    let mut src = SliceSource::new(data);
    let mut smf = Smf::new();
    smf.begin(src.len());
    let mut buf = [0u8; 64];
    loop {
        match smf.open()? {
            Pull::Need(n) => {
                let k = src.read_at(n.offset, &mut buf[..usize::from(n.len)])?;
                smf.supply(&buf[..k]);
            }
            Pull::Ready(_) => break,
            Pull::End => unreachable!(),
        }
    }
    let mut out = Vec::new();
    let mut calls = 0;
    loop {
        calls += 1;
        match smf.next_event() {
            Pull::Need(n) => {
                let k = src.read_at(n.offset, &mut buf[..usize::from(n.len)])?;
                smf.supply(&buf[..k]);
            }
            Pull::Ready(e) => out.push(e),
            Pull::End => return Ok((out, calls)),
        }
        assert!(calls <= 4 * data.len() + 1000, "no progress");
    }
}

/// A boxed player that opened `data`.
pub fn player(data: &[u8], opts: Options, kar: bool) -> Box<MidiPlayer> {
    let mut p = Box::new(MidiPlayer::new(opts));
    p.open_from(&mut SliceSource::new(data), kar).expect("opens");
    p
}

/// Render `frames` frames (or to the end) in blocks of `block`.
pub fn render(p: &mut MidiPlayer, data: &[u8], frames: usize, block: usize) -> Vec<i16> {
    let mut src = SliceSource::new(data);
    let mut out = vec![0i16; 2 * frames];
    let mut done = 0;
    while done < frames {
        let hi = (done + block).min(frames);
        let n = p.render_from(&mut src, &mut out[2 * done..2 * hi]).unwrap();
        if n == 0 {
            break;
        }
        done += n;
    }
    out.truncate(2 * done);
    out
}

/// Open and render the whole song.
pub fn render_song(data: &[u8], opts: Options, kar: bool) -> Vec<i16> {
    let mut p = player(data, opts, kar);
    let d = p.duration() as usize;
    render(&mut p, data, d, 1152)
}

/// FNV-1a over the samples' little-endian bytes.
pub fn hash(samples: &[i16]) -> u64 {
    orpheus_synth::demo::hash_i16(samples)
}

/// RMS of a stretch, dBFS.
pub fn rms_db(s: &[i16]) -> f64 {
    if s.is_empty() {
        return -200.0;
    }
    let e: f64 = s.iter().map(|&x| f64::from(x) * f64::from(x)).sum::<f64>() / s.len() as f64;
    10.0 * (e / (32768.0 * 32768.0) + 1e-20).log10()
}
