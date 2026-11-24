//! The `card` adapter over a real FAT32 / exFAT image (formatted with the
//! Linux tools, held in a RAM disk), driven as the firmware drives it:
//! sessions of a few steps that each open and close their files and are
//! suspended in between, the volume mounted with the player's `/.orpheus`
//! write prefix. The library must list exactly what a build of the same
//! files through the mock card lists; an update with nothing changed reads
//! no tags; queries, `find` and queues work on the result; the file system
//! stays clean.

mod common;

use std::path::PathBuf;
use std::process::Command;
use std::task::Poll;

use common::*;
use orpheus_fs::host::{RamDisk, block_on};
use orpheus_fs::{CacheSlot, DirEntry, MountOptions, OpenMode, Volume};
use orpheus_index::card::{CardIo, Handles, Mode};
use orpheus_library::card::{CardTags, TagState};
use orpheus_library::{Builder, Config, LIBRARY_DIR, Library, List, Progress, ScanStats, Suspended};

type Vol<'c> = Volume<'c, RamDisk>;

fn have(tool: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {tool}")).output().is_ok_and(|o| o.status.success())
}

fn image(exfat: bool) -> Option<RamDisk> {
    let tool = if exfat { "mkfs.exfat" } else { "mkfs.fat" };
    if !have(tool) {
        eprintln!("{tool} missing: skipped");
        return None;
    }
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("orpheus-library-card");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(if exfat { "exfat.img" } else { "fat32.img" });
    let f = std::fs::File::create(&p).unwrap();
    f.set_len(64 << 20).unwrap();
    drop(f);
    let mut c = Command::new(tool);
    if exfat {
        c.args(["-q", "-c", "4096"]);
    } else {
        c.args(["-F", "32", "-s", "1"]);
    }
    assert!(c.arg(&p).output().unwrap().status.success());
    Some(RamDisk::from_bytes(std::fs::read(&p).unwrap()))
}

