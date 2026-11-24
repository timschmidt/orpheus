# yoton-ui

The UI toolkit of `docs/plan/05-ui-apps.md` §5 (WP UI-1). `#![no_std]`, no
allocator, `#![forbid(unsafe_code)]`, no hardware. The same code runs on the
player and in the simulator (`sim/`, crate `yoton-sim`), which renders the
golden images in `sim/golden/`.

## What is here

| Module | Content |
|---|---|
| `gfx` | `Gfx`: drawing over the 128 × 128 RGB565 frame buffer with a local origin and clip (`with_area`, `with_clip`), fills, anti-aliased rounded rectangles and discs, scrims, 1-bit icons, RGB565 images, text (measure, ellipsis, wrap, RTL runs reversed), dirty-rectangle tracking (`DirtySet`, up to four rectangles). Also an `embedded-graphics` `DrawTarget<Color = Rgb565>`. |
| `theme` | `Role` (8 palette roles), `Palette`, `Theme::{Dark, Light, Amber}`, `Accent::{Blue, Green, Pink, Red, Grey}`. A unit test checks 4.5:1 contrast for every text/background pair in every combination. |
| `font` | Built-in bitmap fonts (`UI`, `UI_BOLD`, `NUM28`, `NUM16`), the fallback-font interface `GlyphSource` (33-byte records) and its 64-entry RAM cache `GlyphCache`. |
| `text` | `StrBuf<N>` (inline string, number and duration formatting without `core::fmt`), visual ordering of right-to-left runs, CJK break rules. |
| `event`, `input` | `Key`, `KeyPhase`, `Event`, `Instant`; `KeyMapper` (press/hold/repeat/long, accelerating repeat, 180° key swap) and `KeyGate` (screen-off wake, key lock). All thresholds are in `input::Timing`. |
| `nav` | `Screen` and `ScreenSet` traits, `Nav`, `Cx`, `Wants`, `Dirty`, `Resume`, the 8-entry `NavStack`. |
| `shell` | `Shell`: owns the stack, the top screen and the overlays; applies the global key grammar (05 §3); draws status bar, content, strip and overlays, only where something changed. `System` trait for what the shell needs from the firmware. |
| `menu` | `MenuBuilder` (what a screen offers) and `MenuSheet` (the bottom sheet: actions, toggles, radio groups, sub-menus, value rows). |
| `widgets` | `list::ListView` (virtual, `ListSource`, headers, icons, values, chevron/toggle/radio/check accessories, placeholders, marquee, letter jump, scroll indicator), `dialog::Dialog`, `overlay::{Toast, VolumeOverlay}`, `bars` (status bar, hint strip, mini-player), `misc` (`draw_progress`, `draw_spinner`, `draw_big_value`). `Slider`, `Picker`, `Tabs`, `Meter` and `TextEntry` are not written yet (UI-2…UI-6). |

## API for wiring the firmware

