//! The `card` adapters over a real FAT / exFAT image (formatted with the
//! Linux tools, held in a RAM disk): build in slots that each open and
//! close their files (as the firmware does between requests), suspend and
//! resume in the middle, query, rebuild after a change, and leave a clean
//! file system; the volume is mounted with the player's `/.orpheus` write prefix.

use std::path::PathBuf;
use std::process::Command;
use std::task::Poll;

use orpheus_fs::host::{RamDisk, block_on};
use orpheus_fs::{CacheSlot, DirCursor, DirEntry, MountOptions, OpenMode, Volume};
use orpheus_index::card::{CardIo, Handles, Mode, is_dot, src_entry};
use orpheus_index::{
    Builder, FileName, Index, Locator, Progress, Signature, SortBy, Spec, Suspended, letter_of, natural_cmp,
};

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
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("orpheus-index-card");
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
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("orpheus-index-card");
    let p = dir.join(if exfat { "check-exfat.img" } else { "check-fat32.img" });
    std::fs::write(&p, &disk.data).unwrap();
    let tool = if exfat { "fsck.exfat" } else { "fsck.fat" };
    if !have(tool) {
        return;
    }
    let o = Command::new(tool).arg("-n").arg(&p).output().unwrap();
    assert!(
        o.status.success(),
        "{tool}: {}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}

fn names() -> Vec<(String, bool)> {
    let mut v = Vec::new();
    for i in 1..=150 {
        v.push((format!("Track {i}.mp3"), false));
    }
    for (i, a) in
        ["zebra", "Apple", "banana", "Ölberg", "émile", "_x", "9 lives", "10 lives"].iter().enumerate()
    {
        v.push((format!("{a} {i}.flac"), false));
    }
    for d in ["Live", "albums", "Bonus 2", "Bonus 10"] {
        v.push((d.to_string(), true));
    }
    v
}

fn mount(disk: RamDisk, cache: &mut [CacheSlot], prefix: bool) -> Vol<'_> {
    let opts =
        MountOptions { read_only: false, write_prefix: prefix.then_some("/.orpheus"), partition: None };
    block_on(Volume::mount(disk, cache, opts)).expect("mount")
}

/// Create `/music` with the test names (without the write prefix).
fn populate(disk: RamDisk, list: &[(String, bool)]) -> RamDisk {
    let mut cache = vec![CacheSlot::EMPTY; 8];
    let mut vol = mount(disk, &mut cache, false);
    block_on(async {
        vol.create_dir("/music").await.unwrap();
        for (n, d) in list {
            let p = format!("/music/{n}");
            if *d {
                vol.create_dir(&p).await.unwrap();
            } else {
                let mut f = vol.open(&p, OpenMode::CreateNew).await.unwrap();
                vol.write(&mut f, n.as_bytes()).await.unwrap();
                vol.close(f).await.unwrap();
            }
        }
    });
    let (d, r) = block_on(vol.unmount());
    r.unwrap();
    d
}

fn music(vol: &mut Vol<'_>) -> DirCursor {
    block_on(vol.open_dir("/music")).unwrap()
}

