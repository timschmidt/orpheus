//! M14 (GAME-0/1/2/3): the Games app over the simulator's games back end
//! (`sim/src/games.rs`): saved games and scores on the card, pause and
//! interruptions, the Menu key in Blocks, Solitaire's keys and drawing,
//! the firmware's deferred card queue (`set:gamesio=on`), and the
//! differential tests of every game setting (07 §5.12;
//! `tests/settings/effects.toml`).

use orpheus_apps::games::{self as app, GamesService};
use orpheus_games::blocks::Blocks;
use orpheus_games::bricks::{self, Bricks};
use orpheus_games::salvo::{self, Phase as SalvoPhase, Salvo};
use orpheus_games::scores::Scores;
use orpheus_games::solitaire::{self, Solitaire};
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

fn sol(s: &AppSim) -> &Solitaire {
    match &s.sys.games().session {
        Session::Solitaire(g) => g,
        other => panic!("not Solitaire: {other:?}"),
    }
}

fn salvo(s: &AppSim) -> &Salvo {
    match &s.sys.games().session {
        Session::Salvo(g) => g,
        other => panic!("not Salvo: {other:?}"),
    }
}

fn bricks_game(s: &AppSim) -> &Bricks {
    match &s.sys.games().session {
        Session::Bricks(g) => g,
        other => panic!("not Bricks: {other:?}"),
    }
}

fn file(s: &mut AppSim, name: &str) -> Option<Vec<u8>> {
    read(&mut s.sys.media, &format!("{}/{name}", app::DIR))
}

const LIST: &str = "set:games=on set:gameseed=7 prev*2 ok wait:100 expect-screen:Games";
const BLOCKS: &str = "set:gameseed=7 goto:Blocks wait:100 expect-screen:Blocks";
const TILES: &str = "set:gameseed=7 goto:Game4096 wait:100 expect-screen:Game4096";
const SOL: &str = "set:gameseed=7 goto:Solitaire wait:100 expect-screen:Solitaire";
const SALVO: &str = "set:gameseed=7 goto:Salvo wait:100 expect-screen:Salvo";
const BRICKS: &str = "set:gameseed=7 goto:Bricks wait:100 expect-screen:Bricks";

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

/// Solitaire from the Games list: a deal, a few moves, saved on pause,
/// quit and continued after a restart; hold OK takes a move back; Back
/// with cards in hand puts them back instead of pausing.
#[test]
fn solitaire_saved_undo_and_back() {
    let mut s = sim(&format!("{LIST} down*2 ok wait:100 expect-screen:Solitaire"));
    let start = sol(&s).position();
    // The stock: OK turns a card.
    run(&mut s, "up ok");
    assert_eq!(sol(&s).board().len(solitaire::WASTE), 1);
    // Hold OK: taken back.
    run(&mut s, "hold:ok");
    assert_eq!(sol(&s).position(), start, "undo restores the deal");
    run(&mut s, "ok down next");
    // Pick up a column's card, Back puts it back.
    run(&mut s, "ok");
    assert!(sol(&s).held().is_some());
    run(&mut s, "back expect-screen:Solitaire");
    assert!(sol(&s).held().is_none());
    let moves = sol(&s).moves();
    assert!(moves > 0);
    run(&mut s, "back expect-screen:GamePause");
    let saved = file(&mut s, "solitaire.sav").expect("saved on pause");
    assert_eq!(Session::decode(&saved, Some(id::SOLITAIRE)), Some(s.sys.games().session.clone()));
    run(&mut s, "up ok wait:100 expect-screen:Games");
    let media = std::mem::replace(&mut s.sys.media, orpheus_sim::media::Media::new());
    let mut r = Sim::with_sys(HostServices::with_media(media), Box::new(HexFont::subset()));
    run(&mut r, LIST);
    assert!(r.sys.games().can_continue(id::SOLITAIRE));
    run(&mut r, "down*2 ok wait:100 expect-screen:Solitaire");
    assert_eq!(sol(&r).moves(), moves);
    assert_eq!(sol(&r).board(), sol(&s).board());
}

