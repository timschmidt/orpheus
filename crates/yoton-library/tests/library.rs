//! Build + query against a mock card, checked against an in-memory model.

mod common;

use std::collections::{BTreeMap, HashSet};

use common::*;
use yoton_library::{Config, Item, ItemKind, Library, List, MIN_SCRATCH, TrackRef};

const SERIAL: u32 = 0xC0FFEE;

type K = (u32, u32, u32, u32);
type CKey = (bool, Vec<u8>);

fn tk(t: &TrackRef) -> K {
    (t.serial, t.dir, t.index, t.name_hash)
}

fn cfg() -> Config {
    Config { serial: SERIAL, ..Config::default() }
}

/// Everything a user can see, as text (for comparing two libraries).
fn dump(l: &Library, s: &mut MemStore, with_recent: bool) -> Vec<String> {
    let mut out = Vec::new();
    let row = |l: &Library, s: &mut MemStore, i: &Item| -> String {
        format!(
            "{:?} {} / {} n={} c={} y={} t={} d={} ms={}",
            i.kind,
            text(l, s, i.name),
            text(l, s, i.sub),
            i.tracks,
            i.children,
            i.year,
            i.track,
            i.disc,
            i.duration_ms
        )
    };
    let refs = |l: &Library, s: &mut MemStore, list: List| -> Vec<String> {
        let mut v = Vec::new();
        l.queue(s, list, 0, u32::MAX, |r| {
            v.push(format!("{r:?}"));
            true
        })
        .unwrap();
        v
    };
    let mut lists = vec![List::Artists, List::Albums, List::Songs, List::Genres];
    if with_recent {
        lists.push(List::Recent);
    }
    while let Some(list) = lists.pop() {
        out.push(format!("== {list:?}"));
        for i in all_rows(l, s, list) {
            out.push(row(l, s, &i));
            if let Some(sub) = list.open(&i) {
                lists.push(sub);
            }
        }
        if list.is_tracks() {
            out.extend(refs(l, s, list));
        }
    }
    out
}

