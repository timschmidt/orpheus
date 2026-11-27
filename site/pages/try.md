# Try Orpheus

This is Orpheus running in your browser: the same screens, menus, file
system, music library, decoders and sound settings that run on the player,
compiled to WebAssembly. Only the drivers are replaced by the simulator's.
Click the keys or use your keyboard; hold a key for a long press.

<link rel="stylesheet" href="try/try.css">
<div class="try" id="try" data-state="loading">
<div class="try-player" id="try-player" tabindex="0" aria-label="The player. Keys: arrows, Enter, Backspace, M, plus, minus, P." role="application">
<svg class="try-body" id="try-svg" viewBox="-8 -10 176 340" aria-hidden="true">
<defs><linearGradient id="try-shell" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#3a3f4a"/><stop offset="1" stop-color="#1c1f26"/></linearGradient></defs>
<rect class="try-key try-cap" data-key="power" x="104" y="-7" width="30" height="8" rx="2"><title>Power (P). Hold 1.5 s: power menu</title></rect>
<rect class="try-key try-cap" data-key="vol+" x="158" y="40" width="9" height="26" rx="2"><title>Vol+ (+)</title></rect>
<rect class="try-key try-cap" data-key="vol-" x="158" y="72" width="9" height="26" rx="2"><title>Vol− (−)</title></rect>
<rect x="0" y="0" width="160" height="320" rx="20" fill="url(#try-shell)" stroke="#555b66" stroke-width="1"/>
<rect x="10" y="14" width="140" height="140" rx="6" fill="#07080a"/>
<g class="try-key" data-key="back"><rect class="try-cap" x="14" y="166" width="38" height="16" rx="8"/><text x="33" y="177.5">BACK</text><title>Back (Backspace). Hold: Home</title></g>
<g class="try-key" data-key="menu"><rect class="try-cap" x="108" y="166" width="38" height="16" rx="8"/><text x="127" y="177.5">MENU</text><title>Menu (M). Hold: quick settings</title></g>
<g class="try-key" data-key="up"><path class="try-cap" d="M45.9 201.4A50 50 0 0 1 114.1 201.4L94.3 222.6A21 21 0 0 0 65.7 222.6Z"/><path class="try-glyph" d="M80 197l-6 9h12z"/><title>Up (↑)</title></g>
<g class="try-key" data-key="next"><path class="try-cap" d="M116.6 203.9A50 50 0 0 1 116.6 272.1L95.4 252.3A21 21 0 0 0 95.4 223.7Z"/><path class="try-glyph" d="M107 233v10l8-5zM115 233v10l8-5z"/><title>Next (→). Hold: seek, letter jump</title></g>
<g class="try-key" data-key="down"><path class="try-cap" d="M114.1 274.6A50 50 0 0 1 45.9 274.6L65.7 253.4A21 21 0 0 0 94.3 253.4Z"/><path class="try-glyph" d="M80 279l-6-9h12z"/><title>Down (↓)</title></g>
<g class="try-key" data-key="prev"><path class="try-cap" d="M43.4 272.1A50 50 0 0 1 43.4 203.9L64.6 223.7A21 21 0 0 0 64.6 252.3Z"/><path class="try-glyph" d="M53 233v10l-8-5zM45 233v10l-8-5z"/><title>Prev (←). Hold: seek back, letter jump</title></g>
<g class="try-key" data-key="ok"><circle class="try-cap" cx="80" cy="238" r="18"/><path class="try-glyph" d="M73 231v14l9-7zM84 231h2.5v14h-2.5zM88.5 231h2.5v14h-2.5z"/><title>OK (Enter). Hold in Now Playing: stop</title></g>
<text class="try-brand" x="80" y="306">ORPHEUS</text>
</svg>
<canvas class="try-screen" id="try-screen" width="128" height="128" aria-label="The player's screen"></canvas>
<div class="try-overlay" id="try-overlay"><p id="try-status">Loading Orpheus…</p></div>
</div>
<div class="try-side">
<p class="try-keys"><b>Keys:</b> <kbd>↑</kbd> <kbd>↓</kbd> Up, Down · <kbd>←</kbd> <kbd>→</kbd> Prev, Next · <kbd>Enter</kbd> OK · <kbd>Backspace</kbd> Back · <kbd>M</kbd> Menu · <kbd>+</kbd> <kbd>−</kbd> volume · <kbd>P</kbd> Power. Hold a key for a long press: hold Back for Home, Menu for quick settings.</p>
<p class="try-sound" id="try-sound">Sound starts with your first key press.</p>
<div class="try-drop" id="try-drop"><p><b>Your files:</b> drop music, pictures or text onto the player, or <label class="try-pick">choose files<input type="file" id="try-files" multiple hidden></label>. They go into the folder <i>Your files</i> on the virtual card. They stay in this browser (IndexedDB) and are <b>never uploaded</b>: nothing you add leaves your computer.</p><ul id="try-yours"></ul></div>
<p class="try-buttons"><button type="button" id="try-reset">Factory reset</button> <button type="button" id="try-dev-toggle" aria-expanded="false" aria-controls="try-dev">Developer panel</button></p>
<p class="try-note">Settings, the resume point, the library and your files are kept in this browser. Factory reset deletes all of it.</p>
<section class="try-dev" id="try-dev" hidden aria-label="Developer panel">
<dl class="try-stats" id="try-stats"></dl>
<p><button type="button" id="try-shot">Screenshot</button> <button type="button" id="try-rec">Record keys</button> <label class="try-pick">Replay a sequence<input type="file" id="try-replay" hidden></label></p>
<p class="try-note" id="try-rec-note">Recordings use the format of the <code>ui_keys</code> fuzz corpus: put one into <code>tests/regress/ui_keys/</code> and every <code>cargo test</code> replays it; replay a corpus input here to watch it.</p>
</section>
</div>
</div>
<noscript><p>This page needs JavaScript and WebAssembly.</p></noscript>
<script type="module" src="try/app.js"></script>

