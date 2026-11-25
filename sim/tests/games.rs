//! M14 (GAME-0/1/3): the Games app over the simulator's games back end
//! (`sim/src/games.rs`): saved games and scores on the card, pause and
//! interruptions, the Menu key in Blocks, and the differential tests of
//! every game setting (07 §5.12; `tests/settings/effects.toml`).

use orpheus_apps::games::{self as app, GamesService};
use orpheus_games::blocks::Blocks;
use orpheus_games::scores::Scores;
use orpheus_games::tiles::{Phase, Tiles};
use orpheus_games::{Session, id};
use orpheus_sim::fallback::HexFont;
use orpheus_sim::{AppSim, Sim, apps::HostServices, games::read};

fn run(s: &mut AppSim, script: &str) {
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
}

fn sim(script: &str) -> AppSim {
    let mut s = AppSim::new();
    run(&mut s, script);
    s
}

fn blocks(s: &AppSim) -> &Blocks {
    match &s.sys.games().session {
        Session::Blocks(b) => b,
        other => panic!("not Blocks: {other:?}"),
    }
}

fn tiles(s: &AppSim) -> &Tiles {
    match &s.sys.games().session {
        Session::Tiles(t) => t,
        other => panic!("not 4096: {other:?}"),
    }
}

fn file(s: &mut AppSim, name: &str) -> Option<Vec<u8>> {
    read(&mut s.sys.media, &format!("{}/{name}", app::DIR))
}

const LIST: &str = "set:games=on set:gameseed=7 prev*2 ok wait:100 expect-screen:Games";
const BLOCKS: &str = "set:gameseed=7 goto:Blocks wait:100 expect-screen:Blocks";
const TILES: &str = "set:gameseed=7 goto:Game4096 wait:100 expect-screen:Game4096";

/// Home › Games › 4096: a game played, paused (saved to the card), quit
/// and continued, also after a restart of the player (the card's save
/// file is read again).
#[test]
fn tiles_saved_quit_continued_and_restored() {
    let mut s = sim(&format!("{LIST} down ok wait:100 expect-screen:Game4096 right down left"));
    let moves = tiles(&s).moves();
    assert!(moves > 0);
    assert!(file(&mut s, "4096.sav").is_none(), "nothing written while playing");
    run(&mut s, "back expect-screen:GamePause");
    let saved = file(&mut s, "4096.sav").expect("saved on pause");
    assert_eq!(Session::decode(&saved, Some(id::TILES)), Some(s.sys.games().session.clone()));
    // Quit: back to the list, the game offered again.
    run(&mut s, "up ok wait:100 expect-screen:Games");
    assert!(s.sys.games().can_continue(id::TILES));
    let board = tiles(&s).cells().to_vec();
    // Restart: new services over the same card.
    let media = std::mem::replace(&mut s.sys.media, orpheus_sim::media::Media::new());
    let mut r = Sim::with_sys(HostServices::with_media(media), Box::new(HexFont::subset()));
    run(&mut r, LIST);
    assert!(r.sys.games().can_continue(id::TILES), "the save file is found");
    run(&mut r, "down ok wait:100 expect-screen:Game4096");
    assert_eq!(tiles(&r).cells(), &board[..]);
    assert_eq!(tiles(&r).moves(), moves);
    // Playing another game puts this one into its file first.
    run(&mut r, "back up ok wait:100 up ok wait:100 expect-screen:Blocks");
    assert!(file(&mut r, "4096.sav").is_some());
}