fn check_against_model(l: &Library, s: &mut MemStore, card: &Card, articles: bool) {
    let m = model(card, SERIAL, 16);
    assert_eq!(l.tracks() as usize, m.len());
    let spell_aa =
        spellings(m.iter().flat_map(|t| [(true, t.aa.clone()), (true, t.artist.clone())]), articles);
    let spell_album = spellings(m.iter().map(|t| (true, t.album.clone())), articles);
    let spell_title = spellings(m.iter().map(|t| (false, t.title.clone())), articles);
    let spell_genre = spellings(m.iter().map(|t| (false, t.genre.clone())), articles);
    let sp = |map: &std::collections::HashMap<(bool, Vec<u8>), String>, s: &str, aa: bool| {
        map[&ckey(s, aa, articles)].clone()
    };

    // Track order.
    let mut sorted = m.clone();
    sorted.sort_by(|a, b| cmp_track(a, b, articles));
    let pos_of: std::collections::HashMap<K, usize> =
        sorted.iter().enumerate().map(|(i, t)| (tk(&t.r), i)).collect();
    let mut all = Vec::new();
    for a in all_rows(l, s, List::Artists) {
        l.queue(s, List::ArtistTracks(a.id), 0, u32::MAX, |r| {
            all.push(r);
            true
        })
        .unwrap();
    }
    let want: Vec<TrackRef> = sorted.iter().map(|t| t.r).collect();
    if all != want {
        let i = all.iter().zip(&want).position(|(a, b)| a != b).unwrap_or(all.len().min(want.len()));
        let by_ref: std::collections::HashMap<K, &MTrack> = m.iter().map(|t| (tk(&t.r), t)).collect();
        for j in i.saturating_sub(2)..(i + 3).min(all.len()) {
            let got = by_ref.get(&tk(&all[j]));
            let tr = l.track(s, j as u32).unwrap();
            eprintln!(
                "{j}: lib {:?} [{} | {} | {} | d{} t{}] model-want {:?}",
                got.map(|t| (&t.aa, &t.album, t.disc, t.track, &t.title)),
                text(l, s, tr.album_artist),
                text(l, s, tr.album),
                text(l, s, tr.title),
                tr.disc,
                tr.track,
                (&sorted[j].aa, &sorted[j].album, sorted[j].disc, sorted[j].track, &sorted[j].title)
            );
        }
        panic!("track order differs at {i} (lib {} model {})", all.len(), want.len());
    }
    for (i, t) in sorted.iter().enumerate() {
        let tr = l.track(s, i as u32).unwrap();
        assert_eq!(tr.r, t.r);
        assert_eq!(text(l, s, tr.title), sp(&spell_title, &t.title, false));
        assert_eq!(text(l, s, tr.artist), sp(&spell_aa, &t.artist, true));
        assert_eq!(text(l, s, tr.album), sp(&spell_album, &t.album, true));
        assert_eq!(text(l, s, tr.album_artist), sp(&spell_aa, &t.aa, true));
        assert_eq!(text(l, s, tr.genre), sp(&spell_genre, &t.genre, false));
        assert_eq!((tr.year, tr.track, tr.disc), (t.year, t.track, t.disc));
        assert_eq!(l.find(s, t.r.locator()).unwrap(), Some(i as u32));
    }

    // Artists → albums (year order) → tracks.
    let mut artists: BTreeMap<(bool, Vec<u8>), Vec<usize>> = BTreeMap::new();
    for (i, t) in sorted.iter().enumerate() {
        artists.entry(ckey(&t.aa, true, articles)).or_default().push(i);
    }
    let rows = all_rows(l, s, List::Artists);
    assert_eq!(rows.len(), artists.len());
    for (row, (k, idx)) in rows.iter().zip(&artists) {
        assert_eq!(text(l, s, row.name), spell_aa[k]);
        assert_eq!(row.tracks as usize, idx.len());
        let mut albums: BTreeMap<(bool, Vec<u8>), Vec<usize>> = BTreeMap::new();
        for &i in idx {
            albums.entry(ckey(&sorted[i].album, true, articles)).or_default().push(i);
        }
        let year =
            |v: &Vec<usize>| v.iter().map(|&i| sorted[i].year).filter(|&y| y != 0).min().unwrap_or(u16::MAX);
        let mut want: Vec<_> = albums.iter().collect();
        want.sort_by(|a, b| year(a.1).cmp(&year(b.1)).then(a.0.cmp(b.0)));
        let arows = all_rows(l, s, List::ArtistAlbums(row.id));
        assert_eq!(arows.len(), want.len());
        assert_eq!(row.children as usize, want.len());
        for (ar, (ak, ai)) in arows.iter().zip(want) {
            assert_eq!(text(l, s, ar.name), spell_album[ak]);
            assert_eq!(ar.tracks as usize, ai.len());
            assert_eq!(if ar.year == 0 { u16::MAX } else { ar.year }, year(ai));
            let mut got = Vec::new();
            l.queue(s, List::AlbumTracks(ar.id), 0, u32::MAX, |r| {
                got.push(pos_of[&tk(&r)]);
                true
            })
            .unwrap();
            assert_eq!(&got, ai);
        }
    }

    // Albums by name.
    let mut groups: BTreeMap<(CKey, CKey), Vec<usize>> = BTreeMap::new();
    for (i, t) in sorted.iter().enumerate() {
        groups.entry((ckey(&t.album, true, articles), ckey(&t.aa, true, articles))).or_default().push(i);
    }
    let rows = all_rows(l, s, List::Albums);
    assert_eq!(rows.len(), groups.len());
    assert_eq!(l.albums() as usize, groups.len());
    let mut prev: Option<(String, u32)> = None;
    for row in &rows {
        let name = text(l, s, row.name);
        let ft = l.rows_first_track(s, row);
        let first = pos_of[&tk(&l.track(s, ft).unwrap().r)];
        let t = &sorted[first];
        assert_eq!(name, sp(&spell_album, &t.album, true));
        let key = (ckey(&t.album, true, articles), ckey(&t.aa, true, articles));
        assert_eq!(groups[&key].len(), row.tracks as usize);
        if let Some(p) = &prev {
            assert!(ckey(&p.0, true, articles) <= ckey(&name, true, articles));
        }
        prev = Some((name, row.id));
    }

    // Songs by title.
    let mut songs: Vec<usize> = (0..sorted.len()).collect();
    songs.sort_by(|&a, &b| {
        ckey(&sorted[a].title, false, articles)
            .cmp(&ckey(&sorted[b].title, false, articles))
            .then_with(|| ckey(&sorted[a].aa, true, articles).cmp(&ckey(&sorted[b].aa, true, articles)))
            .then_with(|| ckey(&sorted[a].album, true, articles).cmp(&ckey(&sorted[b].album, true, articles)))
            .then(sorted[a].disc.cmp(&sorted[b].disc))
            .then(sorted[a].track.cmp(&sorted[b].track))
            .then(a.cmp(&b))
    });
    let mut got = Vec::new();
    l.queue(s, List::Songs, 0, u32::MAX, |r| {
        got.push(pos_of[&tk(&r)]);
        true
    })
    .unwrap();
    assert_eq!(got, songs, "songs order");

    // Genres → artists → albums → tracks.
    let mut genres: BTreeMap<(bool, Vec<u8>), Vec<usize>> = BTreeMap::new();
    for (i, t) in sorted.iter().enumerate() {
        genres.entry(ckey(&t.genre, false, articles)).or_default().push(i);
    }
    let rows = all_rows(l, s, List::Genres);
    assert_eq!(rows.len(), genres.len());
    for (row, (k, idx)) in rows.iter().zip(&genres) {
        assert_eq!(text(l, s, row.name), spell_genre[k]);
        let mut got = Vec::new();
        l.queue(s, List::GenreTracks(row.id), 0, u32::MAX, |r| {
            got.push(pos_of[&tk(&r)]);
            true
        })
        .unwrap();
        assert_eq!(&got, idx);
        let mut seen = 0;
        for ga in all_rows(l, s, List::GenreArtists(row.id)) {
            assert_eq!(ga.kind, ItemKind::GenreArtist);
            let mut n = 0;
            for gal in all_rows(l, s, List::GenreArtistAlbums(ga.id)) {
                let mut v = Vec::new();
                l.queue(s, List::GenreAlbumTracks(gal.id), 0, u32::MAX, |r| {
                    v.push(pos_of[&tk(&r)]);
                    true
                })
                .unwrap();
                assert_eq!(v.len() as u32, gal.tracks);
                for &i in &v {
                    assert_eq!(text(l, s, gal.name), sp(&spell_album, &sorted[i].album, true));
                    assert_eq!(text(l, s, ga.name), sp(&spell_aa, &sorted[i].aa, true));
                    assert_eq!(ckey(&sorted[i].genre, false, articles), *k);
                }
                n += v.len();
            }
            assert_eq!(n as u32, ga.tracks);
            seen += n;
        }
        assert_eq!(seen, idx.len());
    }

    // Recently added (a first build: by mtime).
    let mtime: std::collections::HashMap<K, u32> = card
        .dirs
        .iter()
        .flat_map(|d| {
            d.entries.iter().map(move |e| {
                (
                    tk(&TrackRef::new(SERIAL, yoton_fs::Locator { dir: d.id, index: e.index }, &e.name)),
                    e.mtime,
                )
            })
        })
        .collect();
    let mut got = Vec::new();
    l.queue(s, List::Recent, 0, u32::MAX, |r| {
        got.push(r);
        true
    })
    .unwrap();
    for w in got.windows(2) {
        let (a, b) = (mtime[&tk(&w[0])], mtime[&tk(&w[1])]);
        assert!(a > b || (a == b && pos_of[&tk(&w[0])] < pos_of[&tk(&w[1])]));
    }

    // Letters and letter jump.
    for list in [List::Artists, List::Albums, List::Genres, List::Songs] {
        let rows = all_rows(l, s, list);
        let kind_aa = matches!(list, List::Artists | List::Albums);
        let letters: Vec<Option<char>> = rows
            .iter()
            .map(|r| {
                let k = if kind_aa { yoton_library::Kind::Artist } else { yoton_library::Kind::Title };
                yoton_apps::browse::letter_of(yoton_library::sort_name(&text(l, s, r.name), k, articles))
            })
            .collect();
        for (i, want) in letters.iter().enumerate() {
            assert_eq!(l.letter(s, list, i as u32).unwrap(), *want, "{list:?} row {i}");
            for fwd in [true, false] {
                assert_eq!(
                    l.seek_letter(s, list, i as u32, fwd).unwrap(),
                    ref_seek_letter(&letters, i as u32, fwd),
                    "{list:?} from {i} {fwd}"
                );
            }
        }
        // position_of agrees with the rows.
        for (i, r) in rows.iter().enumerate().step_by(7) {
            assert_eq!(l.position_of(s, list, r.id).unwrap(), Some(i as u32));
        }
    }
    assert!(l.verify(s, &mut [0u8; 700]).unwrap());
}

