//! What the firmware's `job:index` piece costs on the card (AU-PERF,
//! 2026-11-25: on the player one unsplittable piece of 52–60 ms, the
//! first sorted listing of a folder while music plays, above every chore
//! budget): sector reads and directory steps of each of its parts on a
//! card shaped like the owner's (exFAT with 128 KiB clusters, and FAT32;
//! a root of hundreds of long-named folders with `/.orpheus` made after
//! them; 16 shards of tens of index files), with the path-walk cache as
//! the player has it then (`/.orpheus/index` warmed at mount and boosted,
//! the art and library folders in the other slots, the shard lost) and
//! the sector cache cold from the playback loop's reads.
//!
//! The 52–60 ms was the root search: every index (or library) build
//! removes its scratch files at its end, every removal emptied the path
//! walk cache, and the next lookup walked `/.orpheus/index/X/NAME` from
//! the root (102–334 sector reads here, against 5–14 with the cache).
//! Now a file's removal keeps the cache, and the lookup runs in stretches
//! (`CardIo::find_index`) that stop at the chore budget whatever the cache
//! holds.
//!
//! `measure_index_piece` prints the table (`--ignored --nocapture`); the
//! `regress_` test bounds the card operations of the lookup and of the
//! first listing; the model test runs the lookup in chore pieces
//! (`orpheus_audio::chore::Piece`) at the player's time per read.

use std::cell::Cell;
use std::path::PathBuf;
use std::process::Command;
use std::task::Poll;

use orpheus_audio::chore::{Costs, Piece, Spent};
use orpheus_fs::host::{RamDisk, block_on};
use orpheus_fs::{CacheSlot, DirCursor, DirEntry, MountOptions, OpenMode, PathWalk, Volume, WalkCache};
use orpheus_index::card::{CardIo, Handles, INDEX_DIR, Lookup, Mode, Place};
use orpheus_index::{Builder, FileName, Index, LetterTable, Locator, Progress, Row, SortBy, Spec};

type Vol<'c> = Volume<'c, RamDisk>;
type Io<'a, 'v, 'c> = CardIo<'a, 'v, 'c, RamDisk>;

const SPEC: Spec = Spec { sort: SortBy::Name, show_hidden: false, audio_only: false, images_only: false };
const ROW0: Row = Row {
    loc: Locator { dir: 0, index: 0 },
    size: 0,
    mtime: 0,
    letter: None,
    kind: orpheus_index::EntryKind::Other,
    hidden: false,
};
/// The firmware's stretch (`media::FIND_SETS`) and seed of the kind's
/// need (`audio::SEED_US`, `job:index`).
const FIND_SETS: u32 = 16;
const SEED_US: u32 = 15_000;

fn have(tool: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {tool}")).output().is_ok_and(|o| o.status.success())
}

/// A formatted image: exFAT with 128 KiB clusters (the owner's card), or
/// FAT32 with 512-byte clusters (one FAT sector per directory cluster
/// crossed: the slowest FAT layout).
fn image(exfat: bool) -> Option<RamDisk> {
    let tool = if exfat { "mkfs.exfat" } else { "mkfs.fat" };
    if !have(tool) {
        eprintln!("{tool} missing: skipped");
        return None;
    }
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("orpheus-index-piece");
    std::fs::create_dir_all(&dir).unwrap();
    let p =
        dir.join(format!("{}-{:?}.img", if exfat { "exfat" } else { "fat32" }, std::thread::current().id()));
    let f = std::fs::File::create(&p).unwrap();
    f.set_len(if exfat { 192 << 20 } else { 64 << 20 }).unwrap();
    drop(f);
    let mut c = Command::new(tool);
    if exfat {
        c.args(["-q", "-c", "131072"]);
    } else {
        c.args(["-F", "32", "-s", "1"]);
    }
    assert!(c.arg(&p).output().unwrap().status.success());
    let d = RamDisk::from_bytes(std::fs::read(&p).unwrap());
    let _ = std::fs::remove_file(&p);
    Some(d)
}