/// A game that ends is counted in `scores.bin` and its save file goes;
/// the list then shows the best result.
#[test]
fn blocks_game_over_records_the_score() {
    let mut s = sim(&format!("{LIST} ok wait:100 ok wait:300 back expect-screen:GamePause"));
    assert!(file(&mut s, "blocks.sav").is_some());
    run(&mut s, "ok set:blocksrows=16 goto:Blocks ok");
    for _ in 0..12 {
        if blocks(&s).ended() {
            break;
        }
        run(&mut s, "hold:down wait:300");
    }
    assert!(blocks(&s).ended(), "topped out");
    let score = blocks(&s).score();
    let scores = Scores::decode(&file(&mut s, app::SCORES_FILE).expect("scores.bin")).expect("valid");
    let rec = scores.get(id::BLOCKS, Blocks::new(Default::default(), 0).config().variant()).expect("record");
    assert_eq!((rec.played, rec.best), (1, score));
    assert!(file(&mut s, "blocks.sav").is_none(), "a finished game is not offered again");
    assert!(!s.sys.games().can_continue(id::BLOCKS));
    // OK on the game-over banner starts a new game.
    run(&mut s, "ok ok wait:300");
    assert!(!blocks(&s).ended());
}

/// Menu in Blocks is the hold key: no context menu opens; holding Menu
/// still opens the quick panel.
#[test]
fn menu_holds_a_piece_and_opens_no_menu() {
    let mut s = sim(&format!("{BLOCKS} ok wait:300"));
    let first = blocks(&s).active().unwrap().piece;
    run(&mut s, "menu");
    assert!(!s.shell.menu_open());
    assert_eq!(blocks(&s).held_piece(), Some(first));
    run(&mut s, "hold:menu expect-screen:QuickPanel");
}

/// Any interruption pauses (05 §7.13): the screen going off, a notice over
/// the game. Only the pause menu's Resume plays on at once.
#[test]
fn interruptions_pause_and_resume_plays_on() {
    let mut s = sim(&format!("{BLOCKS} ok wait:500"));
    let steps = blocks(&s).steps();
    assert!(steps > 20);
    run(&mut s, "power wait:2000");
    let off = blocks(&s).steps();
    assert!(off - steps < 5, "stopped with the screen off ({steps} → {off})");
    run(&mut s, "power wait:1000");
    assert_eq!(blocks(&s).steps(), off, "waits for OK after the screen comes back");
    run(&mut s, "ok wait:500");
    assert!(blocks(&s).steps() > off + 20);
    // A notice over the game: back to a paused game.
    run(&mut s, "set:battery=5 expect-screen:Notice back expect-screen:Blocks wait:500");
    let n = blocks(&s).steps();
    run(&mut s, "wait:500");
    assert_eq!(blocks(&s).steps(), n);
    // Pause menu › Resume: playing at once.
    run(&mut s, "ok wait:100 back expect-screen:GamePause ok expect-screen:Blocks wait:500");
    assert!(blocks(&s).steps() > n + 20);
    // Back in the pause menu is Resume too.
    let m = blocks(&s).steps();
    run(&mut s, "back back expect-screen:Blocks wait:500");
    assert!(blocks(&s).steps() > m + 20);
}

/// New game during a game asks first; Cancel keeps the game.
#[test]
fn new_game_asks_first() {
    let mut s = sim(&format!("{TILES} right down"));
    let before = tiles(&s).clone();
    run(&mut s, "back down ok");
    assert!(s.shell.dialog_open());
    run(&mut s, "ok expect-screen:GamePause");
    assert_eq!(tiles(&s), &before);
    run(&mut s, "set:gameseed=99 ok next ok expect-screen:Game4096");
    assert_eq!(tiles(&s).moves(), 0);
    assert_eq!(tiles(&s).seed(), 99);
}

/// The differential tests of `tests/settings/effects.toml`: each game
/// setting at a value against its reference changes its own game (the
/// declared oracle) and nothing in the other game or on Home.
mod settings_effects {
    use super::*;

    /// The frame and the session after `script`, with `setting` set first.
    fn after(setting: &str, script: &str) -> (Vec<u16>, AppSim) {
        let s = sim(&format!("{setting} {script}"));
        (s.fb.clone(), s)
    }

