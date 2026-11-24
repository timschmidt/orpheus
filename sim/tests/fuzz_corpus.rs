//! The UI fuzz target's committed corpus and regression inputs, replayed on
//! stable through the body the cargo-fuzz target runs (VER-4, 07 §3.8); and
//! the seed journeys (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-sim
//! --test fuzz_corpus seeds` rewrites `tests/fuzz-corpus/ui_keys/seed-*`).

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../tests/replay.rs"]
mod replay;

#[test]
fn ui_keys_corpus() {
    replay::replay("ui_keys", bodies::ui_keys);
}

/// Journey encoder (see `bodies::command`).
#[derive(Default)]
struct J(Vec<u8>);

const UP: u8 = 0;
const DOWN: u8 = 1;
const NEXT: u8 = 3;
const OK: u8 = 4;
const BACK: u8 = 5;
const MENU: u8 = 6;
const VOLUP: u8 = 7;
const POWER: u8 = 9;

impl J {
    fn new(sel: u8) -> J {
        J(vec![sel])
    }
    fn tap(&mut self, k: u8, n: usize) -> &mut J {
        for _ in 0..n {
            self.0.extend_from_slice(&[0, k, 0]);
        }
        self
    }
    fn hold(&mut self, k: u8, b: u8) -> &mut J {
        self.0.extend_from_slice(&[6, k, b]);
        self
    }
    fn wait(&mut self, b: u8) -> &mut J {
        self.0.extend_from_slice(&[9, 0, b]);
        self
    }
    /// `set:` by index into `SETS`.
    fn set(&mut self, i: u8) -> &mut J {
        self.0.extend_from_slice(&[10, i, 0]);
        self
    }
    fn setting(&mut self, i: u8, v: u8) -> &mut J {
        self.0.extend_from_slice(&[12, i, v]);
        self
    }
    fn look(&mut self, what: u8, i: u8) -> &mut J {
        self.0.extend_from_slice(&[13, what, i]);
        self
    }
    fn goto(&mut self, i: u8) -> &mut J {
        self.0.extend_from_slice(&[14, i, 0]);
        self
    }
    fn home(&mut self) -> &mut J {
        self.0.extend_from_slice(&[15, 0, 0]);
        self
    }
}

fn seeds() -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(&str, J)> = Vec::new();
    let mut j = J::new(0);
    j.tap(DOWN, 2).tap(OK, 1).wait(20).tap(DOWN, 1).tap(OK, 1).wait(20).tap(BACK, 2).tap(NEXT, 1).tap(OK, 1);
    v.push(("home-apps", j));
    let mut j = J::new(1);
    j.set(30).goto(1).tap(OK, 1).wait(30).tap(DOWN, 2).tap(OK, 1).tap(MENU, 1).tap(DOWN, 1).tap(OK, 1);
    j.wait(60).tap(BACK, 1).home();
    v.push(("music", j));
    let mut j = J::new(2);
    j.goto(2).wait(30).tap(DOWN, 3).tap(OK, 1).wait(30).tap(DOWN, 1).tap(OK, 1).wait(40).tap(MENU, 1);
    j.tap(DOWN, 2).tap(OK, 1).tap(BACK, 2).hold(DOWN, 40).tap(NEXT, 1);
    v.push(("files", j));
    let mut j = J::new(3);
    j.set(33).goto(3).wait(100).tap(NEXT, 1).tap(OK, 1).tap(MENU, 1).tap(DOWN, 3).tap(OK, 1).wait(50);
    j.tap(MENU, 1).tap(UP, 1).tap(OK, 1).set(37).set(40).hold(OK, 50).tap(VOLUP, 3).wait(200);
    v.push(("now-playing", j));
    let mut j = J::new(4);
    j.goto(6).tap(DOWN, 1).tap(OK, 1).tap(DOWN, 2).tap(OK, 1).tap(BACK, 1).tap(DOWN, 4).tap(OK, 1);
    j.tap(DOWN, 1).tap(OK, 1).setting(7, 1).setting(8, 3).setting(3, 200).look(0, 1).look(1, 2);
    v.push(("settings", j));
    let mut j = J::new(5);
    j.set(17).goto(14).wait(40).tap(DOWN, 1).tap(OK, 1).wait(60).set(24).wait(60).tap(OK, 1).wait(200);
    j.set(20).wait(30).tap(BACK, 1).tap(DOWN, 2).tap(OK, 1).tap(MENU, 1).tap(DOWN, 1).tap(OK, 1).set(22);
    v.push(("bluetooth", j));
    let mut j = J::new(6);
    j.hold(POWER, 70).tap(DOWN, 1).tap(OK, 1).tap(POWER, 1).tap(OK, 1).set(41).tap(OK, 1).wait(80);
    j.set(42).tap(POWER, 1).tap(DOWN, 1).hold(MENU, 40).tap(DOWN, 2).tap(OK, 1).tap(BACK, 1);
    v.push(("power-lock", j));
    let mut j = J::new(7);
    j.set(1).set(5).wait(40).set(12).wait(40).tap(OK, 1).set(14).set(9).wait(40).set(10).set(2).set(6);
    j.set(7).set(28).set(3).wait(100);
    v.push(("system-events", j));
    let mut j = J::new(0);
    j.look(2, 1).goto(1).tap(DOWN, 1).look(2, 2).goto(2).tap(DOWN, 2).look(0, 2).look(1, 4).look(2, 0);
    j.goto(6).goto(13).goto(15).goto(16).goto(5).wait(100).goto(12).goto(7).goto(8);
    v.push(("languages-screens", j));
    v.into_iter().map(|(n, j)| (n.to_string(), j.0)).collect()
}

#[test]
fn write_seeds() {
    let s = seeds();
    for (n, d) in &s {
        eprintln!("seed {n}");
        bodies::ui_keys(d);
    }
    replay::write_seeds("ui_keys", &s);
}
