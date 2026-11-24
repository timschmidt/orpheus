# 09 — Documentation, website and branding

What the project publishes and how it looks: the documentation set, the
project website on GitHub Pages with an in-browser Orpheus and a browser
installer, and the Orpheus/lyre identity shared by firmware, documentation and
website. Work packages: 08 M12. Owner's requirement of 2026-11-24.

## 1. Goals

* A visitor understands in one minute what Orpheus is, what works today, and
  how to try it (in the browser) or install it (on a YM03).
* A user can install, update and restore the stock firmware without a
  terminal where the browser allows it, and with one documented command
  otherwise.
* A developer goes from a fresh clone to a running simulator in under 15
  minutes, and to a first change with a passing gate in under an hour, by
  following the documentation alone.
* Every document is short, current and has one job; nothing is said twice.
* Firmware, documentation and website are visibly one project.

## 2. Documentation set

| Document | Audience | Content | Length target |
|---|---|---|---|
| `README.md` | everyone | Logo, one-paragraph pitch, three screenshots (from the goldens), status line, **Try it** (website link), **Install** (browser / command line), **Build** (four commands), links to the rest | ≤ 1 screen of text plus images |
| `docs/user/` (manual, rendered on the site) | users | Install, update, restore; key grammar; one page per app (what it does, keys, settings, limits); troubleshooting and recovery (ADFU, boot counter, restore vendor); FAQ | one page per topic, ≤ 600 words each |
| `docs/dev/getting-started.md` | new developers | Prerequisites (pinned toolchain, udev rule), clone, `cargo xtask` tour, simulator, goldens, gate, first change walkthrough, working with the player safely (CLAUDE.md rules in human words) | tested in a clean container (§8) |
| `docs/dev/architecture.md` | developers | One-page overview with diagrams (memory map, boot chain, pager, executors, crates and their dependency graph), linking to `docs/plan/02…` for depth | ≤ 2 pages |
| `docs/dev/how-to/*.md` | developers | Add a screen, a setting, a codec, a driver, a test command, a fuzz target; profile; read a fault record | ≤ 1 page each |
| `docs/plan/*`, `docs/STATUS.md` | maintainers | The plan and the log (kept; linked, not duplicated) | — |
| `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md` (Contributor Covenant), `SECURITY.md` (how to report; the authenticator's stated limits), `CHANGELOG.md` (Keep a Changelog, per release), `LICENSE*` (decision D-1), `.github/` issue and PR templates | contributors | Standard open-source files | — |
| rustdoc for every crate, published on the site | developers | `#![warn(missing_docs)]` on every public item; crate-level docs with an example; doc tests run in the gate | 100 % public items documented |
| Hardware notes (`docs/hw/`) | porters | Published only per decision D-7 | — |

Rules:

* Plain, concise English (the style already used in the plan); present tense;
  numbers with units; no marketing words in technical pages.
* Each fact lives in one place; other pages link to it.
* Screenshots in documentation are generated from the golden tests
  (`xtask site shots`), so they never go stale; the gate fails if a manual
  page references a screenshot that no longer exists.
* Translations of the user manual follow the firmware's languages (en, ru, ja
  at first) once the English text is stable.

## 3. Website (GitHub Pages, built from the repository)

* **Source** in `site/` (static pages, CSS, small vanilla JavaScript; no
  heavyweight framework), content from `docs/user/`, `docs/dev/`, rustdoc,
  release metadata and generated screenshots. Built by `cargo xtask site`
  (Markdown → HTML with a Rust static generator, e.g. a `pulldown-cmark`
  based tool in `xtask`, or Zola if the owner prefers) into
  `target/orpheus/site/`.
* **Publishing**: a GitHub Actions workflow builds the site, the WASM
  simulator and rustdoc on every push to `main` and deploys to GitHub Pages
  (`actions/deploy-pages`); pull requests get a build check and an artefact
  preview. Releases attach the firmware files and checksums, and the site
  reads the release list for its download and installer pages.
* **Pages**:
  * **Home**: lyre logo, one-line pitch, an animated strip of real screens,
    "Try it in your browser" and "Install on your YM03" buttons, feature
    grid (playback formats, library, DSP, pictures, radio, recorder,
    Bluetooth and the presenter remote, authenticator, USB disk/CD), status
    by milestone, battery-life table (PWR-4), links to source and docs.
  * **Try it**: the WASM Orpheus (§4).
  * **Install**: the browser installer (§5) with the command-line path
    beside it, recovery and restore instructions.
  * **Manual**, **Developers** (getting started, architecture, how-tos,
    rustdoc), **Hardware** (the YM03, what Orpheus uses of it),
    **Changelog**, **FAQ**.
* **Quality bar**: responsive (phone to desktop), light and dark themes,
  keyboard-navigable, WCAG 2.2 AA contrast, Lighthouse ≥ 95 for performance,
  accessibility, best practices and SEO on Home; no third-party trackers or
  fonts loaded from other origins (privacy: everything self-hosted); total
  Home weight ≤ 500 KB before the simulator is opened; Open Graph/social
  card with the lyre.
* **Tests**: link checker (internal and external, weekly for external),
  HTML validation, Lighthouse CI on the Home and Try-it pages, a Playwright
  smoke test that loads the simulator, presses keys and compares the canvas
  with the golden PNG.

## 4. Orpheus in the browser (WASM)

What runs: the **real UI, app, service, codec, DSP, library, index, file
system and picture code** — the same crates the firmware links — compiled to
`wasm32-unknown-unknown`, with the simulator's back ends replacing the HAL.
That is the honest meaning of "the firmware in the browser": everything
above the hardware layer is the shipping code; drivers, pager and boot stages
are not (they are covered by HIL tests instead). Emulating the Cortex-M4 and
its peripherals in the browser was considered and rejected: large, slow, and
it would mostly test a peripheral model the project would also have to write.