## What is on the card

A FAT32 card image built in your browser's memory, with real works in the
public domain or under free licences, one for every kind of file the
player opens: Bach's Goldberg Variations played by Kimiko Ishizaka (MP3,
FLAC), the opening of Beethoven's Fifth (Ogg Vorbis), Caruso's 1907
*Vesti la giubba* with its words (Opus, LRC), a Chopin nocturne (AIFF),
Satie's first Gymnopédie (WAV), Sousa's Band on a 1909 cylinder (Sun AU),
a LibriVox reading of Orpheus and Eurydice (Speex), a Bach prelude for the
synthesiser (MIDI), Corot, Hokusai, Muybridge, a Hubble picture of the
Ring Nebula in Lyra and NASA's Earthrise (JPEG, PNG, BMP, GIF, QOI), and
two Project Gutenberg books (EPUB, plain text). The recordings are short
excerpts. Every file's author, licence, source and changes are in
`CREDITS.txt` on the card and in the
[demo card credits](https://github.com/timschmidt/orpheus/blob/main/sim/web/demo/CREDITS.md);
nothing on it was generated. The card's files are downloaded once
(about 3.4 MB) next to the page's code.

The code of this page includes the Opus and Speex decoder ports and the
glyph data listed in [third-party material](https://github.com/timschmidt/orpheus/blob/main/THIRD-PARTY.md), under their
own permissive licences. The page loads nothing from other sites and has no
trackers.

## What is real and what is simulated

Real: the apps and screens, the key handling, the FAT file system, the music
library and its index, the tag and lyrics readers, the picture decoders, the
audio decoders and the DSP chain (equaliser, bass, balance, crossfeed,
channels). The volume curve is the player's.

Simulated: battery, charger, clock, Bluetooth, the FM tuner and the step
counter. The player's drivers, its paged memory and its boot stages do not
run here; the hardware tests cover those.

## The desktop simulator

The same apps run in a window on your computer. With
[Rust](https://rustup.rs) installed:

```sh
git clone https://github.com/timschmidt/orpheus && cd orpheus
cargo run -p orpheus-sim --features window -- --window
```

More in [getting started](dev/getting-started.html#4-the-simulator),
which also shows how this page is built (`cargo xtask wasm`).