trait FirstTrack {
    fn rows_first_track(&self, s: &mut MemStore, row: &Item) -> u32;
}

impl FirstTrack for Library {
    fn rows_first_track(&self, s: &mut MemStore, row: &Item) -> u32 {
        self.row(s, List::AlbumTracks(row.id), 0).unwrap().unwrap().id
    }
}

fn only_slot_files(s: &MemStore) {
    let mut names: Vec<&String> = s.files.keys().collect();
    names.sort();
    assert_eq!(names.len(), 1, "leftover files: {names:?}");
    assert!(names[0].starts_with("lib-"));
}

#[test]
fn full_build_matches_model() {
    let mut r = Rng(0x1234_5678);
    let mut card = random_card(&mut r, 3000);
    let mut store = MemStore::new();
    let l = build(&mut store, &mut card, cfg());
    check_against_model(&l, &mut store, &card, false);
    only_slot_files(&store);
    assert_eq!(Library::open(&mut store).unwrap(), Some(l));
}

#[test]
fn ignore_articles() {
    let mut r = Rng(77);
    let mut card = random_card(&mut r, 600);
    let mut store = MemStore::new();
    let l = build(&mut store, &mut card, Config { ignore_articles: true, ..cfg() });
    assert!(l.ignores_articles());
    check_against_model(&l, &mut store, &card, true);
}