/// Every frame drawn piece by piece equals the same frame drawn whole
/// (partial redraw: cursor frames reach into the gaps between piles).
#[test]
fn solitaire_partial_redraw_equals_full() {
    let keys = ["next", "prev", "up", "down", "ok", "menu", "hold:ok", "next*3", "up*2", "back"];
    for seed in 0..4u64 {
        let mut s = sim(&format!("set:gameseed={seed} goto:Solitaire wait:100 set:solplay=25 wait:50"));
        let mut r = orpheus_games::Rng::new(seed);
        for _ in 0..80 {
            let k = keys[r.below(keys.len() as u32) as usize];
            run(&mut s, k);
            if s.screen() != "Solitaire" {
                run(&mut s, "back");
                continue;
            }
            run(&mut s, "wait:40");
            let partial = s.fb.clone();
            s.shell.invalidate();
            s.draw();
            let diff: Vec<(usize, usize)> = (0..partial.len())
                .filter(|&i| partial[i] != s.fb[i])
                .map(|i| (i % 128, i / 128))
                .take(12)
                .collect();
            assert!(
                diff.is_empty(),
                "seed {seed}: after `{k}` the screen differs from a full draw at {diff:?}"
            );
        }
    }
}

/// The firmware's way (`set:gamesio=on`): the list asks, the card answers
/// a step later. Opening shows "Loading…" until the save is read; a game in
/// progress is written before another is read; quitting writes the save.
#[test]
fn deferred_card_queue() {
    use orpheus_apps::games::CardOp;
    // A saved Solitaire game on the card first (the direct way).
    let mut s = sim(&format!("{LIST} down*2 ok wait:100 up ok back up ok wait:100 expect-screen:Games"));
    let moves = sol(&s).moves();
    let media = std::mem::replace(&mut s.sys.media, orpheus_sim::media::Media::new());
    let mut r = Sim::with_sys(HostServices::with_media(media), Box::new(HexFont::subset()));
    run(&mut r, "set:gamesio=on set:gameseed=7 prev*2 ok wait:100 expect-screen:Games");
    assert!(r.sys.games().loaded, "the list's Load was answered");
    assert!(r.sys.games().can_continue(id::SOLITAIRE));
    // Open: the screen waits for the save file.
    run(&mut r, "down*2 ok");
    assert_eq!(r.sys.games().opening, None, "answered within the step");
    run(&mut r, "wait:100 expect-screen:Solitaire");
    assert_eq!(sol(&r).moves(), moves, "the saved game, not a new one");
    // Blocks while Solitaire is in progress: Solitaire is written first.
    let before = r.sys.games.ops.len();
    run(&mut r, "back up ok wait:100 expect-screen:Games up*2 ok wait:200 expect-screen:Blocks");
    let ops = &r.sys.games.ops[before..];
    let w = ops.iter().position(|o| *o == CardOp::Write(id::SOLITAIRE)).expect("Solitaire written");
    let rd = ops.iter().position(|o| *o == CardOp::Read(id::BLOCKS)).expect("Blocks read");
    assert!(w < rd, "written before the other is read: {ops:?}");
    assert!(matches!(r.sys.games().session, Session::Blocks(_)));
}

/// Casino scoring: a deal costs 52 from the bank, cards on the
/// foundations pay 5 when the game is left for a new one.
#[test]
fn casino_bank_carries_over() {
    let mut s = sim(&format!("set:0x0e43=1 set:0x0e44=0 set:0x0e45=0 {SOL}"));
    assert_eq!(s.sys.games().scores.bank(id::SOLITAIRE), -52);
    let scores = orpheus_games::scores::Scores::decode(&file(&mut s, app::SCORES_FILE).unwrap()).unwrap();
    assert_eq!(scores.bank(id::SOLITAIRE), -52, "written at the deal");
    // Some cards up with Menu (as many as can go), then a new game.
    run(&mut s, "set:solnear=2 goto:Solitaire wait:50");
    let up = sol(&s).board().on_foundations() as i32;
    run(&mut s, "back down ok");
    assert!(s.shell.dialog_open(), "a game in progress: New game asks");
    run(&mut s, "next ok wait:100 expect-screen:Solitaire");
    assert_eq!(s.sys.games().scores.bank(id::SOLITAIRE), -52 + 5 * up - 52);
}