fn mount<'c>(disk: RamDisk, cache: &'c mut [CacheSlot], walks: &'c mut WalkCache) -> Vol<'c> {
    let opts = MountOptions { read_only: false, write_prefix: None, partition: None };
    let mut v = block_on(Volume::mount(disk, cache, opts)).expect("mount");
    v.set_walk_cache(walks);
    v
}

fn track(i: usize) -> String {
    format!("/Album/{i:03} - Track Title Number {i}.mp3")
}

/// The card: `roots` artist folders in the root, `/Album` (`tracks`
/// files), `/.orpheus` after them, `per_shard` other index files in each
/// shard (half before the album's index, half after), art and library
/// files. Returns the disk and the album's index.
fn card(exfat: bool, roots: usize, tracks: usize, per_shard: usize) -> Option<(RamDisk, Index)> {
    let disk = image(exfat)?;
    let mut cache = vec![CacheSlot::EMPTY; 8];
    let mut walks = WalkCache::EMPTY;
    let mut vol = mount(disk, &mut cache, &mut walks);
    block_on(async {
        for i in 0..roots {
            vol.create_dir(&format!("/{i:03} Some Artist Name With A Longer Title")).await.unwrap();
        }
        vol.create_dir("/Album").await.unwrap();
        for i in 0..tracks {
            let f = vol.open(&track(i), OpenMode::CreateNew).await.unwrap();
            vol.close(f).await.unwrap();
        }
        for d in
            ["/.orpheus", "/.orpheus/index", "/.orpheus/cache", "/.orpheus/cache/art", "/.orpheus/library"]
        {
            vol.create_dir(d).await.unwrap();
        }
        for s in 0..16u32 {
            vol.create_dir(&format!("{INDEX_DIR}/{s:x}")).await.unwrap();
        }
        other_index_files(&mut vol, 0..per_shard as u32 / 2).await;
        for p in ["/.orpheus/cache/art/A0000001", "/.orpheus/library/lib-a.ydb"] {
            let mut f = vol.open(p, OpenMode::CreateNew).await.unwrap();
            vol.write(&mut f, &[7u8; 2048]).await.unwrap();
            vol.close(f).await.unwrap();
        }
    });
    let ix = build(&mut vol);
    assert_eq!(ix.count() as usize, tracks);
    block_on(other_index_files(&mut vol, per_shard as u32 / 2..per_shard as u32));
    let (d, r) = block_on(vol.unmount());
    r.unwrap();
    Some((d, ix))
}

async fn other_index_files(vol: &mut Vol<'_>, ks: std::ops::Range<u32>) {
    for s in 0..16u32 {
        for k in ks.clone() {
            let name = FileName::index(0x1000 + k * 16 + s, SPEC);
            let f = vol.open(&format!("{INDEX_DIR}/{s:x}/{}", name.as_str()), OpenMode::CreateNew).await;
            vol.close(f.unwrap()).await.unwrap();
        }
    }
}

fn album(vol: &mut Vol<'_>) -> DirCursor {
    block_on(vol.open_dir("/Album")).unwrap()
}

/// The album's index built (or rebuilt in place) in one writing session:
/// it removes its scratch files at the end, as every build does.
fn build(vol: &mut Vol<'_>) -> Index {
    let start = album(vol);
    let mut scratch = vec![0u8; 8192];
    let mut h = Handles::new();
    let mut e = DirEntry::new();
    let mut cur = start;
    block_on(std::future::poll_fn(|cx| {
        let io = CardIo::new(vol, cx, &mut h, Mode::Write { alt: false });
        let mut b = Builder::new(&mut scratch, start.dir_cluster(), SPEC, 0).unwrap();
        let mut src = io.dir(start, &mut cur, &mut e);
        let ix = loop {
            if let Progress::Done(ix) = b.step(&mut io.store(), &mut src, 256).unwrap() {
                break ix;
            }
        };
        io.close_all().unwrap();
        Poll::Ready(ix)
    }))
}