#[test]
fn empty_and_tiny_cards() {
    let mut store = MemStore::new();
    let mut card = Card::new();
    let l = build(&mut store, &mut card, cfg());
    assert_eq!(l.tracks(), 0);
    for list in [List::Artists, List::Albums, List::Songs, List::Genres, List::Recent] {
        assert_eq!(l.len(&mut store, list).unwrap(), 0);
        assert_eq!(l.seek_letter(&mut store, list, 0, true).unwrap(), None);
    }
    assert_eq!(l.sample(&mut store, 1, 10, |_| true).unwrap(), 0);
    for n in [1, 2, 5] {
        let mut r = Rng(n);
        let mut card = random_card(&mut r, n as usize);
        let mut store = MemStore::new();
        let l = build(&mut store, &mut card, cfg());
        check_against_model(&l, &mut store, &card, false);
    }
}

#[test]
fn suspend_resume_and_scratch_sizes_give_the_same_library() {
    let mut r = Rng(99);
    let card = random_card(&mut r, 1500);
    let mut base_store = MemStore::new();
    let base = build(&mut base_store, &mut card.clone(), cfg());
    let want = dump(&base, &mut base_store, true);
    for (scratch, budget, pct) in
        [(MIN_SCRATCH, 64, 0), (MIN_SCRATCH, 5, 30), (16 * 1024, 200, 10), (32 * 1024, 1, 50)]
    {
        let mut store = MemStore::new();
        let mut c = card.clone();
        let (l, st, _) =
            build_with(&mut store, &mut c, cfg(), scratch, budget, pct, &mut Rng(scratch as u64)).unwrap();
        if pct > 0 {
            assert!(st.suspends > 10);
        }
        assert_eq!(dump(&l, &mut store, true), want, "scratch {scratch} budget {budget} suspend {pct}%");
        only_slot_files(&store);
    }
}