/// The differential tests of `tests/settings/effects.toml`: each game
/// setting at a value against its reference changes its own game (the
/// declared oracle) and nothing in the other game or on Home.
mod settings_effects {
    use super::*;

    /// The frame and the session after `script`, with `setting` set first.
    pub(super) fn after(setting: &str, script: &str) -> (Vec<u16>, AppSim) {
        let s = sim(&format!("{setting} {script}"));
        (s.fb.clone(), s)
    }

    /// `key=v` changes the frame of `own` against `key=reference`, and not
    /// the frames of the other contexts.
    pub(super) fn shows_only_in(key: u16, v: i16, reference: i16, own: &str) {
        let set = |x: i16| format!("set:{key:#06x}={x}");
        let (a, _) = after(&set(v), own);
        let (b, _) = after(&set(reference), own);
        assert_ne!(a, b, "{key:#06x}={v} shows in its game");
        // The other games, and Home.
        let mut others = vec!["wait:10".to_string()];
        if key >= 0x0E20 {
            others.push(format!("{BLOCKS} ok wait:400"));
        }
        if !(0x0E21..0x0E40).contains(&key) {
            others.push(format!("{TILES} right down"));
        }
        if !(0x0E41..0x0E60).contains(&key) {
            others.push(format!("{SOL} up ok next ok"));
        }
        if !(0x0E61..0x0EA0).contains(&key) {
            others.push(format!("{SALVO} up right press:ok release:ok wait:1000"));
        }
        if !(0x0EA0..0x0EC0).contains(&key) {
            others.push(format!("{BRICKS} press:prev wait:200 release:prev ok wait:800"));
        }
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

    // ------------------------------------------------------------ Solitaire

    /// Draw three: OK on the stock turns three cards (the waste fans).
    #[test]
    fn solitaire_draw() {
        let script = format!("{SOL} up ok wait:50");
        shows_only_in(0x0E41, 1, 0, &script);
        let (_, three) = after("set:0x0e41=1", &script);
        assert_eq!(sol(&three).board().len(solitaire::WASTE), 3);
        assert_eq!(sol(&three).config().draw, 3);
    }

    /// One pass: after the stock is gone through, OK on it does nothing
    /// (unlimited: the waste goes back).
    #[test]
    fn solitaire_passes() {
        let script = format!("set:0x0e44=0 {SOL} up {} wait:50", "ok ".repeat(26));
        shows_only_in(0x0E42, 2, 0, &script);
        let (_, one) = after("set:0x0e42=2", &script);
        let (_, many) = after("set:0x0e42=0", &script);
        let (_, more) = after("set:0x0e42=2", &format!("set:0x0e44=0 {SOL} up {} wait:50", "ok ".repeat(30)));
        assert_eq!(sol(&one).board(), sol(&more).board(), "OK on the empty stock does nothing");
        assert_eq!(sol(&one).board().passes(), 0);
        assert!(sol(&many).board().passes() > 0);
        assert_eq!(sol(&one).config().passes, 1);
        let (_, three) = after("set:0x0e42=1", &script);
        assert_eq!(sol(&three).config().passes, 3);
    }

    /// Standard points, the casino bank, or the moves.
    #[test]
    fn solitaire_scoring() {
        let script = format!("{SOL} up ok wait:50");
        shows_only_in(0x0E43, 1, 0, &script);
        shows_only_in(0x0E43, 2, 0, &script);
        let (_, casino) = after("set:0x0e43=1", &script);
        assert_eq!(sol(&casino).score(), sol(&casino).board().on_foundations() as i32 * 5 - 52);
        assert_eq!(casino.sys.games().scores.bank(id::SOLITAIRE), -52);
        let (_, none) = after("set:0x0e43=2", &script);
        assert_eq!(none.sys.games().scores.bank(id::SOLITAIRE), 0);
    }

    /// Auto-move: the deal's aces (and safe twos) go up at once.
    #[test]
    fn solitaire_auto_move() {
        let seed = (0..100u64)
            .find(|&k| {
                let s = sim(&format!("set:0x0e44=1 set:gameseed={k} goto:Solitaire"));
                sol(&s).board().on_foundations() > 0
            })
            .expect("a deal with an ace on top");
        let script = format!("set:gameseed={seed} goto:Solitaire wait:100");
        shows_only_in(0x0E44, 0, 1, &script);
        let (_, off) = after("set:0x0e44=0", &script);
        assert_eq!(sol(&off).board().on_foundations(), 0);
    }

    /// Auto-complete: a game with every card up plays itself out.
    #[test]
    fn solitaire_auto_complete() {
        let script = "set:solnear=4 goto:Solitaire wait:1000";
        shows_only_in(0x0E45, 0, 1, script);
        let (_, on) = after("set:0x0e45=1", script);
        let (_, off) = after("set:0x0e45=0", script);
        assert!(sol(&on).won());
        assert!(!sol(&off).won());
    }

    /// The timer: shown or not; the game is the same.
    #[test]
    fn solitaire_timer() {
        let script = format!("{SOL} wait:2100");
        shows_only_in(0x0E46, 0, 1, &script);
        let (_, a) = after("set:0x0e46=0", &script);
        let (_, b) = after("set:0x0e46=1", &script);
        assert_eq!(sol(&a), sol(&b));
        assert!(sol(&a).steps() >= 120);
    }

    /// Winnable deals: the deal check chooses another deal for some seeds.
    #[test]
    fn solitaire_winnable() {
        let seed = (0..50u64)
            .find(|&k| {
                let on = sim(&format!("set:0x0e47=1 set:gameseed={k} goto:Solitaire"));
                let off = sim(&format!("set:0x0e47=0 set:gameseed={k} goto:Solitaire"));
                sol(&on).seed() != sol(&off).seed()
            })
            .expect("a seed whose own deal the check does not win");
        let script = format!("set:gameseed={seed} goto:Solitaire wait:100");
        shows_only_in(0x0E47, 0, 1, &script);
        let (_, on) = after("set:0x0e47=1", &script);
        assert!(sol(&on).checked());
    }
}

/// Salvo (GAME-4/5): every setting shows in the game and nowhere else.
mod salvo_settings {
    use super::settings_effects::{after, shows_only_in};
    use super::*;

