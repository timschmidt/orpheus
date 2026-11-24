//! A queue built from a directory of a real FAT image (mkfs.fat), its
//! locators re-opened without a path walk, and a resume record that names
//! the track.

use std::path::PathBuf;
use std::process::Command;

use yoton_fs::host::{FileDisk, block_on};
use yoton_fs::{CacheSlot, DirEntry, MountOptions, OpenMode, Volume};
use yoton_player::queue::queue_dir;
use yoton_player::{Actions, Command as Cmd, Config, Event, Key, Player, PlayerStore, TrackRef};

#[derive(Default)]
struct Mem(std::collections::BTreeMap<u8, Vec<u8>>);

impl PlayerStore for Mem {
    fn write(&mut self, key: Key, data: &[u8]) {
        self.0.insert(key as u8, data.to_vec());
    }
    fn read(&mut self, key: Key, buf: &mut [u8]) -> usize {
        match self.0.get(&(key as u8)) {
            Some(v) if v.len() == buf.len() => {
                buf.copy_from_slice(v);
                v.len()
            }
            _ => 0,
        }
    }
}

#[test]
fn directory_queue_and_locators() {
    let have =
        Command::new("sh").args(["-c", "command -v mkfs.fat"]).output().is_ok_and(|o| o.status.success());
    if !have {
        eprintln!("mkfs.fat missing: test not run");
        return;
    }
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("yoton-player");
    let _ = std::fs::create_dir_all(&dir);
    let img = dir.join("queue.img");
    let f = std::fs::File::create(&img).unwrap();
    f.set_len(16 << 20).unwrap();
    drop(f);
    assert!(
        Command::new("mkfs.fat")
            .args(["-F", "16", "-i", "1234ABCD"])
            .arg(&img)
            .output()
            .unwrap()
            .status
            .success()
    );

    let names =
        ["01 Пролог.flac", "02 Song.MP3", "cover.jpg", "03 Long name for a wave file.wav", "notes.txt"];
    let mut cache = vec![CacheSlot::EMPTY; 8];
    let dev = FileDisk::open(&img).unwrap();
    let mut vol = block_on(Volume::mount(dev, &mut cache, MountOptions::default())).unwrap();
    block_on(async {
        vol.create_dir("/music").await.unwrap();
        vol.create_dir("/music/sub.mp3").await.unwrap();
        for n in names {
            let mut f = vol.open(&format!("/music/{n}"), OpenMode::Create).await.unwrap();
            vol.write(&mut f, n.as_bytes()).await.unwrap();
            vol.close(f).await.unwrap();
        }
    });
    let mut q: Vec<TrackRef> = Vec::new();
    let n = block_on(queue_dir(&mut vol, "/music", |t| q.push(t))).unwrap();
    assert_eq!(n, 3);
    assert!(q.iter().all(|t| t.serial == vol.serial()));
    // Each locator re-opens the same entry, and the name hash checks it.
    let audio: Vec<&str> = names.iter().copied().filter(|n| yoton_player::queue::is_audio(n)).collect();
    for (t, name) in q.iter().zip(&audio) {
        let mut e = DirEntry::new();
        block_on(vol.open_locator(t.locator(), &mut e)).unwrap();
        assert_eq!(e.name(), *name);
        assert!(t.matches(e.name()));
        assert!(!t.matches("other.mp3"));
    }

    // Play item 1, pause, "lose power", resume: the record names the track.
    let mut st = Mem::default();
    let mut p = Player::new(Config::default(), &mut st);
    let mut out = Actions::default();
    p.handle(Event::Command(Cmd::PlayQueue { id: 1, len: n, start: 1 }), &mut st, &mut out);
    p.handle(Event::Opened { track: q[1], rate: 44100, frames: Some(1_000_000) }, &mut st, &mut out);
    p.handle(Event::Position { frame: 4242 }, &mut st, &mut out);
    p.handle(Event::Command(Cmd::Pause), &mut st, &mut out);
    let p = Player::new(Config::default(), &mut st);
    let r = p.saved().unwrap();
    assert_eq!((r.track, r.frame, r.item), (Some(q[1]), 4242, 1));
    let mut e = DirEntry::new();
    block_on(vol.open_locator(r.track.unwrap().locator(), &mut e)).unwrap();
    assert_eq!(e.name(), "02 Song.MP3");
}
