//! The demo content of the browser card (WEB-2): the tracks synthesised by
//! [`crate::synth`] and encoded by `cargo xtask wasm --demo` (committed in
//! `sim/web/demo/`), and texts and lyrics written for the demo. All of it
//! is the project's own work under the project's licence.

/// The committed encodings, in [`crate::synth::TRACKS`] order.
pub const ENCODED: [&[u8]; 4] = [
    include_bytes!("../demo/lyre.mp3"),
    include_bytes!("../demo/tide.ogg"),
    include_bytes!("../demo/bells.opus"),
    include_bytes!("../demo/pulse.flac"),
];

/// The night-sky picture ([`crate::synth::lyra`]) as a baseline JPEG.
pub const LYRA_JPG: &[u8] = include_bytes!("../demo/lyra.jpg");

/// Lyrics for the first track (written for the demo).
pub const LYRE_LRC: &str = "[00:00.00](lyre)\n\
[00:04.00]Strings of light in the morning air\n\
[00:09.00]a lyre plays where the river bends\n\
[00:14.00]every note a step back home\n\
[00:19.00]do not look back until the song ends\n\
[00:25.00](lyre)\n";

pub const ABOUT: &str = "Orpheus in your browser\n\
\n\
This card is a FAT32 image in your browser's memory. The music on it was \
synthesised for the demo by the Orpheus project: plucked strings (MP3), \
filtered noise (Ogg Vorbis), FM bells (Opus), a drum machine (FLAC) and \
test tones (WAV). The pictures were drawn by the project too.\n\
\n\
Drop your own files onto the player and they appear in the folder \
\"Your files\". They stay in this browser (IndexedDB) and are never \
uploaded anywhere.\n\
\n\
Everything you see is the firmware's own code: the screens, the file \
system, the music library, the decoders and the sound settings. Only the \
drivers are replaced by the simulator's.\n";

pub const MYTH: &str = "The myth of Orpheus\n\
\n\
Orpheus was the finest singer of the old stories. When he played his lyre, \
the rivers slowed to listen, the trees leaned closer and even the stones \
were said to move.\n\
\n\
His wife Eurydice died from the bite of a snake on the day of their \
wedding. Orpheus walked down into the underworld to bring her back. He \
sang to the ferryman, to the three-headed dog at the gate and at last to \
the king and queen of the dead, and for the first time the shadows wept.\n\
\n\
They let Eurydice go, on one condition: she would follow him up to the \
light, and he must not look back at her until they had both left the \
dark. Orpheus walked and played. He heard no footsteps behind him. At the \
very edge of the daylight, afraid that he had been deceived, he turned \
around, and saw her fall back into the shadows for ever.\n\
\n\
He sang of her for the rest of his life. When he died, the Muses \
gathered the pieces of his lyre and set it among the stars, where you can \
still find it: the small constellation Lyra, with the bright star Vega.\n\
\n\
A player named after him should at least know how to keep a song going \
without looking back.\n";