```rust
pub enum Key { Up, Down, Prev, Next, Ok, Back, Menu, VolUp, VolDown, Power }
pub enum KeyPhase { Down, Repeat, Hold, Long, Up { held: bool } }   // Long: Power held 4 s
pub struct Instant(pub u64);                                        // ms since boot
pub enum Event { Key(Key, KeyPhase), Tick(Instant), Service(ServiceEvent), Data(DataReady), Ui(UiEvent) }
pub enum Nav { Stay, Push(ScreenId, Arg), Replace(ScreenId, Arg), Pop, PopTo(ScreenId), Home }
pub struct ScreenId(pub u8);
pub struct Arg(pub u32);

pub struct Cx<'a, S: ?Sized> { pub svc: &'a mut S, pub ui: &'a mut UiRequests, pub now: Instant }

pub trait Screen<S: ?Sized> {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg);
    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav;
    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty);   // content area only
    fn menu(&self, _cx: &Cx<'_, S>, _out: &mut MenuBuilder) {}
    fn wants(&self) -> Wants { Wants::default() }       // ticks, full screen, strip use, title, is_player
    fn save(&self) -> Resume { Resume::default() }      // ≤ 64-byte resume record
    fn restore(&mut self, _cx: &mut Cx<'_, S>, _r: &Resume) {}
}
pub trait ScreenSet<S: ?Sized>: Screen<S> + Sized {   // the app's `enum ScreenState`
    const HOME: ScreenId;
    const QUICK_PANEL: Option<ScreenId> = None;   // pushed on Menu hold
    const LOCK_SCREEN: Option<ScreenId> = None;   // pushed on a locked key
    const POWER_MENU: Option<ScreenId> = None;    // pushed on Power held 1.5 s
    fn global(cx: &mut Cx<'_, S>, ev: &Event) -> Nav { Nav::Stay } // sees service events first
    fn create(id: ScreenId) -> Self;
    fn id(&self) -> ScreenId;
    fn name(&self) -> &'static str;
}

pub trait System {                                   // implemented by the firmware's service layer
    fn status(&self) -> Status;                      // clock, battery, BT, source, lock, timers
    fn volume_step(&mut self, delta: i8) -> (u8, u8);
    fn mini_player(&self) -> Option<MiniPlayer<'_>>;
}

impl<'a> Gfx<'a> {
    pub fn new(px: &'a mut [u16], dirty: &'a mut DirtySet, pal: Palette, fallback: Option<Fallback<'a>>) -> Self;
}
pub struct Fallback<'a> { pub cache: &'a mut GlyphCache, pub src: &'a mut dyn GlyphSource }
pub trait GlyphSource {
    fn read(&mut self, cp: u16, out: &mut [u8; 33]) -> bool;   // record at base + cp * 33
    fn baseline(&self) -> u8 { 13 }
}

impl<T: ScreenSet<S>, S: System + ?Sized> Shell<T, S> {
    pub fn new(svc: &mut S, now: Instant) -> Self;
    pub fn with_root(svc: &mut S, root: ScreenId, arg: Arg, now: Instant) -> Self; // boot, charging
    pub fn key(&mut self, svc: &mut S, k: Key, p: KeyPhase, now: Instant) -> Requests;
    pub fn event(&mut self, svc: &mut S, ev: Event, now: Instant) -> Requests;
    pub fn tick(&mut self, svc: &mut S, now: Instant) -> Requests;   // every TICK_MS while wants_ticks()
    pub fn wants_ticks(&self, svc: &S) -> bool;
    pub fn draw(&mut self, svc: &mut S, g: &mut Gfx<'_>);
    pub fn invalidate(&mut self);                                   // theme change, screen back on
    pub gate: KeyGate,                                              // screen_on, locked, when_locked
}
pub struct Requests { pub wake: bool, pub screen_off: bool, pub power_menu: bool, pub power_off: bool, pub lock_notice: bool }
```

Firmware wiring:

* **Input task**: debounced ladder/PMU edges → `KeyMapper::press/release(key,
  now)`; drain `poll(now)` into the UI channel; sleep until the next edge or
  `next_deadline()`. Set `mapper.rotated` from Display settings.
* **UI task**: on a key, `shell.key(...)`; on a service message,
  `shell.event(...)`; every `TICK_MS` (33 ms) while `shell.wants_ticks()`,
  `shell.tick(...)`. Act on `Requests` (panel off, power menu, wake). Then
  `shell.draw(&mut svc, &mut Gfx::new(fb, &mut dirty, theme.palette(accent),
  Some(Fallback { cache, src })))` and hand `dirty.take().rects()` (at most four
  rectangles, or `bounds()`) to the display driver for a DMA blit.
* The frame buffer is `&mut [u16]` of 128 × 128 native-endian RGB565 pixels
  (the host screenshot tools read it little-endian); any byte swap the LCD
  interface needs belongs to the display driver.
* The fallback source on the device reads `base + cp × 33` from the NOR
  resource synchronously (02 §7).

