# Add a screen

Screens live in `crates/orpheus-apps/src/screens/` and run unchanged in the
simulator and on the player. The grammar they must follow is
[05 §3](../../plan/05-ui-apps.md); the layout rules are 05 §4.

1. **State and trait.** Write a struct and implement
   `orpheus_ui::nav::Screen` for it: `enter` (opened with an `Arg`),
   `event` (keys and service data; returns a `Nav`: stay, push, pop,
   home), `draw` (the content area; with `Dirty::Changed` draw only what
   changed), and optionally `menu` (the Menu sheet), `save`/`restore` (when
   another screen is pushed on top).
2. **Register it.** Add a variant to `AppScreen` and a `ScreenId` in
   `screens/mod.rs`, and the arm in `create`, `id` and `name` (the name is
   what scripts check with `expect-screen:`).
3. **Data.** Screens never touch hardware or files. If the screen needs
   something new, add a method to a service trait in `services.rs` and
   implement it twice: in the simulator (`sim/src/`) and in the firmware
   (`fw/firmware/src/services.rs`). Long work is requested, then delivered
   as an `Event::Data`; never block in `event` or `draw`.
4. **Strings.** Append to `strings.rs` (ids are never reused or
   reordered) and add the translations in `sim/src/packs.rs`.
5. **Look at it.**

   ```sh
   cargo run -p orpheus-sim -- --script "ok down*2 ok shot:mine"
   ```

6. **Golden images.** Add a case to `sim/tests/golden.rs` (each case is a
   key script that takes one `shot:`; most cases run for dark and light and
   for Latin, Cyrillic and CJK text). Then
   `cargo xtask test golden --bless`, open every new and changed PNG in
   `sim/golden/`, and check `target/orpheus/golden.html`.
7. **On the player** (after the gate): `cargo xtask build`,
   `cargo xtask dev stage target/orpheus/firmware.yfw`, then
   `cargo xtask dev keys '…'` and `cargo xtask dev shot out.png` to compare
   with the golden.

Rules that bite: draw through palette roles, never literal colours; text at
least 11 px; a key press changes pixels within 50 ms, so move the
highlight first and fill in data when it arrives; the Back key never
activates anything.