* **Screen**: 128 × 128 canvas scaled by integer factors with
  `image-rendering: pixelated`, inside an SVG drawing of the player whose
  keys are clickable/touchable; physical keyboard mapping shown on hover
  (arrows, Enter, Backspace, M, +/−, P), with hold for long-press.
* **Sound**: decoded PCM through an AudioWorklet; the DSP chain and volume
  as on the player.
* **Card**: a virtual FAT32 image in memory, pre-filled with a small set of
  freely licensed music (CC0/CC-BY with attribution on the page), pictures
  and a text file; visitors can **drop their own files** onto the player —
  they stay in the browser (IndexedDB), are never uploaded, and the page says
  so.
* **State**: settings, resume point and library persist in IndexedDB; a
  "Factory reset" button clears them.
* **Developer extras** (behind a toggle): frame-time and memory panel,
  key-sequence recorder/replayer (the same format as the on-device fuzz
  corpus, 07 §5.7, so a sequence found in the browser can be replayed on the
  player), screenshot button.
* **Budgets**: WASM + data ≤ 4 MB compressed, first screen within 2 s on a
  mid-range laptop, steady 60 fps canvas updates, audio without dropouts in
  Chromium, Firefox and Safari current versions.
* **Build**: `cargo xtask wasm` (wasm-bindgen + wasm-opt; the target and tool
  versions pinned in the toolchain file); size tracked in `budgets.toml` and
  in the metrics history (07 §4.4).

## 5. Installing from the browser

Where supported (WebUSB: Chromium-based desktop browsers — Chrome, Edge,
Opera, Brave — and Chrome on Android; not Firefox or Safari), the Install
page installs, updates and restores firmware with the same code as `xtask`:
the `orpheus-host` crate (ADFU, ymon, NOR payload protocol, developer
channel) compiled to WASM over a WebUSB transport trait. Elsewhere it shows
the command-line path and offers the release files for download.

* **Safety rules are the same as for the tools** (CLAUDE.md, 07 §7): the
  installer first reads the whole NOR and **saves a backup** (download to the
  visitor's disk and a copy in IndexedDB), verifies the read twice, and
  refuses to continue without it; it never writes sector 0 except in the
  first-install step that the visitor explicitly confirms, and never the
  vendor data at `0x7E0000` and above; every image is checked against the
  release manifest's SHA-256 before anything is written; progress survives a
  tab reload by resuming from the last verified sector.
* **Flows**: *First install* (from the stock firmware: how to enter ADFU,
  backup, install, verify, reboot); *Update* (from a running Orpheus: through
  the developer channel to the inactive slot, then the firmware's own update
  path UPD-1 switches slots — no ADFU needed); *Restore stock firmware* (from
  the saved backup); *Recovery* (boot counter tripped, ADFU).
* **Device access**: the firmware's developer-channel interface carries
  WebUSB and Microsoft OS 2.0 descriptors so it works without drivers on
  Windows, macOS, Linux (with the udev rule) and Android; the ROM's ADFU mode
  cannot, so on Windows the first install needs a one-time WinUSB driver
  step that the page explains (or the command-line tool).
* **USB identity**: needs the project's own vendor/product id (decision D-3)
  before public release.
* **Tests**: the WebUSB transport runs the host protocol test suite against
  a mock device in the browser test runner; a HIL test drives a real
  install/update/restore through Chromium (Playwright with a WebUSB
  permission pre-grant) on the reference player.

## 6. Branding (Orpheus and the lyre)

One brand source, `docs/brand/`, used by everything:

* **Logo**: the lyre (`docs/logo/orpheus-lyre.svg`) in full, mono and
  1-bit/small-size variants; the 128 × 128 boot logo and the 16/24/32 px
  firmware icons are generated from the same SVG (`xtask brand`), as are the
  favicon set, app-touch icons and the 1200 × 630 social card.
* **Name**: "Orpheus" in text; `orpheus` for crates, folders and identifiers.
* **Colour**: a small palette (primary/accent, neutrals, status colours)
  defined once in `docs/brand/palette.toml` and generated into the firmware
  theme tables, the website CSS custom properties and the documentation
  stylesheet; contrast checked for both the 128 × 128 RGB565 screen and the
  web (WCAG AA).
* **Type**: the firmware's own bitmap font family is the visual reference for
  the website's headings (rendered as a web font subset), with a system UI
  font for body text.
* **Voice**: concise, friendly, plain (§2 rules).
* `docs/brand/README.md`: usage rules (clear space, minimum sizes, do and
  don't), with the generated assets; the gate checks that generated assets
  are up to date.

## 7. Open decisions this depends on

* D-1 licence (blocks publishing the site and releases).
* D-3 USB identity (blocks public browser installs).
* D-7 publishing the hardware notes (decides what the Hardware page links).
* D-11 public repository name and the Pages address (`<owner>.github.io/<repo>`
  or a custom domain), and when the site goes public.
* Demo content licences for the in-browser card (only CC0/CC-BY material
  with attribution).

## 8. Acceptance

* The getting-started guide is followed in a clean container (CI job) from
  clone to a passing `cargo xtask gate --host` and a simulator screenshot.
* Link check, HTML validation, Lighthouse thresholds, Playwright simulator
  test and the browser-installer mock suite pass in CI on every push.
* Rustdoc: no missing docs on public items; doc tests pass.
* A user check: a person who has not seen the project installs Orpheus with
  the browser installer on a YM03 from the Install page alone, then restores
  the stock firmware, and reports where they hesitated.
* The verification stage (M11) covers the WASM build, the site generator and
  the WebUSB transport like all other project code.
