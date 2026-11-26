//! The simulated microSD card: a FAT16 image formatted here (deterministic,
//! no external tools) and filled through `orpheus-fs` itself, so browsing in
//! the simulator goes through the real file-system code. Built once per
//! process; every simulator mounts a copy-on-write overlay of it.
//!
//! Contents: music in Latin, Cyrillic and Japanese folders with ID3v2 tags
//! and `.lrc` lyrics, a 5000-file folder, text files, books for the Books
//! app (public-domain excerpts in Markdown, UTF-16 text with a Gutenberg
//! header, and a Windows-1251 FB2 with contents; an EPUB 3 of our own
//! text, named to sort last in Files), other file types the
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

/// Lewis Carroll, *Alice's Adventures in Wonderland* (1865, public
/// domain): the opening of the first two chapters, as Markdown.
const ALICE: &str = "---\ntitle: Alice's Adventures in Wonderland\nauthor: Lewis Carroll\n---\n\n# Alice's Adventures in Wonderland\n\n## Chapter I. Down the Rabbit-Hole\n\nAlice was beginning to get very tired of sitting by her sister on the bank, and of having nothing to do: once or twice she had peeped into the book her sister was reading, but it had no pictures or conversations in it, *\u{201c}and what is the use of a book,\u{201d}* thought Alice *\u{201c}without pictures or conversations?\u{201d}*\n\nSo she was considering in her own mind (as well as she could, for the hot day made her feel very sleepy and stupid), whether the pleasure of making a daisy-chain would be worth the trouble of getting up and picking the daisies, when suddenly a **White Rabbit** with pink eyes ran close by her.\n\nThere was nothing so *very* remarkable in that; nor did Alice think it so *very* much out of the way to hear the Rabbit say to itself, \u{201c}Oh dear! Oh dear! I shall be late!\u{201d}\n\n## Chapter II. The Pool of Tears\n\n\u{201c}Curiouser and curiouser!\u{201d} cried Alice (she was so much surprised, that for the moment she quite forgot how to speak good English); \u{201c}now I\u{2019}m opening out like the largest telescope that ever was! Good-bye, feet!\u{201d}\n\n> For when she looked down at her feet, they seemed to be almost out of sight, they were getting so far off.\n\n- Drink me\n- Eat me\n\n";

/// Herman Melville, *Moby-Dick* (1851, public domain): the opening, with a
/// Project Gutenberg style header (UTF-16 LE with a byte-order mark on
/// the card).
const MOBY: &str = "Title: Moby-Dick; or, The Whale\r\nAuthor: Herman Melville\r\n\r\nCHAPTER 1. Loomings.\r\n\r\nCall me Ishmael. Some years ago\u{2014}never mind how long precisely\u{2014}having\r\nlittle or no money in my purse, and nothing particular to interest me\r\non shore, I thought I would sail about a little and see the watery part\r\nof the world. It is a way I have of driving off the spleen and\r\nregulating the circulation.\r\n\r\nThere now is your insular city of the Manhattoes, belted round by\r\nwharves as Indian isles by coral reefs\u{2014}commerce surrounds it with\r\nher surf.\r\n\r\nCHAPTER 2. The Carpet-Bag.\r\n\r\nI stuffed a shirt or two into my old carpet-bag, tucked it under my\r\narm, and started for Cape Horn and the Pacific.\r\n";

