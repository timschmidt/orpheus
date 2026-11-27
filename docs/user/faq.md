# FAQ

## What is Orpheus?

A replacement firmware for the Yoton YM03, a clip-on MP3 player with a
128 × 128 colour screen, a microSD slot, an FM tuner and a Bluetooth
transmitter. It is written in Rust from scratch and is open source (MIT or
Apache-2.0).

## Is it finished?

No. Music, Files, Now Playing, MIDI files, Pictures, Radio, Bluetooth
audio, the games, Books and Settings run on the player every day; the
Recorder, Steps, Stopwatch, the Authenticator, USB Drive and the screen
reader run there but wait for a person's check of some functions. The
[manual's app table](README.md#apps) says which is which. Playlists, the
Bluetooth presenter remote, a USB sound card and reading books aloud are
planned. Nothing has been released yet. [STATUS.md](../STATUS.md) is the
detailed log, and the [changelog](../../CHANGELOG.md) summarises it.

## Can I try it without a player?

Yes: the website's [Try it](https://timschmidt.github.io/orpheus/try.html)
page runs the same screens, menus, file system, library, decoders and sound
settings in the browser, with a demo card and room for your own files
(they stay in your browser). Developers can also run the desktop simulator
([getting started](../dev/getting-started.md)).

## Will it damage my player?

Installing firmware always carries some risk, but the YM03's recovery mode
lives in read-only memory and cannot be overwritten, and Orpheus never
writes the chip's boot record or the original firmware's data area during
normal use. The tools refuse to write without a backup. See
[troubleshooting](troubleshooting.md).

## Can I go back to the original firmware?

Yes, from the backup made before installing:
[restore](install.md#restore-the-original-firmware).

## Does it touch my music?

Orpheus reads your card and writes only inside one folder, `/.orpheus/`
(the library, sorted-folder caches, cover thumbnails, the queue and resume
point, saved games, prepared books), and the Recorder's recordings in
`/Recordings`. Delete `/.orpheus/` at any time; the player rebuilds what
it needs (saved games and bookmarks are lost).

## Which audio formats play?

MP3, FLAC, WAV, Ogg Vorbis, Opus and Speex, and MIDI files (`.mid`,
`.midi`, `.kar`, `.rmi`), which the player's own synthesiser plays with its own set of
instruments. AAC, ALAC, WavPack and APE are planned.

## Does the battery last as long as with the original firmware?

That is a goal and is being measured. Orpheus runs the processor at
1.0 V instead of the chip's default 1.2 V and slows it down when it has
nothing to do. [Power mode](power.md) explains the choices.

## Can the player receive Bluetooth audio from my phone?

No. The Bluetooth module in the YM03 can only send audio.

## Why are some apps missing from Home?

Settings › Display › Home screen chooses which apps Home shows. A player
set up before Books existed keeps its old choice, so Books stays hidden
until you switch it on there. The [Sequencer](sequencer.md) is not on the player
yet.

## Can the player read the screen aloud?

Yes: hold **Back + Menu** for 2 seconds to switch the
[screen reader](screen-reader.md) on or off. Its voice is English only.

## Which languages are there?

The menus are in English. Russian and Japanese translations exist and are
**in progress** on the player. File names and tags in Latin, Greek and
Cyrillic script show as they are; Chinese, Japanese, Korean and other
scripts need a larger font that is **in progress**.

## How do I report a problem?

See [CONTRIBUTING.md](../../CONTRIBUTING.md#reporting-a-bug). For security
problems, [SECURITY.md](../../SECURITY.md).