    /// Fire the aimed shot and watch it fly.
    const FIRE: &str = "press:ok release:ok wait:1500";
    /// Smoke (no damage), fired straight up and settled.
    const SMOKE: &str = "menu down*6 ok set:salvoaim=0.90.150 press:ok release:ok wait:6000";

    #[test]
    fn players_and_who_plays() {
        shows_only_in(0x0E61, 1, 0, SALVO);
        let (_, three) = after("set:0x0e61=1", SALVO);
        assert_eq!(salvo(&three).tanks().len(), 3);
        // Player 1 a computer: it thinks at once.
        shows_only_in(0x0E62, 4, 0, SALVO);
        let (_, cpu) = after("set:0x0e62=4", SALVO);
        assert!(matches!(salvo(&cpu).phase(), SalvoPhase::Think | SalvoPhase::Turn | SalvoPhase::Flight));
        // Player 2 a human too: the pass screen.
        shows_only_in(0x0E63, 0, 2, SALVO);
        let (_, two) = after("set:0x0e63=0", SALVO);
        assert_eq!(salvo(&two).phase(), SalvoPhase::Pass);
        // Players 3 and 4 when they play.
        shows_only_in(0x0E64, 0, 2, &format!("set:0x0e61=1 {SALVO}"));
        shows_only_in(0x0E65, 0, 2, &format!("set:0x0e61=2 {SALVO}"));
        let (_, four) = after("set:0x0e61=2 set:0x0e65=4", SALVO);
        assert_eq!(salvo(&four).config().kinds[3], salvo::Kind::Expert);
    }