/// Add, remove, retag and move things on the card.
fn mutate(card: &mut Card, r: &mut Rng) -> (u32, u32) {
    let mut changed = 0;
    let mut added = 0;
    // Retag some files (new tags, new mtime).
    for d in 0..card.dirs.len() {
        for e in &mut card.dirs[d].entries {
            if let Some((c, t, _)) = e.file.as_mut()
                && *c != Carrier::Ogg
                && *c != Carrier::None
                && e.name.ends_with("mp3") | e.name.ends_with("flac")
                && r.chance(3)
            {
                if *c == Carrier::Id3v1 {
                    t.title = "Retagged".into();
                } else {
                    t.title = format!("{} (remaster)", t.title);
                    t.genre = "Remastered".into();
                }
                e.mtime += 1;
                changed += 1;
            }
        }
    }
    // Remove files from some folders, and one whole folder.
    for d in 1..card.dirs.len() {
        if r.chance(5) {
            let dir = &mut card.dirs[d];
            if let Some(p) = dir.entries.iter().position(|e| e.file.is_some()) {
                dir.entries.remove(p);
            }
        }
    }
    if let Some(p) = card.dirs[0].entries.iter().position(|e| e.name == "Music") {
        let m = card.dirs[0].entries[p].child.unwrap();
        if card.dirs[m].entries.len() > 3 {
            card.dirs[m].entries.remove(1);
        }
    }
    // New files in existing folders and a new album.
    for d in 1..card.dirs.len() {
        if r.chance(4) {
            let t = Tags { title: word_title(r), ..empty_tags() };
            let name = format!("new {}.mp3", r.below(1_000_000));
            card.add_file(d, &name, Carrier::Id3v2 { art: 0, art_first: false }, t, 3_000_000, 200_000);
            added += 1;
        }
    }
    let nd = card.path_dir(&["Music", "Newcomer", "First Album"]);
    for i in 1..=9u16 {
        let t = Tags {
            title: format!("New Song {i}"),
            artist: "Newcomer".into(),
            album: "First Album".into(),
            album_artist: String::new(),
            genre: "Indie".into(),
            year: 2026,
            track: i,
            disc: 0,
        };
        card.add_file(nd, &format!("{i:02}.flac"), Carrier::Flac { art: 0 }, t, 20_000_000, 200_000);
        added += 1;
    }
    (changed, added)
}

#[test]
fn incremental_update_equals_a_fresh_build() {
    let mut r = Rng(4242);
    let mut card = random_card(&mut r, 2000);
    let mut store = MemStore::new();
    let first = build(&mut store, &mut card, cfg());
    assert_eq!(first.generation(), 1);
    let first_dump = dump(&first, &mut store, true);

    // Nothing changed: no tag is read.
    let (again, _, st) =
        build_with(&mut store, &mut card, Config { now: 300_000, ..cfg() }, 12 * 1024, 64, 0, &mut r)
            .unwrap();
    assert_eq!(st.read, 0, "{st:?}");
    assert_eq!(st.reused, again.tracks());
    assert_eq!(dump(&again, &mut store, true), first_dump);
    // The previous slot is gone: the old handle no longer reads.
    assert!(first.rows(&mut store, List::Artists, 0, &mut [Item::default()]).is_err());
    assert_eq!(again.generation(), 2);
    only_slot_files(&store);

    let card_before = card.clone();
    let (changed, added) = mutate(&mut card, &mut r);
    let (inc, _, st) =
        build_with(&mut store, &mut card, Config { now: 400_000, ..cfg() }, 12 * 1024, 64, 20, &mut r)
            .unwrap();
    assert!(st.read as usize <= (changed + added) as usize, "{st:?} changed {changed} added {added}");
    assert!(st.reused > inc.tracks() / 2);
    let mut fresh_store = MemStore::new();
    let fresh = build(&mut fresh_store, &mut card.clone(), cfg());
    assert_eq!(dump(&inc, &mut store, false), dump(&fresh, &mut fresh_store, false));
    // New files come first in Recently added (first seen now), retagged
    // ones keep their first-seen time.
    let before: HashSet<K> = model(&card_before, SERIAL, 16).iter().map(|t| tk(&t.r)).collect();
    let new: HashSet<K> =
        model(&card, SERIAL, 16).iter().map(|t| tk(&t.r)).filter(|r| !before.contains(r)).collect();
    assert!(new.len() >= 9);
    let mut first_rows = HashSet::new();
    inc.queue(&mut store, List::Recent, 0, new.len() as u32, |t| {
        first_rows.insert(tk(&t));
        true
    })
    .unwrap();
    assert_eq!(first_rows, new);
    for t in model(&card, SERIAL, 16) {
        let id = inc.find(&mut store, t.r.locator()).unwrap().unwrap();
        let a = inc.track(&mut store, id).unwrap().added;
        assert_eq!(a == 400_000, new.contains(&tk(&t.r)));
    }
    check_recent_after_update(&inc, &mut store);
    only_slot_files(&store);
}

