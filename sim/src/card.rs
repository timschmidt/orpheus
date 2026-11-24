//! The simulated microSD card: a FAT16 image formatted here (deterministic,
//! no external tools) and filled through `orpheus-fs` itself, so browsing in
//! the simulator goes through the real file-system code. Built once per
//! process; every simulator mounts a copy-on-write overlay of it.
//!
//! Contents: music in Latin, Cyrillic and Japanese folders with ID3v2 tags
//! and `.lrc` lyrics, a 5000-file folder, text files, other file types the
//! browser recognises (pictures that decode, and two that do not), an
//! 8.3-only name, hidden entries.

use std::sync::OnceLock;

use orpheus_fs::host::{RamDisk, block_on};
use orpheus_fs::{CacheSlot, MountOptions, OpenMode, Volume};

use crate::pictures;

/// Image size: 16 MiB, FAT16 with 2 KiB clusters.
pub const SECTORS: u32 = 32 * 1024;

/// A FAT16 boot sector, FATs and an empty root directory.
#[must_use]
pub fn mkfs_fat16(sectors: u32) -> Vec<u8> {
    let mut img = vec![0u8; sectors as usize * 512];
    let spc = 4u8;
    let reserved = 4u16;
    let root_entries = 512u16;
    let fat_sectors = 32u16;
    let b = &mut img[..512];
    b[0..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
    b[3..11].copy_from_slice(b"ORPHESIM");
    b[11..13].copy_from_slice(&512u16.to_le_bytes());
    b[13] = spc;
    b[14..16].copy_from_slice(&reserved.to_le_bytes());
    b[16] = 2;
    b[17..19].copy_from_slice(&root_entries.to_le_bytes());
    let tot16 = u16::try_from(sectors).unwrap_or(0);
    b[19..21].copy_from_slice(&tot16.to_le_bytes());
    b[21] = 0xF8;
    b[22..24].copy_from_slice(&fat_sectors.to_le_bytes());
    b[24..26].copy_from_slice(&32u16.to_le_bytes());
    b[26..28].copy_from_slice(&2u16.to_le_bytes());
    if tot16 == 0 {
        b[32..36].copy_from_slice(&sectors.to_le_bytes());
    }
    b[36] = 0x80;
    b[38] = 0x29;
    b[39..43].copy_from_slice(&0x5953_4D31u32.to_le_bytes());
    b[43..54].copy_from_slice(b"ORPHEUS SIM");
    b[54..62].copy_from_slice(b"FAT16   ");
    b[510] = 0x55;
    b[511] = 0xAA;
    for f in 0..2usize {
        let at = (usize::from(reserved) + f * usize::from(fat_sectors)) * 512;
        img[at..at + 4].copy_from_slice(&[0xF8, 0xFF, 0xFF, 0xFF]);
    }
    img
}

/// A minimal ID3v2.3 tag with UTF-16 text frames, then fake frame bytes.
#[must_use]
pub fn mp3_with_tags(title: &str, artist: &str, album: &str, track: u16) -> Vec<u8> {
    let mut frames = Vec::new();
    let mut frame = |id: &[u8; 4], text: &str| {
        let mut body = vec![1u8, 0xFF, 0xFE];
        for u in text.encode_utf16() {
            body.extend(u.to_le_bytes());
        }
        frames.extend(id);
        frames.extend(u32::try_from(body.len()).unwrap_or(0).to_be_bytes());
        frames.extend([0, 0]);
        frames.extend(body);
    };
    frame(b"TIT2", title);
    frame(b"TPE1", artist);
    frame(b"TALB", album);
    frame(b"TRCK", &track.to_string());
    let n = frames.len();
    let syncsafe = |v: usize| -> [u8; 4] {
        let v = u32::try_from(v).unwrap_or(0);
        [((v >> 21) & 0x7F) as u8, ((v >> 14) & 0x7F) as u8, ((v >> 7) & 0x7F) as u8, (v & 0x7F) as u8]
    };
    let mut out = b"ID3\x03\x00\x00".to_vec();
    out.extend(syncsafe(n));
    out.extend(frames);
    // One MPEG-1 layer III frame header (128 kbit/s, 44.1 kHz) and padding.
    out.extend([0xFF, 0xFB, 0x90, 0x64]);
    out.resize(out.len() + 413, 0);
    out
}

/// (path, title, artist, album, track, lyrics)
pub type TrackRow = (&'static str, &'static str, &'static str, &'static str, u16, Option<&'static str>);

pub const TRACKS: &[TrackRow] = &[
    ("/Music/Sigur Rós/Takk…/01 Glósóli.mp3", "Glósóli", "Sigur Rós", "Takk…", 1, None),
    (
        "/Music/Sigur Rós/Takk…/02 Hoppípolla.mp3",
        "Hoppípolla",
        "Sigur Rós",
        "Takk…",
        2,
        Some(
            "[00:00.00]Brosandi\n[00:12.00]hendumst í hringi\n[00:24.00]Höldumst í hendur\n[00:36.00]allur heimurinn óskýr\n[00:48.00]nema þú stingur þér út úr\n[01:00.00]Rennblaut, allur rennvotur\n[01:12.00]engin gúmmístígvél\n[01:24.00]Hlaupandi inn í okkur\n[01:36.00]vitum að við getum meitt okkur\n",
        ),
    ),
    ("/Music/Sigur Rós/Takk…/03 Með blóðnasir.mp3", "Með blóðnasir", "Sigur Rós", "Takk…", 3, None),
    (
        "/Music/Кино/Звезда по имени Солнце/01 Звезда по имени Солнце.mp3",
        "Звезда по имени Солнце",
        "Кино",
        "Звезда по имени Солнце",
        1,
        Some(
            "[00:00.00]Белый снег, серый лёд\n[00:10.00]На растрескавшейся земле\n[00:20.00]Одеялом лоскутным на ней\n[00:30.00]Город в дорожной петле\n[00:40.00]А над городом плывут облака\n[00:50.00]Закрывая небесный свет\n[01:00.00]А над городом — жёлтый дым\n",
        ),
    ),
    (
        "/Music/Кино/Звезда по имени Солнце/02 Пачка сигарет.mp3",
        "Пачка сигарет",
        "Кино",
        "Звезда по имени Солнце",
        2,
        None,
    ),
    ("/Music/Кино/Звезда по имени Солнце/03 Апрель.mp3", "Апрель", "Кино", "Звезда по имени Солнце", 3, None),
    (
        "/Music/YOASOBI/THE BOOK/01 夜に駆ける.mp3",
        "夜に駆ける",
        "YOASOBI",
        "THE BOOK",
        1,
        Some(
            "[00:00.00]沈むように溶けてゆくように\n[00:08.00]二人だけの空が広がる夜に\n[00:16.00]「さよなら」だけだった\n[00:24.00]その一言で全てが分かった\n[00:32.00]日が沈み出した空と君の姿\n[00:40.00]フェンス越しに重なっていた\n",
        ),
    ),
    ("/Music/YOASOBI/THE BOOK/02 群青.mp3", "群青", "YOASOBI", "THE BOOK", 2, None),
    ("/Music/YOASOBI/THE BOOK/03 ハルジオン.mp3", "ハルジオン", "YOASOBI", "THE BOOK", 3, None),
];

const WORDS: [&str; 26] = [
    "Aurora", "Breeze", "Cascade", "Dawn", "Ember", "Fjord", "Glacier", "Harbor", "Island", "Jungle",
    "Kestrel", "Lagoon", "Meadow", "Nebula", "Orchid", "Prairie", "Quartz", "River", "Summit", "Tundra",
    "Umbra", "Valley", "Willow", "Xenon", "Yonder", "Zephyr",
];

/// Files in `/Music/Big folder` (05 §7.2 acceptance: 5000 entries).
pub const BIG: usize = 5000;

const BOOK_EN: &str = "It was the best of times, it was the worst of times, it was the age of wisdom, it was the age of foolishness, it was the epoch of belief, it was the epoch of incredulity, it was the season of Light, it was the season of Darkness, it was the spring of hope, it was the winter of despair, we had everything before us, we had nothing before us, we were all going direct to Heaven, we were all going direct the other way.\n\nIn short, the period was so far like the present period, that some of its noisiest authorities insisted on its being received, for good or for evil, in the superlative degree of comparison only.\n\nThere were a king with a large jaw and a queen with a plain face, on the throne of England; there were a king with a large jaw and a queen with a fair face, on the throne of France. In both countries it was clearer than crystal to the lords of the State preserves of loaves and fishes, that things in general were settled for ever.\n";
const BOOK_RU: &str = "— Eh bien, mon prince. Gênes et Lucques ne sont plus que des apanages, des поместья, de la famille Buonaparte.\n\nТак говорила в июле 1805 года известная Анна Павловна Шерер, фрейлина и приближённая императрицы Марии Феодоровны, встречая важного и чиновного князя Василия, первого приехавшего на её вечер.\n\nАнна Павловна кашляла несколько дней, у неё был грипп, как она говорила (грипп был тогда новое слово, употреблявшееся только редкими).\n";
const BOOK_JA: &str = "春は、あけぼの。やうやう白くなりゆく山ぎは、すこしあかりて、紫だちたる雲の細くたなびきたる。\n\n夏は、夜。月の頃はさらなり、闇もなほ、蛍の多く飛びちがひたる。また、ただ一つ二つなど、ほのかにうち光りて行くもをかし。雨など降るもをかし。\n\n秋は、夕暮れ。夕日のさして山の端いと近うなりたるに、烏の寝どころへ行くとて、三つ四つ、二つ三つなど、飛びいそぐさへあはれなり。\n";

fn build() -> Vec<u8> {
    let img = mkfs_fat16(SECTORS);
    let mut cache: Vec<CacheSlot> = vec![CacheSlot::EMPTY; 32];
    let mut vol = block_on(Volume::mount(RamDisk::from_bytes(img), &mut cache, MountOptions::default()))
        .expect("mount the fresh image");
    let mut day = 0u8;
    let mut write = |vol: &mut Volume<'_, RamDisk>, path: &str, data: &[u8]| {
        // Spread modification times over 2026 so "Sort by date" means something.
        day = day.wrapping_add(7);
        vol.set_time(orpheus_fs::DateTime {
            year: 2026,
            month: 1 + (day / 28) % 12,
            day: 1 + day % 28,
            hour: 9 + day % 12,
            minute: day % 60,
            second: 0,
            centis: 0,
        });
        let mut f = block_on(vol.open(path, OpenMode::CreateNew)).unwrap_or_else(|e| panic!("{path}: {e:?}"));
        if !data.is_empty() {
            block_on(vol.write(&mut f, data)).expect("write");
        }
        block_on(vol.close(f)).expect("close");
    };
    let dirs = [
        "/Music",
        "/Music/Sigur Rós",
        "/Music/Sigur Rós/Takk…",
        "/Music/Кино",
        "/Music/Кино/Звезда по имени Солнце",
        "/Music/YOASOBI",
        "/Music/YOASOBI/THE BOOK",
        "/Music/Big folder",
        "/Music/Podcasts",
        "/Books",
        "/Pictures",
        "/Images",
        "/Recordings",
        "/.orpheus",
    ];
    for d in dirs {
        block_on(vol.create_dir(d)).unwrap_or_else(|e| panic!("{d}: {e:?}"));
    }
    for (path, title, artist, album, track, lyrics) in TRACKS {
        write(&mut vol, path, &mp3_with_tags(title, artist, album, *track));
        if let Some(l) = lyrics {
            let lrc = format!("{}.lrc", path.trim_end_matches(".mp3"));
            write(&mut vol, &lrc, l.as_bytes());
        }
    }
    write(
        &mut vol,
        "/Music/Podcasts/A very long conversation about the history of portable music players.mp3",
        &mp3_with_tags(
            "A very long conversation about the history of portable music players",
            "Radio Talk",
            "",
            7,
        ),
    );
    write(
        &mut vol,
        "/Music/Mix.m3u8",
        b"#EXTM3U\nSigur R\xc3\xb3s/Takk\xe2\x80\xa6/02 Hopp\xc3\xadpolla.mp3\n",
    );
    for i in 0..BIG {
        let name = format!("/Music/Big folder/{} {}.mp3", WORDS[i % 26], i / 26 + 1);
        write(&mut vol, &name, &[]);
    }
    write(&mut vol, "/Books/A Tale of Two Cities.txt", BOOK_EN.repeat(6).as_bytes());
    write(&mut vol, "/Books/Война и мир.txt", BOOK_RU.repeat(6).as_bytes());
    write(&mut vol, "/Books/枕草子.txt", BOOK_JA.repeat(6).as_bytes());
    write(&mut vol, "/Books/Lyrics.lrc", b"[00:01.00]la la la\n");
    write(&mut vol, "/Pictures/Iceland.jpg", &[0xFF, 0xD8, 0xFF, 0xE0]);
    let (w, h) = (240, 160);
    let png = orpheus_host::image::rgb565_png_bytes(&pictures::to_rgb565_le(&pictures::aurora(w, h)), w, h)
        .expect("png");
    write(&mut vol, "/Pictures/Aurora.png", &png);
    write(&mut vol, "/Pictures/Lake.bmp", &pictures::bmp(160, 96, &pictures::lake(160, 96)));
    write(&mut vol, "/Pictures/Sunset.qoi", &pictures::qoi(72, 120, &pictures::tree(72, 120)));
    write(&mut vol, "/Pictures/Москва.png", b"\x89PNG\r\n\x1a\n");
    write(&mut vol, "/Images/rescue.iso", &[0u8; 2048]);
    write(&mut vol, "/Images/boot.img", &[0u8; 512]);
    write(&mut vol, "/README.TXT", b"Orpheus simulated card.\nThis file has an 8.3 name only.\n");
    write(&mut vol, "/update.yfw", b"YFW1");
    write(&mut vol, "/otpauth.txt", b"otpauth://totp/Example:me?secret=JBSWY3DPEHPK3PXP\n");
    write(&mut vol, "/.orpheus/library.ydb", b"");
    write(&mut vol, "/.hidden-notes.txt", b"hidden");
    block_on(vol.flush()).expect("flush");
    vol.into_device().data
}

/// The card image (built on first use, shared). Building takes seconds
/// (5000 creates through orpheus-fs), so it is cached in
/// `target/orpheus/card-<hash of this source file>.img`.
pub fn image() -> &'static [u8] {
    static IMG: OnceLock<Vec<u8>> = OnceLock::new();
    IMG.get_or_init(|| {
        // The pictures on the card are made by `pictures.rs`: part of the key.
        let key = include_str!("card.rs")
            .bytes()
            .chain(include_str!("pictures.rs").bytes())
            .fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
        let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
            || std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("target"),
            std::path::PathBuf::from,
        );
        let path = target.join("orpheus").join(format!("card-{key:016x}.img"));
        if let Ok(b) = std::fs::read(&path)
            && b.len() == SECTORS as usize * 512
        {
            return b;
        }
        let b = build();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        if std::fs::write(&tmp, &b).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
        b
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_builds_and_lists() {
        let t = std::time::Instant::now();
        let img = image();
        eprintln!("card built in {:?}", t.elapsed());
        let mut cache: Vec<CacheSlot> = vec![CacheSlot::EMPTY; 8];
        let mut vol =
            block_on(Volume::mount(RamDisk::from_bytes(img.to_vec()), &mut cache, MountOptions::default()))
                .unwrap();
        let mut cur = block_on(vol.open_dir("/Music/Big folder")).unwrap();
        let mut e = orpheus_fs::DirEntry::new();
        let mut n = 0;
        while block_on(vol.read_dir(&mut cur, &mut e)).unwrap() {
            n += 1;
        }
        assert_eq!(n, BIG);
    }
}
