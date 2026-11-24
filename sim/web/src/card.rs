//! The browser's card: a FAT32 image formatted here and filled through
//! `orpheus-fs` (as the simulator's own card), with the demo set, the
//! visitor's files under `/Your files` and, when the previous visit left
//! one for the same set of files, the music library under
//! `/.orpheus/library`.

use std::sync::Arc;

use orpheus_fs::host::{RamDisk, block_on};
use orpheus_fs::{CacheSlot, MountOptions, OpenMode, Volume};
use orpheus_sim::pictures;

use crate::{demo, synth};

const SECTOR: usize = 512;
/// The smallest card: FAT32 needs at least 65 525 clusters (one sector
/// each here), so 36 MiB.
pub const MIN_BYTES: usize = 36 << 20;
/// Reserved sectors before the first FAT.
const RESERVED: u32 = 32;

/// Folder of the visitor's files.
pub const YOURS: &str = "/Your files";
/// The library's folder (`orpheus_library::LIBRARY_DIR`).
pub const LIBRARY: &str = orpheus_library::LIBRARY_DIR;

/// A FAT32 boot sector, FS information sector, their backups and two FATs
/// with the root directory's cluster allocated. One sector per cluster up
/// to 256 MiB, eight above.
#[must_use]
pub fn mkfs_fat32(sectors: u32) -> Vec<u8> {
    let mut img = vec![0u8; sectors as usize * SECTOR];
    let spc: u32 = if sectors > (256 << 20) / SECTOR as u32 { 8 } else { 1 };
    // FAT size: enough for every cluster the data area could hold.
    let clusters = (sectors - RESERVED) / spc;
    let fat = (clusters + 2).div_ceil(SECTOR as u32 / 4);
    let b = &mut img[..SECTOR];
    b[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    b[3..11].copy_from_slice(b"ORPHEUS ");
    b[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    b[13] = spc as u8;
    b[14..16].copy_from_slice(&(RESERVED as u16).to_le_bytes());
    b[16] = 2;
    b[21] = 0xF8;
    b[24..26].copy_from_slice(&32u16.to_le_bytes());
    b[26..28].copy_from_slice(&64u16.to_le_bytes());
    b[32..36].copy_from_slice(&sectors.to_le_bytes());
    b[36..40].copy_from_slice(&fat.to_le_bytes());
    b[44..48].copy_from_slice(&2u32.to_le_bytes());
    b[48..50].copy_from_slice(&1u16.to_le_bytes());
    b[50..52].copy_from_slice(&6u16.to_le_bytes());
    b[64] = 0x80;
    b[66] = 0x29;
    b[67..71].copy_from_slice(&0x4F52_5057u32.to_le_bytes());
    b[71..82].copy_from_slice(b"ORPHEUS WEB");
    b[82..90].copy_from_slice(b"FAT32   ");
    b[510] = 0x55;
    b[511] = 0xAA;
    let fsinfo = &mut img[SECTOR..2 * SECTOR];
    fsinfo[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    fsinfo[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    fsinfo[488..492].copy_from_slice(&u32::MAX.to_le_bytes());
    fsinfo[492..496].copy_from_slice(&u32::MAX.to_le_bytes());
    fsinfo[508..512].copy_from_slice(&0xAA55_0000u32.to_le_bytes());
    img.copy_within(0..2 * SECTOR, 6 * SECTOR);
    for f in 0..2 {
        let at = (RESERVED + f * fat) as usize * SECTOR;
        img[at..at + 12]
            .copy_from_slice(&[0xF8, 0xFF, 0xFF, 0x0F, 0xFF, 0xFF, 0xFF, 0x0F, 0xFF, 0xFF, 0xFF, 0x0F]);
    }
    img
}

/// One file to put on the card.
pub struct File {
    pub path: String,
    pub data: Vec<u8>,
}

/// The demo set (synthesised test tones included).
#[must_use]
pub fn demo_files() -> Vec<File> {
    let mut v = Vec::new();
    let mut add = |path: &str, data: Vec<u8>| v.push(File { path: path.to_string(), data });
    add("/About this card.txt", demo::ABOUT.as_bytes().to_vec());
    for (t, data) in synth::TRACKS.iter().zip(demo::ENCODED) {
        add(t.path, data.to_vec());
    }
    let lrc = synth::TRACKS[0].path.replace(".mp3", ".lrc");
    add(&lrc, demo::LYRE_LRC.as_bytes().to_vec());
    add("/Music/Test tones/Sweep 20 Hz to 20 kHz.wav", synth::wav(&synth::sweep(44_100, 10.0), 1));
    add("/Music/Test tones/Left and right.wav", synth::wav(&synth::left_right(22_050, 6.0), 2));
    add("/Pictures/Lyra.jpg", demo::LYRA_JPG.to_vec());
    let (w, h) = (240, 150);
    add("/Pictures/Lake.bmp", pictures::bmp(w, h, &pictures::lake(w, h)));
    add("/Pictures/Tree.qoi", pictures::qoi(96, 128, &pictures::tree(96, 128)));
    add("/Pictures/Aurora.qoi", pictures::qoi(w, h, &pictures::aurora(w, h)));
    add("/Books/The myth of Orpheus.txt", demo::MYTH.as_bytes().to_vec());
    v
}

/// A name for a visitor's file that the card accepts: no path, no
/// characters FAT forbids, at most 200 characters.
#[must_use]
pub fn clean_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let mut s: String =
        base.chars().map(|c| if c.is_control() || "\"*:<>?|".contains(c) { '_' } else { c }).collect();
    s = s.trim().trim_end_matches('.').to_string();
    if s.chars().count() > 200 {
        let (stem, ext) = s.rsplit_once('.').map_or((s.as_str(), ""), |(a, b)| (a, b));
        let stem: String = stem.chars().take(190).collect();
        s = if ext.is_empty() { stem } else { format!("{stem}.{ext}") };
    }
    if s.is_empty() { "file".to_string() } else { s }
}

/// Folders on the way to `path`, outermost first.
fn parents(path: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut at = 0;
    while let Some(i) = path[at + 1..].find('/') {
        at += i + 1;
        v.push(path[..at].to_string());
    }
    v
}

/// Build the card: the demo set, the visitor's files (renamed if their
/// name is taken) and the library files (`name` relative to the library
/// folder). Returns the image and the paths of the visitor's files.
pub fn build(user: &[File], library: &[File]) -> Result<(Arc<[u8]>, Vec<String>), String> {
    let demo = demo_files();
    let content: usize = demo.iter().chain(user).chain(library).map(|f| f.data.len() + 4096).sum();
    let bytes = (content + content / 8 + (8 << 20)).max(MIN_BYTES).next_multiple_of(4 << 20);
    let sectors =
        u32::try_from(bytes / SECTOR).map_err(|_| "the files are too large for a card".to_string())?;
    let mut cache = vec![CacheSlot::EMPTY; 32];
    let mut vol = block_on(Volume::mount(
        RamDisk::from_bytes(mkfs_fat32(sectors)),
        &mut cache,
        MountOptions::default(),
    ))
    .map_err(|e| format!("mount the new card: {e:?}"))?;
    vol.set_time(orpheus_fs::DateTime {
        year: 2026,
        month: 11,
        day: 24,
        hour: 12,
        minute: 0,
        second: 0,
        centis: 0,
    });
    let mut dirs = std::collections::BTreeSet::new();
    let mut put = |vol: &mut Volume<'_, RamDisk>, path: &str, data: &[u8]| -> Result<(), String> {
        for d in parents(path) {
            if dirs.insert(d.clone()) {
                block_on(vol.create_dir(&d)).map_err(|e| format!("{d}: {e:?}"))?;
            }
        }
        let mut f = block_on(vol.open(path, OpenMode::CreateNew)).map_err(|e| format!("{path}: {e:?}"))?;
        if !data.is_empty() {
            block_on(vol.write(&mut f, data)).map_err(|e| format!("{path}: {e:?}"))?;
        }
        block_on(vol.close(f)).map_err(|e| format!("{path}: {e:?}"))
    };
    for f in &demo {
        put(&mut vol, &f.path, &f.data)?;
    }
    let mut paths = Vec::new();
    let mut taken = std::collections::BTreeSet::new();
    for f in user {
        let name = clean_name(&f.path);
        let mut path = format!("{YOURS}/{name}");
        let mut n = 2;
        while !taken.insert(path.to_lowercase()) {
            let (stem, ext) = name.rsplit_once('.').map_or((name.as_str(), ""), |(a, b)| (a, b));
            path = if ext.is_empty() {
                format!("{YOURS}/{stem} ({n})")
            } else {
                format!("{YOURS}/{stem} ({n}).{ext}")
            };
            n += 1;
        }
        put(&mut vol, &path, &f.data)?;
        paths.push(path);
    }
    if user.is_empty() {
        block_on(vol.create_dir(YOURS)).map_err(|e| format!("{YOURS}: {e:?}"))?;
    }
    for f in library {
        let name = clean_name(&f.path);
        put(&mut vol, &format!("{LIBRARY}/{name}"), &f.data)?;
    }
    block_on(vol.flush()).map_err(|e| format!("flush: {e:?}"))?;
    Ok((Arc::from(vol.into_device().data), paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_name("C:\\music\\a:b?.mp3"), "a_b_.mp3");
        assert_eq!(clean_name("../x/song.ogg"), "song.ogg");
        assert_eq!(clean_name(" . "), "file");
        assert_eq!(clean_name(&"x".repeat(300)).chars().count(), 190);
    }

    #[test]
    fn parents_of_a_path() {
        assert_eq!(parents("/a/b/c.txt"), ["/a", "/a/b"]);
        assert!(parents("/c.txt").is_empty());
    }

    #[test]
    fn card_is_fat32_with_the_demo_and_the_visitors_files() {
        let user = [
            File { path: "song.mp3".into(), data: vec![1; 3000] },
            File { path: "song.mp3".into(), data: vec![2; 10] },
        ];
        let (img, paths) = build(&user, &[]).unwrap();
        assert_eq!(paths, ["/Your files/song.mp3", "/Your files/song (2).mp3"]);
        let mut cache = vec![CacheSlot::EMPTY; 8];
        let mut vol =
            block_on(Volume::mount(RamDisk::from_bytes(img.to_vec()), &mut cache, MountOptions::default()))
                .unwrap();
        assert_eq!(vol.kind(), orpheus_fs::FsKind::Fat32);
        for f in demo_files() {
            let mut h =
                block_on(vol.open(&f.path, OpenMode::Read)).unwrap_or_else(|e| panic!("{}: {e:?}", f.path));
            assert_eq!(h.size() as usize, f.data.len(), "{}", f.path);
            let mut buf = vec![0; f.data.len()];
            block_on(vol.read_exact_or_eof(&mut h, &mut buf)).unwrap();
            assert!(buf == f.data, "{}", f.path);
        }
        let h = block_on(vol.open("/Your files/song (2).mp3", OpenMode::Read)).unwrap();
        assert_eq!(h.size(), 10);
    }
}