fn check_recent_after_update(l: &Library, s: &mut MemStore) {
    let mut prev = u32::MAX;
    let mut ids = Vec::new();
    l.queue(s, List::Recent, 0, u32::MAX, |t| {
        ids.push(t);
        true
    })
    .unwrap();
    for t in ids {
        let id = l.find(s, t.locator()).unwrap().unwrap();
        let a = l.track(s, id).unwrap().added;
        assert!(a <= prev);
        prev = a;
    }
}

#[test]
fn power_cut_at_many_points() {
    let mut r = Rng(31337);
    let mut card = random_card(&mut r, 300);
    // A cut during the very first build: nothing opens.
    let mut total = MemStore::new();
    build(&mut total, &mut card.clone(), cfg());
    let first_bytes = total.write_bytes;
    for k in 1..12u64 {
        let mut store = MemStore::new();
        store.cut_after = Some(first_bytes * k / 12);
        assert!(build_with(&mut store, &mut card.clone(), cfg(), 12 * 1024, 32, 0, &mut r).is_err());
        store.revive();
        assert_eq!(Library::open(&mut store).unwrap(), None);
        let l = build(&mut store, &mut card.clone(), cfg());
        check_against_model(&l, &mut store, &card, false);
    }
    // Cuts during an update: the old library stays.
    let mut base = MemStore::new();
    let old = build(&mut base, &mut card, cfg());
    let old_dump = dump(&old, &mut base, true);
    mutate(&mut card, &mut r);
    let mut probe = base.clone();
    probe.reset_counters();
    build(&mut probe, &mut card.clone(), cfg());
    let upd_bytes = probe.write_bytes;
    let mut fresh = MemStore::new();
    let want = dump(&build(&mut fresh, &mut card.clone(), cfg()), &mut fresh, false);
    for k in 0..40u64 {
        let mut store = base.clone();
        store.cut_after = Some(upd_bytes * k / 40 + r.below(512));
        let res = build_with(&mut store, &mut card.clone(), cfg(), 12 * 1024, 32, 10, &mut r);
        store.revive();
        let l = Library::open(&mut store).unwrap().expect("a library opens");
        if res.is_err() {
            assert_eq!(l.generation(), old.generation(), "cut {k}");
            assert_eq!(dump(&l, &mut store, true), old_dump);
            assert!(l.verify(&mut store, &mut [0u8; 512]).unwrap());
        }
        let l = build(&mut store, &mut card.clone(), cfg());
        assert_eq!(dump(&l, &mut store, false), want, "after cut {k}");
        only_slot_files(&store);
    }
}

#[test]
fn queue_windows_and_sample() {
    let mut r = Rng(5);
    let mut card = random_card(&mut r, 1000);
    let mut store = MemStore::new();
    let l = build(&mut store, &mut card, cfg());
    let n = l.tracks();
    // Queues: windows and early stop.
    let mut all = Vec::new();
    l.queue(&mut store, List::Songs, 0, u32::MAX, |t| {
        all.push(t);
        true
    })
    .unwrap();
    assert_eq!(all.len() as u32, n);
    let mut part = Vec::new();
    assert_eq!(
        l.queue(&mut store, List::Songs, 100, 50, |t| {
            part.push(t);
            true
        })
        .unwrap(),
        50
    );
    assert_eq!(part, all[100..150]);
    let mut k = 0;
    assert_eq!(
        l.queue(&mut store, List::Songs, 0, 500, |_| {
            k += 1;
            k < 7
        })
        .unwrap(),
        7
    );
    // Samples: distinct, valid, about uniform.
    let valid: HashSet<K> = all.iter().map(tk).collect();
    let mut hits = std::collections::HashMap::new();
    let rounds = 400;
    for seed in 0..rounds {
        let mut v = Vec::new();
        assert_eq!(
            l.sample(&mut store, seed, 100, |t| {
                v.push(t);
                true
            })
            .unwrap(),
            100
        );
        let set: HashSet<K> = v.iter().map(tk).collect();
        assert_eq!(set.len(), 100);
        for t in v {
            assert!(valid.contains(&tk(&t)));
            *hits.entry(tk(&t)).or_insert(0u32) += 1;
        }
    }
    // Expected hits per track: 400 × 100 / n ≈ 40; every track drawn,
    // none wildly over.
    let expect = f64::from(rounds as u32 * 100) / f64::from(n);
    assert_eq!(hits.len() as u32, n);
    let chi2: f64 = hits.values().map(|&h| (f64::from(h) - expect).powi(2) / expect).sum();
    assert!(chi2 < 1.3 * f64::from(n), "chi² {chi2} for {n} tracks");
    assert_eq!(l.sample(&mut store, 9, 5000, |_| true).unwrap(), n);
}