Key grammar as implemented (05 §3): a *tap* is `Up { held: false }`; screens
act on taps for OK, Back, Prev/Next, and on `Down`/`Repeat` for Up/Down. The
shell consumes Vol± (volume and overlay, also with the screen off), Power
(tap → `screen_off`, 1.5 s → `power_menu`, 4 s → `power_off`), Menu (tap →
context menu from `Screen::menu`, hold → `ScreenSet::QUICK_PANEL`) and
Back-hold (→ Home). Repeat runs at 8/s from the 500 ms hold, ramps to 30/s
between 1.5 s and 2.5 s held. Screens open dialogs and toasts through
`cx.ui.dialog(DialogSpec::destructive(...))` / `cx.ui.toast(..)`; answers come
back as `Event::Ui(UiEvent::Dialog { .. })` / `UiEvent::Menu { .. }`.

## RAM (thumbv7em-none-eabihf, measured with `size_of`)

| Item | Bytes | Notes |
|---|---:|---|
| `Shell<T, S>` excluding the top screen | 1 400 | nav stack 548 (8 × 68), open menu sheet 360, dialog 144, toast 64, volume 16, requests 192, bookkeeping |
| `Shell<AppScreen, _>` with the largest `yoton-apps` screen | ≈ 2 044 | under the 2 KiB of 05 §5.1 |
| `KeyMapper` | 312 | 10 key states + 16-event queue |
| `KeyGate` | 6 | inside `Shell` |
| `GlyphCache` | 2 252 | 64 fallback glyphs (02 §7: 2.2 KiB) |
| `ListView` | 48 | per list on a screen |
| `DirtySet` | 34 | per frame |
| Frame buffer | 32 768 | SRAM1, owned by the firmware |
| **Toolkit total** | **≈ 3 960** | + the top screen's own state; no heap, nothing grows with stack depth |

Text drawing streams glyphs straight from rodata (or from one 33-byte copy of a
cached fallback record) into the frame buffer; text drawing keeps one
33-byte record copy on the stack (stack depth not yet measured on target). Possible saving if needed: the menu sheet
and the dialog are never open together and could share one union (−140 B).

## Rodata (paged from NOR)

| Font | Source | Size |
|---|---|---:|
| `fonts/ui12.yfnt` | DejaVu Sans Condensed 12 px, 2 bpp, 777 glyphs: Latin, Latin-1, Extended-A, Greek, Cyrillic (U+0400–04FF), punctuation, arrows, ▲▶▼◀ triangles, symbols | 17 794 |
| `fonts/ui12b.yfnt` | DejaVu Sans Condensed Bold 12 px, 2 bpp, 521 glyphs (Latin-1, Extended-A, Greek, basic Cyrillic) | 12 466 |
| `fonts/num28.yfnt` | Fira Sans Condensed, 28 px digits, 4 bpp, tabular; `0-9 : . - + % / ,` and space | 3 551 |
| `fonts/num16.yfnt` | Fira Sans Condensed Medium, 16 px digits, 4 bpp, tabular | 1 253 |

Code size is measured by `xtask size` once the firmware links the toolkit.

## Fonts: decisions and licences

* **UI face: DejaVu Sans Condensed 12 px** (cap height 9, descent 3; a 12 px
  cap-to-descender box as 05 §4 asks). Chosen from DejaVu Sans/Condensed,
  Liberation Sans, Droid Sans, FreeSans, Noto Sans and Fira Sans rendered side
  by side at 11–13 px in the simulator: DejaVu's hinting gives the cleanest
  stems at this size and the condensed width fits more of a title in 128 px.
  **Deviation from 02 §7** ("BDF … 1-bit rows"): glyphs are 2 bits per pixel
  (4-level anti-aliasing, blended against the frame buffer). It looked
  clearly better than 1-bit hinted output on dark and light backgrounds and
  costs about 6 KiB more NOR; the format also supports 1 bpp
  (`fontgen.py --bpp 1`) if the panel or the budget demands it.
* **Bold**: the same family, for titles, the clock and focused dialog buttons.
* **Numerals: Fira Sans Condensed** (Regular for 28 px, Medium for 16 px), 4
  bpp, digits forced to one advance so clocks do not jitter.