    #[test]
    fn rounds_and_timer() {
        shows_only_in(0x0E66, 0, 1, SALVO);
        let (_, ten) = after("set:0x0e66=3", SALVO);
        assert_eq!(salvo(&ten).config().rounds, 10);
        shows_only_in(0x0E67, 1, 0, SALVO);
        let (_, timed) = after("set:0x0e67=1", &format!("{SALVO} wait:16000"));
        assert!(salvo(&timed).phase() != SalvoPhase::Aim || salvo(&timed).turn() != 0, "the timer fired");
    }

    #[test]
    fn terrain() {
        for (key, v, r) in [(0x0E68, 3, 0), (0x0E68, 2, 0), (0x0E69, 2, 1), (0x0E6A, 2, 1), (0x0E6B, 5, 0)] {
            shows_only_in(key, v, r, SALVO);
        }
        // A terrain seed: the same ground in another game.
        let a = after("set:0x0e6b=5 set:gameseed=1", "goto:Salvo wait:100").1;
        let b = after("set:0x0e6b=5 set:gameseed=2", "goto:Salvo wait:100").1;
        assert_eq!(salvo(&a).heights(), salvo(&b).heights());
        let (_, isl) = after("set:0x0e68=3", SALVO);
        assert!(salvo(&isl).water() > 0);
    }

    #[test]
    fn world_width() {
        shows_only_in(0x0E6C, 3, 2, SALVO);
        shows_only_in(0x0E6C, 0, 2, SALVO);
        let (_, w) = after("set:0x0e6c=3", SALVO);
        assert_eq!(salvo(&w).width(), 384);
    }

    #[test]
    fn gravity_drag_wind_gusts() {
        let fire = format!("{SALVO} {FIRE}");
        shows_only_in(0x0E6D, 0, 2, &fire);
        shows_only_in(0x0E71, 0, 1, &fire);
        shows_only_in(0x0E6E, 0, 1, SALVO);
        shows_only_in(0x0E6F, 30, 8, &format!("set:0x0e6e=3 {SALVO}"));
        let (_, calm) = after("set:0x0e6e=0", SALVO);
        assert_eq!(salvo(&calm).wind_tenths(), 0);
        let (_, custom) = after("set:0x0e6e=3 set:0x0e6f=30", SALVO);
        assert_eq!(salvo(&custom).config().wind, 30);
        shows_only_in(0x0E70, 1, 0, &format!("set:0x0e6e=2 {fire}"));
        let (_, moon) = after("set:0x0e6d=0", SALVO);
        assert_eq!(salvo(&moon).config().gravity, 0);
    }