/// One firmware-style slot: resume (or start), at most `steps` steps of
/// `budget`, then suspend; files closed at the end.
fn slot(
    vol: &mut Vol<'_>,
    st: Option<Suspended>,
    start: DirCursor,
    cur: &mut DirCursor,
    spec: Spec,
    alt: bool,
    steps: usize,
) -> Result<Index, Box<Suspended>> {
    let mut scratch = vec![0u8; 4096];
    let mut h = Handles::new();
    let mut e = DirEntry::new();
    let r = block_on(std::future::poll_fn(|cx| {
        let io = CardIo::new(vol, cx, &mut h, Mode::Write { alt });
        let mut b = match st {
            Some(s) => s.resume(&mut scratch).unwrap(),
            None => Builder::new(&mut scratch, start.dir_cluster(), spec, 0).unwrap(),
        };
        let mut src = io.dir(start, cur, &mut e);
        let mut out = None;
        for _ in 0..steps {
            if let Progress::Done(ix) = b.step(&mut io.store(), &mut src, 64).unwrap() {
                out = Some(Ok(ix));
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

fn query<R>(vol: &mut Vol<'_>, f: impl FnOnce(&CardIo<'_, '_, '_, RamDisk>) -> R) -> R {
    let mut h = Handles::new();
    let mut f = Some(f);
    block_on(std::future::poll_fn(|cx| {
        let io = CardIo::new(vol, cx, &mut h, Mode::Read);
        let r = (f.take().unwrap())(&io);
        io.close_all().unwrap();
        Poll::Ready(r)
    }))
}

fn name_of(vol: &mut Vol<'_>, l: Locator) -> String {
    let mut e = DirEntry::new();
    block_on(vol.open_locator(l, &mut e)).unwrap();
    e.name().to_string()
}

fn signature(vol: &mut Vol<'_>, start: DirCursor) -> Signature {
    let mut sig = Signature::default();
    let mut cur = start;
    let mut e = DirEntry::new();
    while block_on(vol.read_dir(&mut cur, &mut e)).unwrap() {
        if !is_dot(&e) {
            sig.add(&src_entry(&e), e.name());
        }
    }
    sig
}

fn listing(vol: &mut Vol<'_>, ix: &Index) -> Vec<String> {
    let all = query(vol, |io| {
        let mut out = vec![
            orpheus_index::Row {
                loc: Locator { dir: 0, index: 0 },
                size: 0,
                mtime: 0,
                letter: None,
                kind: orpheus_index::EntryKind::Other,
                hidden: false,
            };
            ix.count() as usize
        ];
        // Windows of 32, as the firmware reads them.
        let mut i = 0;
        while i < out.len() {
            let end = (i + 32).min(out.len());
            let n = ix.rows(&mut io.store(), i as u32, &mut out[i..end]).unwrap();
            assert!(n > 0);
            i += n;
        }
        out
    });
    all.iter().map(|r| name_of(vol, r.loc)).collect()
}

fn expected(list: &[(String, bool)]) -> Vec<String> {
    let mut v: Vec<&(String, bool)> = list.iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| natural_cmp(&a.0, &b.0)));
    v.into_iter().map(|(n, _)| n.clone()).collect()
}

fn scenario(exfat: bool) {
    let Some(disk) = image(exfat) else { return };
    let mut list = names();
    let disk = populate(disk, &list);
    let mut cache = vec![CacheSlot::EMPTY; 8];
    let mut vol = mount(disk, &mut cache, true);
    let start = music(&mut vol);
    let dir = start.dir_cluster();
    let spec = Spec { sort: SortBy::Name, show_hidden: false, audio_only: false, images_only: false };

    // Nothing yet: a read-only session finds no index and creates nothing.
    assert_eq!(query(&mut vol, |io| Index::open(&mut io.store(), dir, spec).unwrap()), None);
    assert!(block_on(vol.open_dir("/.orpheus")).is_err());

    // Build in slots of two steps (each suspends and closes its files),
    // with a second build (other scratch pair) in between.
    let mut cur = start;
    let mut st = None;
    let mut slots = 0;
    let ix = loop {
        slots += 1;
        match slot(&mut vol, st.take(), start, &mut cur, spec, false, 2) {
            Ok(ix) => break ix,
            Err(s) => st = Some(*s),
        }
        if slots == 2 {
            let root = vol.root();
            let mut rc = root;
            let date = Spec { sort: SortBy::Date, ..spec };
            let mut s2 = None;
            let r = loop {
                match slot(&mut vol, s2.take(), root, &mut rc, date, true, 100) {
                    Ok(ix) => break ix,
                    Err(s) => s2 = Some(*s),
                }
            };
            assert_eq!(r.count(), 1, "the root lists /music only (.orpheus is hidden)");
        }
        assert!(slots < 200);
    };
    assert!(slots > 2, "the build spanned several slots");
    assert_eq!(ix.count() as usize, list.len());
    assert_eq!(ix.dirs(), 4);
    assert_eq!(listing(&mut vol, &ix), expected(&list));
    assert!(ix.is_current(&signature(&mut vol, start)));

    // Reopened read-only: same index; letters agree with the rows.
    let again = query(&mut vol, |io| Index::open(&mut io.store(), dir, spec).unwrap()).expect("index");
    assert_eq!(again, ix);
    let names = listing(&mut vol, &ix);
    let runs = query(&mut vol, |io| {
        let mut runs = vec![(0u32, None); ix.letter_runs() as usize];
        assert_eq!(ix.read_letter_runs(&mut io.store(), 0, &mut runs).unwrap(), runs.len());
        runs
    });
    for (i, n) in names.iter().enumerate() {
        let r = runs.iter().rposition(|&(first, _)| first as usize <= i).unwrap();
        assert_eq!(runs[r].1, letter_of(n), "row {i} {n}");
    }
    let jump = query(&mut vol, |io| ix.seek_letter(&mut io.store(), 0, true).unwrap());
    assert_eq!(jump.map(|j| j.1), Some('B'), "Live, albums, Bonus… → B");

    // Find by probe (names read through the volume).
    let target = names.iter().position(|n| n == "Track 42.mp3").unwrap() as u32;
    let mut e = DirEntry::new();
    block_on(vol.stat("/music/Track 42.mp3", &mut e)).unwrap();
    let s = src_entry(&e);
    let found = query(&mut vol, |io| {
        let mut ne = DirEntry::new();
        let mut buf = vec![0u8; orpheus_index::MAX_NAME];
        let probe = orpheus_index::Probe {
            name: "Track 42.mp3",
            loc: s.loc,
            is_dir: s.is_dir,
            hidden: s.hidden,
            size: s.size,
            mtime: s.mtime,
        };
        let r = ix.find(&mut io.store(), &mut io.names(&mut ne), &probe, &mut buf).unwrap();
        let p = ix.position_of(&mut io.store(), s.loc).unwrap();
        (r, p)
    });
    assert_eq!(found, (Ok(target), Some(target)));

    // Files: the index in its shard, no scratch files left.
    let shard = format!("{:x}", dir & 0xF);
    let path = format!("/.orpheus/index/{shard}/{}", FileName::index(dir, spec).as_str());
    assert!(block_on(vol.exists(&path)).unwrap(), "{path}");
    for t in ["sort-a.tmp", "sort-b.tmp", "sort-c.tmp", "sort-d.tmp"] {
        assert!(!block_on(vol.exists(&format!("/.orpheus/index/{t}"))).unwrap(), "{t} left behind");
    }

    // A change makes it stale; a rebuild (in one slot) is current again.
    let (d, r) = block_on(vol.unmount());
    r.unwrap();
    let disk = {
        let mut cache = vec![CacheSlot::EMPTY; 8];
        let mut v = mount(d, &mut cache, false);
        block_on(async {
            let f = v.open("/music/Track 151.mp3", OpenMode::CreateNew).await.unwrap();
            v.close(f).await.unwrap();
        });
        let (d, r) = block_on(v.unmount());
        r.unwrap();
        d
    };
    list.push(("Track 151.mp3".into(), false));
    let mut vol = mount(disk, &mut cache, true);
    let start = music(&mut vol);
    assert!(!ix.is_current(&signature(&mut vol, start)));
    let mut cur = start;
    let ix2 = slot(&mut vol, None, start, &mut cur, spec, false, 1000).expect("done");
    assert!(ix2.is_current(&signature(&mut vol, start)));
    assert_eq!(listing(&mut vol, &ix2), expected(&list));

    let (d, r) = block_on(vol.unmount());
    r.unwrap();
    fsck(&d, exfat);
}

#[test]
fn fat32_build_query_rebuild() {
    scenario(false);
}

#[test]
fn exfat_build_query_rebuild() {
    scenario(true);
}