/// Alexander Pushkin, *The Queen of Spades* (1834, public domain): the
/// epigraph and the opening of chapter I, as FB2 (Windows-1251 on the
/// card, as many Russian FB2 files are).
const QUEEN: &str = "<?xml version=\"1.0\" encoding=\"windows-1251\"?>\n<FictionBook xmlns=\"http://www.gribuser.ru/xml/fictionbook/2.0\"><description><title-info><genre>prose_classic</genre><author><first-name>Александр</first-name><last-name>Пушкин</last-name></author><book-title>Пиковая дама</book-title><lang>ru</lang></title-info></description><body><title><p>Пиковая дама</p></title><epigraph><p>Пиковая дама означает тайную недоброжелательность.</p><text-author>Новейшая гадательная книга</text-author></epigraph><section><title><p>I</p></title><p>Однажды играли в карты у конногвардейца Нарумова. Долгая зимняя ночь прошла незаметно; сели ужинать в пятом часу утра. Те, которые остались в выигрыше, ели с большим аппетитом; прочие, в рассеянности, сидели перед пустыми своими приборами.</p><p>— Что ты сделал, <emphasis>Сурин</emphasis>? — спросил хозяин.</p><p>— Проиграл, по обыкновению. Надобно признаться, что я несчастлив: играю мирандолем, никогда не горячусь, ничем меня с толку не собьёшь, а всё проигрываюсь!</p><empty-line/><p>— А Германн-то каков! — сказал один из гостей, указывая на молодого инженера.</p></section><section><title><p>II</p></title><p>Старая графиня *** сидела в своей уборной перед зеркалом. Три девушки окружали её.</p></section></body></FictionBook>";

/// `s` in Windows-1251 (Russian letters, `—`, `«`, `»`, `ё`; ASCII as is).
fn cp1251(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| match c {
            'А'..='я' => u8::try_from(u32::from(c) - 0x410 + 0xC0).unwrap_or(b'?'),
            'Ё' => 0xA8,
            'ё' => 0xB8,
            '—' => 0x97,
            '«' => 0xAB,
            '»' => 0xBB,
            c if c.is_ascii() => c as u8,
            _ => b'?',
        })
        .collect()
}

/// The EPUB 3 sample for Books (BOOK-2): our own short text, in a zip with
/// `mimetype` stored first and the rest deflated (miniz_oxide), a
/// navigation document with nested entries, three chapters (headings,
/// emphasis, a list, a quote, an anchor the contents point at), a style
/// sheet and an image the reader skips.
pub const EPUB_PATH: &str = "/Books/灯台守.epub";

const EPUB_OPF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="uid">urn:uuid:5e1a7c00-0000-4000-8000-000000000001</dc:identifier>
    <dc:title>The Lighthouse Keeper</dc:title>
    <dc:creator>Orpheus Project</dc:creator>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="css" href="css/book.css" media-type="text/css"/>
    <item id="c1" href="text/lamp.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="text/storm.xhtml" media-type="application/xhtml+xml"/>
    <item id="c3" href="text/morning.xhtml" media-type="application/xhtml+xml"/>
    <item id="pic" href="images/lens.png" media-type="image/png"/>
  </manifest>
  <spine><itemref idref="c1"/><itemref idref="c2"/><itemref idref="c3"/></spine>
</package>"#;

const EPUB_NAV: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Contents</title></head>
<body><nav epub:type="toc"><h1>Contents</h1><ol>
<li><a href="text/lamp.xhtml">I. The Lamp</a><ol><li><a href="text/lamp.xhtml#log">The Log</a></li></ol></li>
<li><a href="text/storm.xhtml">II. The Storm</a><ol><li><a href="text/storm.xhtml#glass">The glass falls</a></li></ol></li>
<li><a href="text/morning.xhtml">III. Morning</a></li>
</ol></nav></body></html>"#;

fn epub_page(title: &str, body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n<head><title>{title}</title><link rel=\"stylesheet\" href=\"../css/book.css\"/></head>\n<body><section epub:type=\"chapter\">\n{body}\n</section></body></html>"
    )
}

const EPUB_LAMP: &str = "<h1>I. The Lamp</h1>
<p>The keeper climbed the hundred and twelve steps every evening at dusk, counting them under his breath as his father had done before him. At the top the great lens waited in its brass cage, cold and patient, and he polished it with a soft cloth until the last of the daylight shone through it like water.</p>
<p><em>Light the lamp</em>, his father used to say, <em>and the sea will tell you the rest.</em></p>
<p>He trimmed the wick, filled the reservoir with oil and struck the match. The flame caught, steadied, and the lens took it up and threw it out across the bay in a long white arm that swept the dark once every twenty seconds.</p>
<p class=\"figure\"><img src=\"../images/lens.png\" alt=\"The lens\"/></p>
<h2 id=\"log\">The Log</h2>
<p>In the log he wrote the date, the wind, the state of the sea and the ships he saw:</p>
<ul><li>the <i>Margaret</i>, a schooner, bound north;</li><li>two fishing boats from the harbour;</li><li>a steamer, far out, showing no flag.</li></ul>
<blockquote><p>Wind north-west, fresh. Sea moderate. All well.</p></blockquote>";