/// Card operations: sector reads and directory slots stepped through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Ops {
    reads: u64,
    steps: u32,
}

impl std::fmt::Display for Ops {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:3} r {:5} s", self.reads, self.steps)
    }
}

fn ops(vol: &mut Vol<'_>) -> Ops {
    let steps = vol.dir_steps();
    Ops { reads: vol.device().reads, steps }
}

fn io_ops(io: &Io<'_, '_, '_>) -> Ops {
    io.with_vol(|v, _| ops(v))
}

fn since(a: Ops, b: Ops) -> Ops {
    Ops { reads: b.reads - a.reads, steps: b.steps - a.steps }
}

fn max(a: Ops, b: Ops) -> Ops {
    Ops { reads: a.reads.max(b.reads), steps: a.steps.max(b.steps) }
}

/// The player's time for card work as the model counts it (µs): 500 per
/// sector read (0.3–1 ms on the player), 3 per directory slot looked at
/// (the matcher and `advance`, with a code-line load now and then).
fn model_us(o: Ops) -> u32 {
    (o.reads * 500 + u64::from(o.steps) * 3) as u32
}

/// A firmware-style read-only session (`media::with_index`).
fn session<R>(
    vol: &mut Vol<'_>,
    f: FileName,
    at: &mut Option<Place>,
    g: impl FnOnce(&Io<'_, '_, '_>) -> R,
) -> R {
    let mut h = Handles::new();
    let mut g = Some(g);
    let kept = *at;
    let (r, p) = block_on(std::future::poll_fn(|cx| {
        let io = CardIo::new(vol, cx, &mut h, Mode::Read);
        if let Some(k) = kept {
            io.reuse(&f, k);
        }
        let r = (g.take().unwrap())(&io);
        let p = io.place(&f);
        io.close_all().unwrap();
        Poll::Ready((r, p))
    }));
    assert!(h.is_empty());
    *at = p;
    r
}

/// The player's caches at the time of `job:index`: `/.orpheus/index`
/// warmed at mount (`Volume::warm_walk`), then the art and library walks;
/// the sector cache cold.
fn as_on_the_player(vol: &mut Vol<'_>) {
    block_on(vol.warm_walk(INDEX_DIR)).unwrap();
    for p in ["/.orpheus/cache/art/A0000001", "/.orpheus/library/lib-a.ydb"] {
        let f = block_on(vol.open(p, OpenMode::Read)).unwrap();
        block_on(vol.close(f)).unwrap();
    }
    vol.invalidate_cached(0, u32::MAX);
}

/// The walk cache lost (a directory removed or renamed, a remount).
fn cache_lost(vol: &mut Vol<'_>) {
    vol.set_walk_cache(Box::leak(Box::new(WalkCache::EMPTY)));
    vol.invalidate_cached(0, u32::MAX);
}

/// The whole lookup with no budget (idle, paused): the outcome, its card
/// operations and the most between two asks of `left` (one stretch, or
/// the open with the header read).
fn lookup(vol: &mut Vol<'_>, dircl: u32, at: &mut Option<Place>) -> (Lookup, Ops, Ops) {
    let f = FileName::index(dircl, SPEC);
    let t0 = ops(vol);
    let mut gap = Ops::default();
    let r = session(vol, f, at, |io| {
        let last = Cell::new(io_ops(io));
        let gap_ = Cell::new(Ops::default());
        let mut w = PathWalk::START;
        let r = io
            .find_index(dircl, SPEC, &mut w, FIND_SETS, || {
                let now = io_ops(io);
                gap_.set(max(gap_.get(), since(last.get(), now)));
                last.set(now);
                true
            })
            .unwrap();
        gap = max(gap_.get(), since(last.get(), io_ops(io)));
        r
    });
    (r, since(t0, ops(vol)), gap)
}

/// One `job:index` piece as the firmware runs it (`media::Server::step`,
/// `Job::Find`): a session, `CardIo::find_index` asking
/// `chore::Piece::left` on a clock that advances by the model's time of
/// the card operations done, `budget_us`, the kind's need from `costs`
/// (and the piece measured into it, `audio::piece_end`).
fn index_piece(
    vol: &mut Vol<'_>,
    dircl: u32,
    w: &mut PathWalk,
    budget_us: u32,
    costs: &mut Costs<1>,
    at: &mut Option<Place>,
) -> (Lookup, Spent, Ops) {
    let f = FileName::index(dircl, SPEC);
    let t0 = ops(vol);
    let need = costs.need(0, SEED_US);
    let (r, s) = session(vol, f, at, |io| {
        let now = || model_us(since(t0, io_ops(io)));
        let p = Cell::new(Piece::new(0, budget_us, need));
        let r = io
            .find_index(dircl, SPEC, w, FIND_SETS, || {
                let mut q = p.get();
                let ok = q.left(now());
                p.set(q);
                ok
            })
            .unwrap();
        (r, p.get().end(now()))
    });
    costs.observe(0, s.unit, SEED_US);
    (r, s, since(t0, ops(vol)))
}

#[test]
#[ignore = "prints measurements"]
fn measure_index_piece() {
    println!("job:index (sector reads r, directory steps s); walk cache as on the player, sectors cold");
    for exfat in [true, false] {
        for (roots, per_shard) in [(300, 40), (1000, 40), (300, 100)] {
            let Some((disk, ix)) = card(exfat, roots, 300, per_shard) else { continue };
            let mut cache = vec![CacheSlot::EMPTY; 8];
            let mut walks = WalkCache::EMPTY;
            let mut vol = mount(disk, &mut cache, &mut walks);
            let dircl = album(&mut vol).dir_cluster();
            let f = ix.file();
            let shard = format!("{INDEX_DIR}/{:x}", dircl & 0xF);
            let path = format!("{shard}/{}", f.as_str());
            let fs = if exfat { "exFAT 128K" } else { "FAT32 512 " };
            println!("{fs} root {roots:4}, {per_shard:3} files/shard:");

            // The lookup as one piece (the firmware before), and in
            // stretches with no budget.
            as_on_the_player(&mut vol);
            let t = ops(&mut vol);
            let got =
                session(&mut vol, f, &mut None, |io| Index::open(&mut io.store(), dircl, SPEC).unwrap());
            assert_eq!(got, Some(ix));
            let whole = since(t, ops(&mut vol));
            as_on_the_player(&mut vol);
            let (r, all, gap) = lookup(&mut vol, dircl, &mut None);
            assert_eq!(r, Lookup::Found(ix));
            println!(
                "  cache as on the player: one piece {whole}; in stretches {all}, the most between asks {gap}"
            );
            as_on_the_player(&mut vol);
            let _ = build(&mut vol);
            vol.invalidate_cached(0, u32::MAX);
            let (_, after_build, _) = lookup(&mut vol, dircl, &mut None);
            println!("  after an index build (its scratch files removed): {after_build}");
            cache_lost(&mut vol);
            let (_, lost, gap) = lookup(&mut vol, dircl, &mut None);
            println!(
                "  cache lost: {lost} ({:.1} ms at 0.5 ms a read), the most between asks {gap}",
                f64::from(model_us(lost)) / 1000.0
            );

            // The parts.
            as_on_the_player(&mut vol);
            let t = ops(&mut vol);
            block_on(vol.open_dir(&shard)).unwrap();
            let shard_search = since(t, ops(&mut vol));
            vol.invalidate_cached(0, u32::MAX);
            let t = ops(&mut vol);
            let (mut file, _) = block_on(vol.open_located(&path)).unwrap();
            let file_search = since(t, ops(&mut vol));
            let t = ops(&mut vol);
            let mut hb = [0u8; 68];
            block_on(vol.read_exact_or_eof(&mut file, &mut hb)).unwrap();
            let header = since(t, ops(&mut vol));
            let t = ops(&mut vol);
            let mut rb = [0u8; 4 * 32];
            file.set_position(512);
            block_on(vol.read_exact_or_eof(&mut file, &mut rb)).unwrap();
            let rows_after = since(t, ops(&mut vol));
            block_on(vol.close(file)).unwrap();
            println!(
                "  parts: shard search {shard_search}; shard + file search {file_search}; header {header}; \
                 first rows after it {rows_after}"
            );

            // A folder with no index: the shard searched to its end.
            as_on_the_player(&mut vol);
            let (r, missing, _) = lookup(&mut vol, dircl ^ 0x100, &mut None);
            assert_eq!(r, Lookup::Missing);

            // The pieces after it: the focused row (a pass over the rows,
            // here 300), 8 rows with their entries, the letter table.
            let mut at = None;
            lookup(&mut vol, dircl, &mut at);
            vol.invalidate_cached(0, u32::MAX);
            let t = ops(&mut vol);
            session(&mut vol, f, &mut at, |io| {
                ix.position_of(&mut io.store(), Locator { dir: 1, index: 1 }).unwrap()
            });
            let focus = since(t, ops(&mut vol));
            vol.invalidate_cached(0, u32::MAX);
            let t = ops(&mut vol);
            first_rows(&mut vol, &ix, &mut at);
            let rows8 = since(t, ops(&mut vol));
            println!("  no index {missing}; focus over 300 rows {focus}; 8 rows + letters {rows8}");

            // In chore pieces of 20 ms at the model's time.
            for lost in [false, true] {
                as_on_the_player(&mut vol);
                if lost {
                    cache_lost(&mut vol);
                }
                let (n, took, unit) = in_pieces(&mut vol, dircl, &ix);
                println!(
                    "  pieces of 20 ms ({}): {n}, longest {:.1} ms, longest stretch {:.1} ms",
                    if lost { "cache lost" } else { "cache as on the player" },
                    f64::from(took) / 1000.0,
                    f64::from(unit) / 1000.0
                );
            }
        }
    }
}

/// The first rows of the window and the letter table, in one session
/// from the place (`job:rows`).
fn first_rows(vol: &mut Vol<'_>, ix: &Index, at: &mut Option<Place>) {
    let mut e = DirEntry::new();
    session(vol, ix.file(), at, |io| {
        let mut rows = [ROW0; 4];
        for k in 0..2 {
            assert_eq!(ix.rows(&mut io.store(), 4 * k, &mut rows).unwrap(), 4);
            for r in &rows {
                io.with_vol(|v, cx| orpheus_index::card::spin(cx, v.open_locator(r.loc, &mut e))).unwrap();
            }
        }
        let mut t = LetterTable::<80>::EMPTY;
        t.load(ix, &mut io.store()).unwrap();
    });
}

/// The lookup in `job:index` pieces of 20 ms, the sector cache cold
/// between them (the playback loop's reads): how many pieces, the longest
/// one and the longest stretch between two asks (µs). Every piece keeps
/// to its budget and the index is found.
fn in_pieces(vol: &mut Vol<'_>, dircl: u32, ix: &Index) -> (u32, u32, u32) {
    let mut costs = Costs::<1>::new();
    let mut w = PathWalk::START;
    let (mut n, mut took, mut unit) = (0u32, 0u32, 0u32);
    loop {
        let mut at = None;
        let (r, s, o) = index_piece(vol, dircl, &mut w, 20_000, &mut costs, &mut at);
        n += 1;
        took = took.max(s.took);
        unit = unit.max(s.unit);
        assert!(!s.over, "piece {n}: {s:?}, {o}");
        match r {
            Lookup::More => {}
            Lookup::Found(got) => {
                assert_eq!(got, *ix);
                assert!(at.is_some(), "the place is kept for the listing's pieces");
                return (n, took, unit);
            }
            Lookup::Missing => panic!("piece {n}: not found"),
        }
        assert!(n < 1000);
        vol.invalidate_cached(0, u32::MAX);
    }
}

/// The lookup of a listing's index on the owner-like card does no root
/// search after an index build (which removes its scratch files) and no
/// stretch of it reads more than a few sectors, and the first listing's
/// index part (lookup, first rows, letter table) stays a few dozen sector
/// reads. Before: the build emptied the walk cache, and the lookup alone
/// was 102–113 sector reads and 1,582–1,677 directory steps in one piece
/// (`job:index` 52–60 ms on the player).
#[test]
fn regress_index_lookup_card_operations() {
    for exfat in [true, false] {
        let Some((disk, ix)) = card(exfat, 300, 300, 40) else { continue };
        let mut cache = vec![CacheSlot::EMPTY; 8];
        let mut walks = WalkCache::EMPTY;
        let mut vol = mount(disk, &mut cache, &mut walks);
        let dircl = album(&mut vol).dir_cluster();
        let fs = if exfat { "exFAT" } else { "FAT32" };
        as_on_the_player(&mut vol);
        for round in 0..3 {
            // A build while idle (the folder changed), then a listing.
            let _ = build(&mut vol);
            vol.invalidate_cached(0, u32::MAX);
            let mut at = None;
            let (r, all, gap) = lookup(&mut vol, dircl, &mut at);
            assert_eq!(r, Lookup::Found(ix), "{fs} round {round}");
            assert!(all.reads <= 12 && all.steps <= 200, "{fs} round {round}: lookup {all}");
            // FAT32 with 512-byte clusters: a FAT sector per directory cluster.
            let most = if exfat { 4 } else { 6 };
            assert!(gap.reads <= most && gap.steps <= 60, "{fs} round {round}: one stretch {gap}");
            let t = ops(&mut vol);
            first_rows(&mut vol, &ix, &mut at);
            let rows = since(t, ops(&mut vol));
            assert!(all.reads + rows.reads <= 30, "{fs} round {round}: first listing {all} + {rows}");
        }
        // The walk cache lost: the root is searched, but in stretches of a
        // few sectors each.
        cache_lost(&mut vol);
        let (r, all, gap) = lookup(&mut vol, dircl, &mut None);
        assert_eq!(r, Lookup::Found(ix));
        assert!(all.reads > 50, "{fs}: {all}: the root searched");
        assert!(gap.reads <= 6 && gap.steps <= 100, "{fs}: one stretch {gap}");
    }
}

/// The host model of the chore rule: the lookup runs in `job:index`
/// pieces of a 20 ms budget at 0.5 ms a sector read (`model_us`), with the
/// kind's need learnt as the firmware learns it (`chore::Costs`, seed
/// 15 ms): every piece keeps to its budget, its longest stretch is a few
/// ms, and the index is found, with the walk cache as on the player and
/// with it lost (a root of 300 or 1,000 folders searched). As one piece
/// (the firmware before) the lost-cache lookup is 56–182 ms in the model:
/// the soak's 52–60 ms on a root of a few hundred folders.
#[test]
fn index_lookup_pieces_fit_a_20ms_budget() {
    for exfat in [true, false] {
        for roots in [300, 1000] {
            let Some((disk, ix)) = card(exfat, roots, 40, 40) else { continue };
            let mut cache = vec![CacheSlot::EMPTY; 8];
            let mut walks = WalkCache::EMPTY;
            let mut vol = mount(disk, &mut cache, &mut walks);
            let dircl = album(&mut vol).dir_cluster();
            for lost in [false, true] {
                let case = format!("{} root {roots} lost {lost}", if exfat { "exFAT" } else { "FAT32" });
                as_on_the_player(&mut vol);
                if lost {
                    cache_lost(&mut vol);
                    // The model sees the old single piece over any budget.
                    let (_, whole, _) = lookup(&mut vol, dircl, &mut None);
                    assert!(model_us(whole) > 50_000, "{case}: {whole}");
                    cache_lost(&mut vol);
                }
                let (n, took, unit) = in_pieces(&mut vol, dircl, &ix);
                assert!(took <= 20_000, "{case}: a piece of {took} µs");
                assert!(unit <= 4_000, "{case}: a stretch of {unit} µs");
                if !lost {
                    assert!(n <= 2, "{case}: {n} pieces");
                }
            }
        }
    }
}