    /// `key=v` changes the frame of `own` against `key=reference`, and not
    /// the frames of the other contexts.
    fn shows_only_in(key: u16, v: i16, reference: i16, own: &str) {
        let set = |x: i16| format!("set:{key:#06x}={x}");
        let (a, _) = after(&set(v), own);
        let (b, _) = after(&set(reference), own);
        assert_ne!(a, b, "{key:#06x}={v} shows in its game");
        let blocks_key = key < 0x0E20;
        let others = [
            if blocks_key { format!("{TILES} right down") } else { format!("{BLOCKS} ok wait:400") },
            "wait:10".into(),
        ];
        for other in &others {
            assert_eq!(
                after(&set(v), other).0,
                after(&set(reference), other).0,
                "{key:#06x} leaks into `{other}`"
            );
        }
    }

    fn play() -> String {
        format!("{BLOCKS} ok wait:1500")
    }

    #[test]
    fn blocks_mode() {
        shows_only_in(0x0E01, 1, 0, &play());
        shows_only_in(0x0E01, 2, 0, &play());
        let (_, s) = after("set:0x0e01=2", &play());
        assert_eq!(blocks(&s).config().mode, orpheus_games::blocks::Mode::Attack);
    }

    #[test]
    fn blocks_start_level() {
        shows_only_in(0x0E02, 15, 1, &play());
        let (_, slow) = after("set:0x0e02=1", &play());
        let (_, fast) = after("set:0x0e02=15", &play());
        assert_eq!(blocks(&fast).level(), 15);
        assert!(blocks(&fast).active().unwrap().y > blocks(&slow).active().unwrap().y + 3, "falls faster");
    }

    #[test]
    fn blocks_pieces() {
        shows_only_in(0x0E03, 1, 0, &play());
        let (_, bag) = after("set:0x0e03=0", &play());
        let (_, random) = after("set:0x0e03=1", &play());
        assert!(blocks(&bag).config().bag && !blocks(&random).config().bag);
    }

    #[test]
    fn blocks_ghost() {
        shows_only_in(0x0E04, 0, 1, &play());
        let (_, a) = after("set:0x0e04=0", &play());
        let (_, b) = after("set:0x0e04=1", &play());
        assert_eq!(blocks(&a), blocks(&b), "the game is the same");
    }

    #[test]
    fn blocks_previews() {
        for v in 0..3 {
            shows_only_in(0x0E05, v, 3, &play());
        }
    }

    #[test]
    fn blocks_hold() {
        shows_only_in(0x0E06, 0, 1, &format!("{} menu wait:100", play()));
        let (_, off) = after("set:0x0e06=0", &format!("{} menu", play()));
        assert_eq!(blocks(&off).held_piece(), None);
    }

    /// Soft drop to the floor, then wait 400 ms: locked with 0.25 s, not
    /// with 1 s.
    #[test]
    fn blocks_lock_delay() {
        let script = format!("set:0x0e02=15 {BLOCKS} ok wait:2200");
        shows_only_in(0x0E07, 0, 3, &script);
        let (_, short) = after("set:0x0e07=0", &script);
        let (_, long) = after("set:0x0e07=3", &script);
        assert!(blocks(&short).pieces() > blocks(&long).pieces());
    }

    fn column(s: &AppSim) -> i8 {
        blocks(s).active().unwrap().cells().iter().map(|c| c.0).max().unwrap()
    }

    /// Next held for 200 ms: the piece runs after 100 ms, not after 267.
    #[test]
    fn blocks_auto_shift() {
        let script = format!("{} press:next wait:200 release:next", play());
        shows_only_in(0x0E08, 0, 4, &script);
        let (_, quick) = after("set:0x0e08=0", &script);
        let (_, slow) = after("set:0x0e08=4", &script);
        assert!(column(&quick) > column(&slow));
    }