const EPUB_STORM: &str = "<h1>II. The Storm</h1>
<p>The wind backed to the south in the night and began to rise. By midnight the whole tower hummed with it, a low note felt in the teeth more than heard, and the rain came against the gallery glass in handfuls.</p>
<p id=\"glass\"><strong>Then the glass fell.</strong> He tapped the barometer twice with his knuckle and watched the needle sink, slow and sure, past <i>Rain</i>, past <i>Stormy</i>, to a place on the dial he had never seen it go.</p>
<p>He did not sleep. Every hour he climbed the steps to wipe the salt from the windows and to see that the flame stood straight, and every hour the sea was higher on the rocks below.</p>
<hr/>
<p>Near four o&#8217;clock a light showed to the south&#8212;low, red, and much too close to the reef.</p>";

const EPUB_MORNING: &str = "<h1>III. Morning</h1>
<p>The ship was the <i>Margaret</i>, come back from the north with her mainmast gone. She had seen the light, her captain said afterwards, in the one moment the rain had lifted, and had put her helm over in time.</p>
<p>The keeper wrote it in the log in his small square hand, under the date and the wind, and then, because there was no column for it, in the margin: <em>All well.</em></p>";

/// A zip of `(name, data, deflate)` entries.
fn zip(parts: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut z = Vec::new();
    let mut cd = Vec::new();
    for &(name, data, deflate) in parts {
        let body = if deflate { miniz_oxide::deflate::compress_to_vec(data, 9) } else { data.to_vec() };
        let local = u32::try_from(z.len()).unwrap_or(0);
        let mut h = Vec::new();
        h.extend_from_slice(&20u16.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(&(if deflate { 8u16 } else { 0 }).to_le_bytes());
        h.extend_from_slice(&[0, 0, 0x21, 0x5B]);
        h.extend_from_slice(&orpheus_books::zip::crc32(0, data).to_le_bytes());
        h.extend_from_slice(&u32::try_from(body.len()).unwrap_or(0).to_le_bytes());
        h.extend_from_slice(&u32::try_from(data.len()).unwrap_or(0).to_le_bytes());
        h.extend_from_slice(&u16::try_from(name.len()).unwrap_or(0).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(b"PK\x03\x04");
        z.extend_from_slice(&h);
        z.extend_from_slice(name.as_bytes());
        z.extend_from_slice(&body);
        cd.extend_from_slice(b"PK\x01\x02\x14\x03");
        cd.extend_from_slice(&h);
        // Comment length, disk, internal and external attributes.
        cd.extend_from_slice(&[0; 10]);
        cd.extend_from_slice(&local.to_le_bytes());
        cd.extend_from_slice(name.as_bytes());
    }
    let at = u32::try_from(z.len()).unwrap_or(0);
    let n = u16::try_from(parts.len()).unwrap_or(0);
    z.extend_from_slice(&cd);
    z.extend_from_slice(b"PK\x05\x06\0\0\0\0");
    z.extend_from_slice(&n.to_le_bytes());
    z.extend_from_slice(&n.to_le_bytes());
    z.extend_from_slice(&u32::try_from(cd.len()).unwrap_or(0).to_le_bytes());
    z.extend_from_slice(&at.to_le_bytes());
    z.extend_from_slice(&0u16.to_le_bytes());
    z
}

/// The sample EPUB's bytes.
#[must_use]
pub fn epub() -> Vec<u8> {
    let container = "<?xml version=\"1.0\"?>\n<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\"><rootfiles><rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles></container>";
    let (lamp, storm, morning) = (
        epub_page("The Lamp", EPUB_LAMP),
        epub_page("The Storm", EPUB_STORM),
        epub_page("Morning", EPUB_MORNING),
    );
    zip(&[
        ("mimetype", b"application/epub+zip", false),
        ("META-INF/container.xml", container.as_bytes(), true),
        ("OEBPS/content.opf", EPUB_OPF.as_bytes(), true),
        ("OEBPS/nav.xhtml", EPUB_NAV.as_bytes(), true),
        ("OEBPS/css/book.css", b"p { text-indent: 1.2em; margin: 0 }\nh1 { text-align: center }", true),
        ("OEBPS/text/lamp.xhtml", lamp.as_bytes(), true),
        ("OEBPS/text/storm.xhtml", storm.as_bytes(), true),
        ("OEBPS/text/morning.xhtml", morning.as_bytes(), true),
        ("OEBPS/images/lens.png", b"\x89PNG\r\n\x1a\n(not drawn)", false),
    ])
}

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
    write(&mut vol, "/Books/Alice in Wonderland.md", ALICE.repeat(2).as_bytes());
    let mut moby = vec![0xFF, 0xFE];
    moby.extend(MOBY.repeat(3).encode_utf16().flat_map(u16::to_le_bytes));
    write(&mut vol, "/Books/Moby Dick.txt", &moby);
    write(&mut vol, "/Books/Пиковая дама.fb2", &cp1251(QUEEN));
    write(&mut vol, EPUB_PATH, &epub());
    write(&mut vol, "/Pictures/Iceland.jpg", &[0xFF, 0xD8, 0xFF, 0xE0]);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (w, h) = (240, 160);
        let png =
            orpheus_host::image::rgb565_png_bytes(&pictures::to_rgb565_le(&pictures::aurora(w, h)), w, h)
                .expect("png");
        write(&mut vol, "/Pictures/Aurora.png", &png);
    }
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
    test_images(&mut vol);
    block_on(vol.flush()).expect("flush");
    vol.into_device().data
}