    #[test]
    fn health_fall_weapons_shop_guide() {
        shows_only_in(0x0E72, 0, 1, &format!("{SALVO} set:salvohealth=0.40 wait:50"));
        let (_, fifty) = after("set:0x0e72=0", SALVO);
        assert!(salvo(&fifty).tanks().iter().all(|t| t.health == 50));
        // A tank left floating falls 20 px when the shot settles.
        let fall = format!(
            "{SALVO} set:salvoflat=60 set:salvoplace=0.30 set:salvoplace=1.200 set:salvoflat=40 set:salvoplace=0.30 {SMOKE}"
        );
        shows_only_in(0x0E73, 0, 1, &fall);
        let (_, hurt) = after("set:0x0e73=1", &fall);
        let (_, safe) = after("set:0x0e73=0", &fall);
        assert!(salvo(&hurt).tanks()[1].health < salvo(&safe).tanks()[1].health);
        shows_only_in(0x0E74, 0, 1, &format!("{SALVO} menu"));
        // The shop after the first round.
        let won = format!("{SALVO} set:salvohealth=1.0 {SMOKE} ok wait:100");
        shows_only_in(0x0E75, 0, 1, &won);
        let (_, shop) = after("set:0x0e75=1", &won);
        assert_eq!(salvo(&shop).phase(), SalvoPhase::Shop);
        // The aim guide: a preview, the last shot, nothing.
        shows_only_in(0x0E76, 2, 0, SALVO);
        let (_, a) = after("set:0x0e76=2", SALVO);
        let (_, b) = after("set:0x0e76=0", SALVO);
        assert_eq!(salvo(&a), salvo(&b), "the game is the same");
    }
}

/// Salvo in the app: a game saved on pause and continued, a computer's
/// turn played out, the settings' groups.
#[test]
fn salvo_saved_and_continued() {
    let mut s = sim(&format!("{LIST} down*3 ok wait:100 expect-screen:Salvo up up right"));
    let angle = salvo(&s).tanks()[0].angle;
    run(&mut s, "back expect-screen:GamePause");
    let saved = file(&mut s, "salvo.sav").expect("saved on pause");
    let Some(Session::Salvo(back)) = Session::decode(&saved, Some(id::SALVO)) else {
        panic!("no Salvo save")
    };
    assert_eq!(back.tanks()[0].angle, angle);
    assert_eq!(back.heights(), salvo(&s).heights());
    run(&mut s, "ok wait:100 expect-screen:Salvo press:ok release:ok wait:30000");
    assert!(salvo(&s).tanks().iter().any(|t| t.health < 100) || salvo(&s).turn() == 0, "shots were fired");
    // Settings: three groups, each its own list.
    run(
        &mut s,
        "back expect-screen:GamePause down*2 ok expect-screen:GameSettings down ok expect-screen:GameSettings",
    );
    run(&mut s, "back back down*2 ok expect-screen:Games");
    assert!(s.sys.games().can_continue(id::SALVO));
}

/// Bricks (GAME-6): every setting shows in the game and nowhere else.
mod bricks_settings {
    use super::settings_effects::{after, shows_only_in};
    use super::*;

    #[test]
    fn start_level_and_level_set() {
        shows_only_in(0x0EA1, 5, 1, BRICKS);
        let (_, five) = after("set:0x0ea1=5", BRICKS);
        assert_eq!(bricks_game(&five).level(), 5);
        // Hand-made, generated, mixed: level 1 hand-made in two of them.
        shows_only_in(0x0EA6, 1, 2, BRICKS);
        shows_only_in(0x0EA6, 0, 2, &format!("set:0x0ea1=9 {BRICKS}"));
        let (_, gen_) = after("set:0x0ea6=1", BRICKS);
        let cells: Vec<u8> = bricks_game(&gen_).cells().iter().map(|c| c & !bricks::CARRY).collect();
        assert_eq!(cells, bricks::layout(bricks::LevelSet::Generated, 1, 7).to_vec());
    }

    #[test]
    fn ball_speed_paddle_lives() {
        let play = format!("{BRICKS} ok wait:500");
        shows_only_in(0x0EA2, 2, 1, &play);
        shows_only_in(0x0EA2, 0, 1, &play);
        let (_, fast) = after("set:0x0ea2=2", BRICKS);
        assert_eq!(bricks_game(&fast).speed(), bricks::BASE_SPEEDS[2]);
        shows_only_in(0x0EA3, 0, 1, BRICKS);
        shows_only_in(0x0EA3, 2, 1, BRICKS);
        let (_, small) = after("set:0x0ea3=0", BRICKS);
        assert_eq!(bricks_game(&small).paddle().1, bricks::PADDLE_WIDTHS[0]);
        shows_only_in(0x0EA4, 2, 1, BRICKS);
        shows_only_in(0x0EA4, 0, 1, BRICKS);
        let (_, five) = after("set:0x0ea4=2", BRICKS);
        assert_eq!(bricks_game(&five).lives(), 5);
    }

