//! Building a queue from a directory (04 §6.5: "a directory listing
//! materialised when playback starts"). The player itself only knows the
//! queue's length and identity; the firmware keeps the locators (in
//! `/.orpheus/queue.bin`, a window of it in RAM) and resolves an item index
//! when the player asks to open it.

use orpheus_fs::{BlockDevice, DirEntry, Volume};

use crate::store::TrackRef;

/// File extensions the built codecs play (case-insensitive).
pub const AUDIO_EXTENSIONS: [&str; 9] = ["mp3", "mp2", "flac", "wav", "aif", "aiff", "aifc", "au", "snd"];

/// Whether a file name looks playable.
#[must_use]
pub fn is_audio(name: &str) -> bool {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => AUDIO_EXTENSIONS.iter().any(|e| e.eq_ignore_ascii_case(ext)),
        _ => false,
    }
}

/// Hand every playable file of directory `path` (in directory order, not
/// recursive) to `sink`; returns how many there were. Sorting belongs to
/// the directory index (AU-12); `sink` typically appends to the queue file.
pub async fn queue_dir<D: BlockDevice>(
    vol: &mut Volume<'_, D>,
    path: &str,
    mut sink: impl FnMut(TrackRef),
) -> Result<u32, orpheus_fs::Error<D::Error>> {
    let serial = vol.serial();
    let mut cur = vol.open_dir(path).await?;
    let mut e = DirEntry::new();
    let mut n = 0u32;
    while vol.read_dir(&mut cur, &mut e).await? {
        if !e.is_dir() && is_audio(e.name()) {
            sink(TrackRef::new(serial, e.locator(), e.name()));
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions() {
        assert!(is_audio("a.MP3") && is_audio("x.flac") && is_audio("b.Wav"));
        assert!(!is_audio("cover.jpg") && !is_audio(".mp3") && !is_audio("mp3"));
    }
}