/// A test image's bytes: a counter pattern, distinct per `seed`.
#[must_use]
pub fn image_bytes(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| (i / 512) as u8 ^ (i as u8).wrapping_mul(31) ^ seed).collect()
}

/// Size of `/.orpheus/images/test.img`.
pub const TEST_IMG: usize = 1024 * 1024;
/// Clusters of `/.orpheus/images/scattered.img`, each its own run: more
/// than the USB Drive's extent table holds (682).
pub const SCATTERED_RUNS: usize = 700;

/// USB Drive (USB-6) test images: `/.orpheus/images/test.img` (1 MiB,
/// contiguous) and `scattered.img`, written one cluster at a time in turn
/// with a filler file (deleted after) so that every cluster is a run of
/// its own.
fn test_images(vol: &mut Volume<'_, RamDisk>) {
    block_on(vol.create_dir("/.orpheus/images")).expect("images dir");
    let mut f = block_on(vol.open("/.orpheus/images/test.img", OpenMode::CreateNew)).expect("test.img");
    block_on(vol.write(&mut f, &image_bytes(TEST_IMG, 0x5A))).expect("write");
    block_on(vol.close(f)).expect("close");
    let cluster = 2048;
    let data = image_bytes(SCATTERED_RUNS * cluster, 0x33);
    let mut a = block_on(vol.open("/.orpheus/images/scattered.img", OpenMode::CreateNew)).expect("scattered");
    let mut b = block_on(vol.open("/.orpheus/filler.bin", OpenMode::CreateNew)).expect("filler");
    for piece in data.chunks(cluster) {
        block_on(vol.write(&mut a, piece)).expect("write");
        block_on(vol.flush_file(&mut a)).expect("flush");
        block_on(vol.write(&mut b, &[0u8; 2048])).expect("write");
        block_on(vol.flush_file(&mut b)).expect("flush");
    }
    block_on(vol.close(a)).expect("close");
    block_on(vol.close(b)).expect("close");
    // The gaps stay free; the image stays scattered.
    block_on(vol.remove("/.orpheus/filler.bin")).expect("remove filler");
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
