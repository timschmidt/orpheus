# FAQ

## What is Orpheus?

A replacement firmware for the Yoton YM03, a clip-on MP3 player with a
128 × 128 colour screen, a microSD slot, an FM tuner and a Bluetooth
transmitter. It is written in Rust from scratch and is open source (MIT or
Apache-2.0).

## Is it finished?

No. Music, Files, Now Playing, Pictures, Bluetooth audio and Settings run
on the player every day. Radio, Recorder, Steps, Stopwatch, the
authenticator, USB disk images, playlists and the Bluetooth presenter
remote are planned. [STATUS.md](../STATUS.md) is the detailed log, and the
[changelog](../../CHANGELOG.md) summarises it.

## Can I try it without a player?

Soon: a version that runs in the web browser, with the same screens and
the same music code, is **in progress**. Today the desktop simulator does
it for developers ([getting started](../dev/getting-started.md)).

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
point). Delete that folder at any time; the player rebuilds it.

## Which audio formats play?

MP3, FLAC, WAV, Ogg Vorbis, Opus and Speex. AAC, ALAC, WavPack and APE are
planned.

## Does the battery last as long as with the original firmware?

That is a goal and is being measured. Orpheus runs the processor at
1.0 V instead of the chip's default 1.2 V and slows it down when it has
nothing to do. [Performance](performance.md) explains the faster mode.

## Can the player receive Bluetooth audio from my phone?

No. The Bluetooth module in the YM03 can only send audio.

## Why are some apps missing from Home?

Home shows only the apps that work on the player today. The simulator and
the screenshots in this manual show all planned apps.

## Which languages are there?

The menus are in English. Russian and Japanese translations exist and are
**in progress** on the player. File names and tags in Latin, Greek and
Cyrillic script show as they are; Chinese, Japanese, Korean and other
scripts need a larger font that is **in progress**.

## How do I report a problem?

See [CONTRIBUTING.md](../../CONTRIBUTING.md#reporting-a-bug). For security
problems, [SECURITY.md](../../SECURITY.md).