    /// Next held for 300 ms: 0 ms runs to the wall, 83 ms a cell or two.
    #[test]
    fn blocks_repeat() {
        let script = format!("{} press:next wait:300 release:next", play());
        shows_only_in(0x0E09, 0, 4, &script);
        let (_, wall) = after("set:0x0e09=0", &script);
        let (_, slow) = after("set:0x0e09=4", &script);
        assert_eq!(column(&wall), 9);
        assert!(column(&slow) < 9);
    }

    /// The long piece upright at the left wall turns only with kicks.
    #[test]
    fn blocks_kicks() {
        // Seed 7 starts with the long piece? Find a seed that does.
        let seed = (0..200u64)
            .find(|&k| {
                let s = sim(&format!("set:gameseed={k} goto:Blocks"));
                blocks(&s).active().unwrap().piece == orpheus_games::blocks::Piece::I
            })
            .expect("a seed starting with the long piece");
        let script = format!("set:gameseed={seed} goto:Blocks wait:100 ok up hold:prev:600 up wait:50");
        let (a, on) = after("set:0x0e0a=1", &script);
        let (b, off) = after("set:0x0e0a=0", &script);
        assert_ne!(a, b);
        assert_ne!(blocks(&on).active().unwrap().rot, blocks(&off).active().unwrap().rot);
        shows_only_in(0x0E0A, 0, 1, &script);
    }

    #[test]
    fn tiles_board() {
        for v in [1, 2] {
            shows_only_in(0x0E21, v, 0, TILES);
            let (_, s) = after(&format!("set:0x0e21={v}"), TILES);
            assert_eq!(tiles(&s).n(), usize::try_from(4 + v).unwrap());
        }
    }

    /// Two 1024 tiles joined: the goal banner with 2048, not with 4096.
    #[test]
    fn tiles_goal() {
        let script = "set:tiles=10.10 goto:Game4096 wait:100 left wait:200";
        shows_only_in(0x0E22, 0, 1, script);
        let (_, a) = after("set:0x0e22=0", script);
        let (_, b) = after("set:0x0e22=1", script);
        assert_eq!((tiles(&a).phase(), tiles(&b).phase()), (Phase::Won, Phase::Playing));
        let (_, endless) = after("set:0x0e22=3", "set:tiles=11.11 goto:Game4096 wait:100 left wait:200");
        assert_eq!(tiles(&endless).phase(), Phase::Playing);
    }

    /// New games' first tiles over 40 seeds: no 4 at 0 %, some at 50 %.
    #[test]
    fn tiles_four() {
        let fours = |v: i16| {
            (0..40)
                .map(|k| {
                    let s = sim(&format!("set:0x0e23={v} set:gameseed={k} goto:Game4096"));
                    tiles(&s).cells().iter().filter(|&&e| e == 2).count()
                })
                .sum::<usize>()
        };
        assert_eq!(fours(0), 0);
        assert!(fours(3) > 10);
        shows_only_in(0x0E23, 3, 1, &format!("{TILES} right down left up right"));
    }

    /// OK after a slide takes it back only with undo steps.
    #[test]
    fn tiles_undo() {
        let script = format!("{TILES} right down ok");
        shows_only_in(0x0E24, 0, 1, &script);
        let (_, none) = after("set:0x0e24=0", &script);
        let (_, one) = after("set:0x0e24=1", &script);
        assert_eq!(none.sys.games().session.clone(), {
            let (_, two) = after("set:0x0e24=0", &format!("{TILES} right down"));
            two.sys.games().session.clone()
        });
        assert_eq!(tiles(&one).moves(), 1);
    }

    /// 33 ms after a slide the tiles are on their way (on) or home (off);
    /// later the frames are the same.
    #[test]
    fn tiles_animations() {
        let mid = format!("{TILES} right press:down wait:40");
        shows_only_in(0x0E25, 0, 1, &mid);
        let late = format!("{TILES} right down wait:300");
        assert_eq!(after("set:0x0e25=0", &late).0, after("set:0x0e25=1", &late).0);
    }
}