* **Fallback (CJK, Hangul, kana, Thai, Hebrew, Arabic …)**: the 16 × 16,
  33-byte-record format of 02 §2.4, read through `GlyphSource`. For golden
  images and the full-flash font pack the glyphs come from **GNU Unifont
  17.0.05**; the simulator commits only the glyphs its sample text uses
  (`sim/assets/unifont-subset.hex`, regenerated by
  `sim/assets/make_subset.py`).

Licences (texts next to the data):

| Data | Licence | File |
|---|---|---|
| DejaVu Sans (Condensed, Bold) | Bitstream Vera Fonts licence + Arev licence, DejaVu changes public domain — permissive; derivatives may not be called "Bitstream" or "Vera" | `fonts/LICENSE-DejaVu.txt` |
| Fira Sans Condensed | SIL Open Font License 1.1, Reserved Font Name "Fira" (our bitmap derivatives are named `num28`/`num16`, not Fira) | `fonts/LICENSE-Fira.txt` |
| GNU Unifont (subset in `sim/assets`) | dual SIL OFL 1.1 / GPL-2.0+ with font embedding exception; **used under OFL 1.1** | `sim/assets/OFL-unifont.txt` |

All compatible with the project's MIT OR Apache-2.0 code (fonts are data under
their own licences). Nothing from the vendor's `UNICODE.FON` is committed.

Regenerating: `crates/yoton-ui/fontgen/regen.sh` (Python 3 with Pillow and
fontTools; FreeType renders with the font's own hinting, so results depend on
the installed FreeType and on Fedora's `dejavu-sans-fonts` 2.37 and
`mozilla-fira-sans-fonts` 4.202). It should become an `xtask fonts` command.

## The vendor font (verification for 02 §2.4)

In the descrambled backup (`yoton/dumps/nor_descrambled_8m.bin`) the vendor
firmware's directory entry `UNICODE FON` sits at `0xFC00` (32-byte entries:
8.3 name, u32 offset in 512-byte sectors `0x0B1C`, u32 length `0x210000` =
2 162 688). The glyph table starts at NOR offset **`0x172C00`** (= `0xF400` +
`0x0B1C` × 512, so directory offsets are relative to `0xF400`). Records are 33
bytes, direct-indexed by BMP code point: 16 rows × 2 bytes, MSB = leftmost
pixel, then one advance byte (`A` = 9, `Ж` = 11, `中`/`あ` = 16). Baseline:
row 13. The simulator can draw with it for development
(`YOTON_FALLBACK_FON=<dump>@0x172c00 cargo run -p yoton-sim -- …`); it is never
used for golden images. Whether the NOR descrambler key stream allows reading
it in place at random offsets (U-NOR-3) needs the player and is not checked.

## Deviations from 05 §5 and open points

* `KeyPhase::Long` added (Power 4 s); `Event::Ui(UiEvent)` added for menu and
  dialog results; `Cx` is a struct over a generic service type `S` instead of
  a fixed set of service traits (the traits arrive with `yoton-apps`).
* `ServiceEvent` has a first set of variants (`SettingChanged`, `Minute`
  added in UI-2) plus `Other { kind, value }`.
* Strings: the `tr!` table lives in `yoton-apps` (`strings.rs`). Toolkit
  code draws no literals except the ellipsis; the demo screens in `sim/` draw
  sample text directly.
* Key lock (UI-2): with the keys locked, any non-allowed key gives
  `Gate::LockNotice` (screen on or off); `Gate::Wake` is only for a screen
  that went dark without locking.
* Added in UI-2: `Icon2` + `Gfx::icon2` (2-bit icons), `widgets::misc::
  draw_slider`, `Item::wide_value` (information rows), `UiRequests::toast_for`
  and `invalidate_all`, `StrBuf::push_str_or_ellipsis`, and line-start rules
  in `Gfx::wrap` (no line begins with `。`, `、`, `）`, small kana …).
* Added in UI-3/UI-4: `ListSource::seek_letter` (letter jump answered by
  the source, so windowed lists are never scanned), accelerated held
  Up/Down in `ListView` (1, then 5, then 20 rows per repeat: about 10 s
  across 5000 rows), 1-bit `TEXT`, `CHIP`, `KEY` row icons.
* Screen-change slide animation belongs to UI-7.