fn fsck(disk: &RamDisk, exfat: bool) {
    let tool = if exfat { "fsck.exfat" } else { "fsck.fat" };
    if !have(tool) {
        return;
    }
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("orpheus-library-card");
    let p = dir.join(if exfat { "check-exfat.img" } else { "check-fat32.img" });
    std::fs::write(&p, &disk.data).unwrap();
    let o = Command::new(tool).arg("-n").arg(&p).output().unwrap();
    assert!(
        o.status.success(),
        "{tool}: {}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}

fn mount(disk: RamDisk, cache: &mut [CacheSlot], prefix: bool) -> Vol<'_> {
    let opts =
        MountOptions { read_only: false, write_prefix: prefix.then_some("/.orpheus"), partition: None };
    block_on(Volume::mount(disk, cache, opts)).expect("mount")
}

const FILE_SIZE: u64 = 24 * 1024;

/// A mock card small enough for the image: no pictures in the tags, small
/// files, names the FAT accepts.
fn small_card(tracks: usize) -> Card {
    let mut c = random_card(&mut Rng(7), tracks);
    for d in &mut c.dirs {
        // FAT names are unique without regard to case.
        let mut seen = std::collections::HashSet::new();
        for e in &mut d.entries {
            e.name = e.name.replace(['\\', ':', '*', '?', '"', '<', '>', '|'], "_");
            let mut k = 1;
            while !seen.insert(e.name.to_lowercase()) {
                k += 1;
                e.name = match e.name.rsplit_once('.') {
                    Some((s, x)) if e.child.is_none() => format!("{s} ~{k}.{x}"),
                    _ => format!("{} ~{k}", e.name),
                };
            }
            if let Some((carrier, _, size)) = e.file.as_mut() {
                *size = FILE_SIZE;
                *carrier = match *carrier {
                    Carrier::Id3v2 { .. } => Carrier::Id3v2 { art: 0, art_first: false },
                    Carrier::Flac { .. } => Carrier::Flac { art: 0 },
                    c => c,
                };
            }
        }
    }
    c
}

/// Write the mock card's folders and files to the image.
fn populate(disk: RamDisk, card: &Card) -> RamDisk {
    let mut cache = vec![CacheSlot::EMPTY; 8];
    let mut vol = mount(disk, &mut cache, false);
    let mut q = vec![(0usize, String::new())];
    block_on(async {
        while let Some((d, path)) = q.pop() {
            for e in &card.dirs[d].entries {
                let p = format!("{path}/{}", e.name);
                if let Some(ch) = e.child {
                    vol.create_dir(&p).await.unwrap();
                    q.push((ch, p));
                } else if let Some((carrier, tags, size)) = &e.file {
                    let vf = make_file(*carrier, tags, *size);
                    let mut bytes = vec![0u8; vf.size as usize];
                    for (at, seg) in &vf.segs {
                        let at = *at as usize;
                        let n = seg.len().min(bytes.len().saturating_sub(at));
                        bytes[at..at + n].copy_from_slice(&seg[..n]);
                    }
                    let mut f = vol.open(&p, OpenMode::CreateNew).await.unwrap();
                    vol.write(&mut f, &bytes).await.unwrap();
                    vol.close(f).await.unwrap();
                }
            }
        }
    });
    let (d, r) = block_on(vol.unmount());
    r.unwrap();
    d
}

/// One firmware-style session: resume (or start) on fresh scratch, at most
/// `steps` steps, then suspend; every file closed at the end.
fn session(
    vol: &mut Vol<'_>,
    st: Option<Suspended>,
    cfg: Config,
    steps: usize,
) -> Result<(Library, ScanStats), Box<Suspended>> {
    let mut scratch = vec![0x5Au8; 16 * 1024];
    let mut h = Handles::new();
    let mut e = DirEntry::new();
    let mut tags = TagState::new();
    let r = block_on(std::future::poll_fn(|cx| {
        let io = CardIo::new_in(vol, cx, &mut h, Mode::Write { alt: false }, LIBRARY_DIR);
        let mut b = match st {
            Some(s) => s.resume(&mut scratch).unwrap(),
            None => Builder::new(&mut scratch, cfg).unwrap(),
        };
        let mut out = None;
        for _ in 0..steps {
            let mut src = CardTags::new(&io, &mut tags, &mut e);
            if let Progress::Done(l) = b.step(&mut io.store(), &mut src, 32).unwrap() {
                out = Some(Ok((l, b.stats())));
                break;
            }
        }
        let out = out.unwrap_or_else(|| Err(Box::new(b.suspend(&mut io.store()).unwrap())));
        io.close_all().unwrap();
        Poll::Ready(out)
    }));
    assert!(h.is_empty());
    r
}

/// The firmware's way: the state stays where it is (`Suspended::new`,
/// `step_in`, `pause_in`); the scratch is scribbled over between sessions.
fn build_in_place(vol: &mut Vol<'_>, cfg: Config) -> (Library, ScanStats, usize) {
    let mut st = Suspended::new(20 * 1024, cfg).unwrap();
    let mut scratch = vec![0u8; 20 * 1024];
    let mut sessions = 0;
    loop {
        sessions += 1;
        assert!(sessions < 100_000, "no progress");
        scratch.fill(sessions as u8);
        let mut h = Handles::new();
        let mut e = DirEntry::new();
        let mut tags = TagState::new();
        let done = block_on(std::future::poll_fn(|cx| {
            let io = CardIo::new_in(vol, cx, &mut h, Mode::Write { alt: false }, LIBRARY_DIR);
            let mut out = None;
            for _ in 0..1 + sessions % 5 {
                let mut src = CardTags::new(&io, &mut tags, &mut e);
                if let Progress::Done(l) = st.step_in(&mut scratch, &mut io.store(), &mut src, 32).unwrap() {
                    out = Some(l);
                    break;
                }
            }
            if out.is_none() {
                st.pause_in(&mut scratch, &mut io.store()).unwrap();
            }
            io.close_all().unwrap();
            Poll::Ready(out)
        }));
        assert!(h.is_empty());
        if let Some(l) = done {
            return (l, st.stats(), sessions);
        }
    }
}

fn build(vol: &mut Vol<'_>, cfg: Config) -> (Library, ScanStats, usize) {
    let mut st = None;
    let mut sessions = 0;
    loop {
        sessions += 1;
        assert!(sessions < 100_000, "no progress");
        match session(vol, st.take(), cfg, 1 + sessions % 7) {
            Ok((l, s)) => return (l, s, sessions),
            Err(s) => st = Some(*s),
        }
    }
}

/// Queries in one read-only session.
fn query<R>(vol: &mut Vol<'_>, f: impl FnOnce(&CardIo<'_, '_, '_, RamDisk>) -> R) -> R {
    let mut h = Handles::new();
    let mut f = Some(f);
    block_on(std::future::poll_fn(|cx| {
        let io = CardIo::new_in(vol, cx, &mut h, Mode::Read, LIBRARY_DIR);
        let r = (f.take().unwrap())(&io);
        io.close_all().unwrap();
        Poll::Ready(r)
    }))
}