#[test]
fn figures() {
    // Struct sizes (RAM), on-card size and I/O per 1000 tracks, speed.
    eprintln!(
        "sizes: Suspended {} B, Builder {} B, Library {} B, Item {} B, Track {} B",
        size_of::<yoton_library::Suspended>(),
        size_of::<yoton_library::Builder<'static>>(),
        size_of::<Library>(),
        size_of::<Item>(),
        size_of::<yoton_library::Track>()
    );
    assert!(size_of::<yoton_library::Suspended>() <= 1280);
    assert!(size_of::<Library>() <= 256);
    for n in [1000usize, 5000] {
        let mut r = Rng(n as u64);
        let mut card = random_card(&mut r, n);
        let mut store = MemStore::new();
        let t0 = std::time::Instant::now();
        let (l, _, st) = build_with(&mut store, &mut card, cfg(), 12 * 1024, 256, 0, &mut r).unwrap();
        let dt = t0.elapsed();
        let size = l.file_len();
        eprintln!(
            "{n} files: {} tracks, {} artists, {} albums, {} genres; library {} B ({} B/1000 tracks), peak on card {} B; \
             host {:.0} ms ({:.0} tracks/s)",
            l.tracks(),
            l.artists(),
            l.albums(),
            l.genres(),
            size,
            size * 1000 / u64::from(l.tracks()),
            store.peak_total,
            dt.as_secs_f64() * 1000.0,
            f64::from(l.tracks()) / dt.as_secs_f64()
        );
        eprintln!(
            "  tag reads: {} files read, {} calls, {} B ({:.0} B/file, {:.2} calls/file), {} cut short; card file opens {}; \
             directory entries {}",
            st.read,
            st.tag_reads,
            st.tag_bytes,
            st.tag_bytes as f64 / f64::from(st.read.max(1)),
            f64::from(st.tag_reads) / f64::from(st.read.max(1)),
            st.truncated,
            card.file_opens,
            card.dir_entries
        );
        eprintln!(
            "  library I/O: {} reads / {} B read, {} writes / {} B written ({:.0} B written per track)",
            store.reads,
            store.read_bytes,
            store.writes,
            store.write_bytes,
            store.write_bytes as f64 / f64::from(l.tracks())
        );
        // An update with nothing changed.
        store.reset_counters();
        card.file_opens = 0;
        card.dir_entries = 0;
        let t0 = std::time::Instant::now();
        let (l, _, st) = build_with(&mut store, &mut card, cfg(), 12 * 1024, 256, 0, &mut r).unwrap();
        eprintln!(
            "  unchanged update: {:.0} ms host, {} tag reads, {} directory entries, {} B read, {} B written",
            t0.elapsed().as_secs_f64() * 1000.0,
            st.read,
            card.dir_entries,
            store.read_bytes,
            store.write_bytes
        );
        // Query costs: a window of 8 rows of each list.
        for list in [List::Artists, List::Albums, List::Songs, List::Genres, List::Recent] {
            store.reset_counters();
            let mut w = [Item::default(); 8];
            let mid = l.len(&mut store, list).unwrap() / 2;
            l.rows(&mut store, list, mid, &mut w).unwrap();
            let mut b = [0u8; 128];
            for i in &w {
                let _ = l.text(&mut store, i.name, &mut b).unwrap();
            }
            eprintln!("  window of 8 {list:?} with names: {} reads, {} B", store.reads, store.read_bytes);
        }
    }
}