    /// Power-ups: with them a brick that carries one drops a capsule when
    /// the ball breaks it; without, no brick carries one.
    #[test]
    fn power_ups() {
        // A seed whose level 1 has a capsule in an exposed brick (the
        // bottom row), and a shot straight up under it.
        let (seed, col) = (0..100u64)
            .find_map(|seed| {
                let (_, s) = after(&format!("set:gameseed={seed}"), "goto:Bricks wait:50");
                let g = bricks_game(&s);
                (0..bricks::COLS).find(|&c| g.cell(5, c) & bricks::CARRY != 0).map(|c| (seed, c))
            })
            .expect("a capsule in the bottom row");
        let x = bricks::BRICKS_X + col as i32 * bricks::BRICK_W + 3;
        let shot = format!("set:gameseed={seed} goto:Bricks wait:50 ok set:bricksball=0.{x}.48.0.2 wait:150");
        shows_only_in(0x0EA5, 0, 1, &shot);
        let (_, on) = after("set:0x0ea5=1", &shot);
        assert!(bricks_game(&on).capsules().iter().any(|c| c.kind != 0), "a capsule falls");
        let (_, off) = after("set:0x0ea5=0", &shot);
        assert!(bricks_game(&off).capsules().iter().all(|c| c.kind == 0));
        assert!(bricks_game(&off).cells().iter().all(|c| c & bricks::CARRY == 0));
    }

