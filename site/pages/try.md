# Try Orpheus

> **Coming.** This page will run Orpheus in your browser: the same screens,
> menus, music and picture code that run on the player, compiled to
> WebAssembly, with a virtual card of freely licensed music and room for
> your own files (which never leave your browser). It is work package
> WEB-2 and is not built yet.

## Until then: the desktop simulator

The simulator runs the real apps on your computer. With
[Rust](https://rustup.rs) installed:

```sh
git clone https://github.com/timschmidt/orpheus && cd orpheus
cargo run -p orpheus-sim --features window -- --window
```

Keys: the arrow keys are Up, Down, Prev and Next; Enter is OK; Backspace is
Back; M is Menu; + and − change the volume; P is Power; S saves a
screenshot. The [key grammar](manual/keys.html) is the same as on the
player.

The simulator's card is generated: a few folders of short test tracks and
pictures. More in [getting started](dev/getting-started.html).