/// Names of every row of the user-visible lists (no locators: the mock's
/// and the image's differ).
fn names_of<S: orpheus_index::Storage>(l: &Library, s: &mut S) -> Vec<String>
where
    S::Error: core::fmt::Debug,
{
    let mut out = Vec::new();
    for list in [List::Artists, List::Albums, List::Songs, List::Genres] {
        let n = l.len(s, list).unwrap();
        let mut rows = [orpheus_library::Item::default(); 8];
        let mut i = 0;
        while i < n {
            let k = l.rows(s, list, i, &mut rows).unwrap();
            for it in &rows[..k] {
                let mut b = [0u8; 128];
                let name = l.text(s, it.name, &mut b).unwrap().to_string();
                out.push(format!("{list:?} {name} {}", it.tracks));
            }
            i += k as u32;
        }
    }
    out
}

fn scenario(exfat: bool) {
    let Some(disk) = image(exfat) else { return };
    let card = small_card(90);
    let disk = populate(disk, &card);
    let mut cache = vec![CacheSlot::EMPTY; 8];
    let mut vol = mount(disk, &mut cache, true);
    let cfg = Config { serial: vol.serial(), ..Config::default() };

    // First build, in many small sessions.
    let (lib, stats, sessions) = build(&mut vol, cfg);
    assert!(sessions > 10, "{sessions} sessions");
    assert_eq!(stats.reused, 0);
    assert!(stats.read > 0);
    let opened = query(&mut vol, |io| Library::open(&mut io.store()).unwrap()).expect("opens");
    assert_eq!(opened, lib);

    // The same files through the mock card list the same names.
    let mut mem = MemStore::new();
    let mut mock = card.clone();
    let reference = common::build(&mut mem, &mut mock, Config { serial: cfg.serial, ..Config::default() });
    assert_eq!(lib.tracks(), reference.tracks());
    let want = names_of(&reference, &mut mem);
    let got = query(&mut vol, |io| names_of(&lib, &mut io.store()));
    assert_eq!(got, want);

    // `find` answers for a file on the card, and its album queue holds it.
    let (loc, album) = query(&mut vol, |io| {
        let t = lib.track(&mut io.store(), 3).unwrap();
        let loc = t.r.locator();
        assert_eq!(lib.find(&mut io.store(), loc).unwrap(), Some(3));
        (loc, t.album_id)
    });
    let mut q = Vec::new();
    query(&mut vol, |io| {
        lib.queue(&mut io.store(), List::AlbumTracks(album), 0, 256, |t| {
            q.push(t.locator());
            true
        })
        .unwrap()
    });
    assert!(q.contains(&loc));
    let mut e = DirEntry::new();
    for l in &q {
        block_on(vol.open_locator(*l, &mut e)).unwrap();
        assert!(!e.is_dir());
    }

    // Nothing changed: the update (driven in place) reads no tags and
    // gives the same lists.
    let (lib2, stats2, _) = build_in_place(&mut vol, cfg);
    assert_eq!(stats2.read, 0, "{stats2:?}");
    assert_eq!(lib2.generation(), lib.generation() + 1);
    let again = query(&mut vol, |io| names_of(&lib2, &mut io.store()));
    assert_eq!(again, want);

    // Only the two slots' current one and nothing temporary is left.
    let left: Vec<String> = block_on(async {
        let mut cur = vol.open_dir(LIBRARY_DIR).await.unwrap();
        let mut v = Vec::new();
        while vol.read_dir(&mut cur, &mut e).await.unwrap() {
            if !matches!(e.name(), "." | "..") {
                v.push(e.name().to_string());
            }
        }
        v
    });
    assert!(left.iter().all(|n| n.ends_with(".ydb") || n == "fake.mp3"), "{left:?}");

    block_on(vol.flush()).unwrap();
    let (disk, r) = block_on(vol.unmount());
    r.unwrap();
    fsck(&disk, exfat);
}

#[test]
fn fat32_build_query_update() {
    scenario(false);
}

#[test]
fn exfat_build_query_update() {
    scenario(true);
}
