# Troubleshooting and recovery

The YM03 is hard to break: its chip has a USB recovery mode (**ADFU**) in
read-only memory, and Orpheus is built so that it can always get there.

## The player restarts by itself

Orpheus watches itself. If it hangs for a few seconds, the watchdog
restarts it. If it crashes, it shows a **crash screen** (what
went wrong and where, in hexadecimal) and restarts after 3 seconds or a
key press. The crash details stay in memory for the developers: if you can,
[report it](../../CONTRIBUTING.md#reporting-a-bug) with what you were doing
and, if you have the tools, the output of `cargo xtask dev results`.

## The player does not start

The boot stage counts starts that do not reach a healthy firmware (the
firmware clears the count after 30 seconds of running). After **three
failed starts in a row** it tries the other firmware slots, and if none of
them works it stops in **ADFU**: the screen stays dark and a computer sees
a USB device with Actions' vendor id `10d6`. From there the install tools
can put a working firmware back:

```sh
cargo xtask dev state                               # what runs on the player
cargo xtask dev stage target/orpheus/firmware.yfw   # write a good firmware
cargo xtask dev reset                               # start it
```

## Force recovery mode

When nothing else helps:

1. Unplug the player.
2. Press the reset pinhole next to the earphone jack with a paper clip.
3. Hold **Next** and plug in USB. The player starts in ADFU.

With Orpheus running, Settings › System › **Recovery mode** (or the power
menu) does the same without the pinhole.

## The screen is dark but music plays

The screen switched off and the keys locked (Power, or the backlight and
lock timers in Settings › Display). Press **Power** to wake it.

## The tools do not find the player

Settings › USB › **Developer channel** off hides the connection the
install and developer tools use. Switch it on again there (it takes effect
at once), or use Settings › System › Factory reset. Recovery mode (above)
works either way.

## No sound in wired headphones

* Check the volume (Vol+) and Settings › Sound › Volume limit.
* Check that the plug is fully in.

## A file does not play

Orpheus plays MP3, FLAC, WAV, Ogg Vorbis, Opus, Speex and MIDI. Other
formats (AAC/M4A, ALAC, WavPack, APE, WMA) are **in progress** and do not
play yet. In [Battery saver](power.md) some files are too much for the
slow processor and stutter or stop: switch Power mode to Automatic.

A heavy MIDI file can sound thinner: at 288 MHz the player leaves out
effects and some notes rather than stop ([MIDI files](music.md#midi-files)).

## Pictures say "Stop playback to view pictures"

Pictures and music share memory: stop the music (hold OK on Now Playing)
and the picture appears.

## Books is not on Home

A player set up before Books existed keeps its old Home choice: switch
Books on in Settings › Display › Home screen.

## A book does not open while music plays

An EPUB is converted the first time it opens, which needs the memory
music uses ("Stop the music to prepare this book."): stop the music once.
Opening a book during music can cause a short dropout, and once showed
"The book could not be read.": open it again (known issues).

## The clock shows 00:00

The clock sometimes stops after a restart (known issue). Set it in
Settings › Date & time.

## Going back to the original firmware

`cargo xtask dev restore vendor --i-have-a-backup` writes the saved copy
of the original flash back. See [install](install.md#restore-the-original-firmware).

## Still stuck?

Wait two minutes (the restart counter needs that long at most), then
[open an issue](https://github.com/timschmidt/orpheus/issues) with what
the screen shows and what `cargo xtask dev state` prints.