    /// Steering: the paddle's speed, at once (also in a game in progress).
    #[test]
    fn steering() {
        let slide = format!("{BRICKS} press:next wait:300 release:next");
        shows_only_in(0x0EA7, 2, 1, &slide);
        shows_only_in(0x0EA7, 0, 1, &slide);
        let x = |v: i16| bricks_game(&after(&format!("set:0x0ea7={v}"), &slide).1).paddle().0;
        assert!(x(0) < x(1) && x(1) < x(2));
        // Changed in the pause menu: the game in progress takes it.
        let mut s = sim(&format!("{BRICKS} back expect-screen:GamePause"));
        run(&mut s, "set:0x0ea7=2 back expect-screen:Bricks press:next wait:300 release:next");
        assert_eq!(bricks_game(&s).config().sens, 2);
        assert_eq!(bricks_game(&s).paddle().0, x(2));
    }
}

/// Bricks in the app: a game played, saved on pause, quit, continued
/// after a restart of the player; a lost game recorded in the scores.
#[test]
fn bricks_saved_and_continued() {
    let mut s = sim(&format!(
        "{LIST} down*4 ok wait:100 expect-screen:Bricks press:next wait:200 release:next ok wait:600"
    ));
    run(&mut s, "back expect-screen:GamePause");
    let saved = file(&mut s, "bricks.sav").expect("saved on pause");
    assert_eq!(Session::decode(&saved, Some(id::BRICKS)), Some(s.sys.games().session.clone()));
    run(&mut s, "up ok wait:100 expect-screen:Games");
    assert!(s.sys.games().can_continue(id::BRICKS));
    let game = bricks_game(&s).clone();
    let media = std::mem::replace(&mut s.sys.media, orpheus_sim::media::Media::new());
    let mut r = Sim::with_sys(HostServices::with_media(media), Box::new(HexFont::subset()));
    run(&mut r, LIST);
    run(&mut r, "down*4 ok wait:100 expect-screen:Bricks");
    assert_eq!(bricks_game(&r), &game, "the same game after a restart");
    // One life left, the ball falling past the paddle: over, recorded.
    run(&mut r, "ok wait:50 set:brickslives=1 set:bricksball=0.2.100.0.0 wait:600");
    assert!(bricks_game(&r).ended());
    let scores = Scores::decode(&file(&mut r, app::SCORES_FILE).unwrap()).unwrap();
    let v = bricks_game(&r).config().variant();
    assert_eq!(scores.get(id::BRICKS, v).map(|rec| rec.played), Some(1));
    assert!(file(&mut r, "bricks.sav").is_none(), "a finished game's save is removed");
}

/// Every Bricks frame drawn in parts equals the same frame drawn whole:
/// balls, capsules and the paddle moving over bricks and each other.
#[test]
fn bricks_partial_redraw_equals_full() {
    let keys = [
        "press:next",
        "release:next",
        "press:prev",
        "release:prev",
        "ok",
        "wait:200",
        "wait:500",
        "set:brickscap=1.30.60",
        "set:brickscap=3.60.96",
        "set:brickscap=4.70.97",
        "set:brickscap=2.10.70",
        "set:bricksball=1.50.70.3.2",
        "set:bricksspeed=1280",
    ];
    for seed in 0..4u64 {
        let mut s = sim(&format!("set:gameseed={seed} set:0x0ea4=2 goto:Bricks wait:100 ok"));
        let mut r = orpheus_games::Rng::new(seed);
        for step in 0..120 {
            if s.screen() != "Bricks" {
                run(&mut s, "back");
                continue;
            }
            if bricks_game(&s).ended() {
                run(&mut s, "ok");
            }
            let k = keys[r.below(keys.len() as u32) as usize];
            // A place may be taken: then nothing happens.
            let _ = s.run_script(k);
            run(&mut s, "wait:34");
            let partial = s.fb.clone();
            s.shell.invalidate();
            s.draw();
            let diff: Vec<(usize, usize)> = (0..partial.len())
                .filter(|&i| partial[i] != s.fb[i])
                .map(|i| (i % 128, i / 128))
                .take(12)
                .collect();
            assert!(
                diff.is_empty(),
                "seed {seed} step {step}: after `{k}` the screen differs from a full draw at {diff:?}"
            );
        }
    }
}

/// The game loop's cost on the host (05 §7.13: up to 30 frames/s, partial
/// redraw): per 33 ms frame, the logic steps and the drawing, and the
/// panel rows a frame leaves to flush (the firmware flushes the band of
/// dirty rows). Printed for docs/STATUS.md; the bounds only catch a
/// frame that redraws the whole screen every time.
#[test]
fn game_loop_cost() {
    let frames = 600u32;
    let measure = |name: &str, setup: &str, keys: &[&str]| {
        let mut s = sim(setup);
        let _ = s.take_dirty();
        let mut rows = 0u64;
        let t0 = std::time::Instant::now();
        for f in 0..frames {
            if !keys.is_empty() && f % 6 == 0 {
                // A key about every 200 ms.
                run(&mut s, keys[(f / 6) as usize % keys.len()]);
            } else {
                s.advance(33);
            }
            let d = s.take_dirty();
            if !d.is_empty() {
                let b = d.bounds();
                rows += u64::try_from(b.h).unwrap_or(0);
            }
        }
        let us = t0.elapsed().as_secs_f64() * 1e6 / f64::from(frames);
        let per = rows as f64 / f64::from(frames);
        println!(
            "{name}: {us:.1} µs per frame on the host (sim, all services), {per:.1} dirty rows per frame"
        );
        per
    };
    let home = measure("home (idle baseline)", "wait:10", &[]);
    let blocks = measure("blocks, falling", &format!("{BLOCKS} ok wait:100"), &["next", "prev", "up"]);
    let sol = measure("solitaire, cursor", &format!("{SOL} set:solplay=30"), &["next", "up", "prev", "down"]);
    let sol_idle = measure("solitaire, timer only", SOL, &[]);
    let _ = measure("salvo, aiming", SALVO, &["up", "right", "down"]);
    let _ = measure("salvo, a flight", &format!("{SALVO} set:salvoaim=0.70.900 press:ok release:ok"), &[]);
    let _ = measure("salvo, computers", &format!("set:0x0e62=4 set:0x0e63=4 {SALVO}"), &[]);
    let bricks = measure(
        "bricks, in play (balls, capsules, the paddle moving)",
        &format!("set:0x0ea4=2 {BRICKS} set:brickslives=9 ok set:brickscap=3.60.96 wait:300"),
        &["press:next", "release:next", "ok", "press:prev", "release:prev", "ok"],
    );
    assert!(home < 1.0);
    assert!(blocks < 128.0 * 0.9, "Blocks redraws the whole screen every frame ({blocks:.1} rows)");
    assert!(sol < 128.0 * 0.6, "Solitaire redraws too much per frame ({sol:.1} rows)");
    assert!(sol_idle < 2.0, "the timer alone ({sol_idle:.1} rows per frame)");
    assert!(bricks < 128.0 * 0.9, "Bricks redraws the whole screen every frame ({bricks:.1} rows)");
}
