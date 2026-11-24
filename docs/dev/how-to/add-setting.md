# Add a setting

The settings model is one table in `crates/orpheus-apps/src/settings.rs`:
every group and item with its store key, kind, range and default. The
Settings screens are generated from it; the firmware keeps the values in
the NOR settings store (`orpheus-settings`, at `0x7DA000`).

1. **Pick a key.** Keys are `u16`, the group in the high byte
   (`0x0101..=0x0CFF`; System is `0x0C..`). Take the next free one in the
   group and **never reuse** a key, even of a removed setting: old stores
   on players still hold it.
2. **Add the item** to the group's list, for example a choice:

   ```rust
   c(0x0C07, Str::Performance, ch![S(Str::PerfNormal), S(Str::PerfMaximum)], 0),
   ```

   Kinds: `Toggle`, `Choice`, `Range` (with a unit), `Action`, `Info`,
   plus the special `Language` and `HomeApps`. Every stored
   value is an `i16`; decoding validates it against the kind and falls back
   to the default, so a corrupt record never reaches the UI.
3. **Name it** with `pub const NAME: SettingId = SettingId(0x0C07);` in the
   `id` module, so code refers to it by name.
4. **Strings**: append the label and choice names to `strings.rs`, with
   translations in `sim/src/packs.rs`.
5. **Use it.** In the firmware, read it with `settings.get(id::NAME)` where
   the behaviour lives (for example the governor reads
   `id::PERFORMANCE`). If the setting must act at once, handle its change in
   the service that applies settings. The simulator stores it the same way.
6. **Factory reset** sets every item back to its default with no extra
   code.
7. **Goldens**: a new row moves the lists below it; re-bless and look at
   the changed images (`cargo xtask test golden --bless`). If the setting
   is visible to users, describe it in `docs/user/settings.md` (or the
   app's page).

Raise `STORED` (the number of stored settings; a unit test checks it).
The firmware's boot-time key buffer is sized from it.
