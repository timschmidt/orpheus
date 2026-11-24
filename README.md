<p align="center"><img src="docs/logo/orpheus-lyre.svg" alt="Orpheus logo: a lyre" width="120"></p>

# Orpheus

A pure-Rust, [Embassy](https://embassy.dev)-based firmware for the Yoton YM03
MP3 player (Actions ATJ2157: Cortex-M4F, 320 KiB RAM, 8 MiB SPI NOR, microSD,
128 × 128 LCD, FM tuner, accelerometer, Bluetooth transmitter module).

**State (2026-11-24): running on the player** in the full-flash layout
(the stock firmware is replaced): demand-paged firmware with Home, Files
(sorted through an on-card index), Music (queue, resume, bookmarks, shuffle,
A–B), Now Playing, Pictures, Settings; MP3/FLAC/WAV/Vorbis/Speex playback
with the DSP chain; USB card reader; Bluetooth module control; power-off.
See `docs/STATUS.md` for details and open items. The plan was written on
2026-11-23 from the research and the Rockbox port in the companion repository
(`../yoton`, mirrored at https://github.com/timschmidt/yoton-rockbox).

## Start here

| Read | For |
|---|---|
| [docs/PLAN.md](docs/PLAN.md) | Scope, findings, decisions, open decisions for the owner, document map |
| [docs/plan/08-milestones.md](docs/plan/08-milestones.md) | Milestones and work packages, in order |
| [docs/STATUS.md](docs/STATUS.md) | What has been done so far |
| [CLAUDE.md](CLAUDE.md) | Rules for anyone (human or AI) working on the tree or touching the player |
| [docs/USER-CHECKS.md](docs/USER-CHECKS.md) | Checks only a person can make |

## Layout of the documentation

```
docs/
├── PLAN.md                 master plan
├── STATUS.md               living progress log
├── USER-CHECKS.md          checks that need eyes, ears or hands
├── plan/
│   ├── 01-hardware.md      hardware reference and unknowns register
│   ├── 02-architecture.md  memory, boot, demand paging, tasks, power, storage, crates
│   ├── 03-hal-drivers.md   driver specifications
│   ├── 04-audio.md         pipeline, codecs, DSP, recording, indexes, pictures
│   ├── 05-ui-apps.md       UI grammar, toolkit, every screen and app
│   ├── 06-connectivity.md  USB, disk/CD emulation, Bluetooth, authenticator
│   ├── 07-tooling-testing.md  host tools, simulator, on-device tests, safety rules
│   └── 08-milestones.md    work packages, dependencies, risks
├── hw/                     reverse-engineering reports (kept out of git by default)
├── research/               Rust ecosystem survey
└── vendor/                 datasheet text and SDK register header (local reference; out of git)
```

`docs/hw/` and `docs/vendor/` are listed in `.gitignore`, following the
companion repository's policy of not publishing analysis derived from vendor
firmware. Whether to publish `docs/hw/` is the owner's call (decision D-7 in
the plan); a fresh clone without those directories is missing reference
material the work packages cite.

## Licence

Not chosen yet (decision D-1 in the plan; default proposal MIT OR Apache-2.0).
