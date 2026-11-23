//! IMA and Microsoft ADPCM, written from the format descriptions (IMA
//! Digital Audio Focus and Technical Working Groups, "Recommended Practices
//! for Enhancing Digital Audio Compatibility in Multimedia Systems" 3.00;
//! Microsoft "Multimedia Standards Update" 1994). The arithmetic follows
//! ffmpeg's decoders exactly (multiplication form of the IMA step, C
//! truncating division in MS prediction) so the output is bit-identical to
//! the reference decodes.

pub(crate) const STEP: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88,
    97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658,
    724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660,
    4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818,
    18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

const INDEX: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Ima {
    pub predictor: i32,
    pub index: i32,
}

impl Ima {
    /// One nibble, WAV form: `diff = ((2·delta + 1) · step) >> 3`.
    #[inline]
    pub(crate) fn expand(&mut self, nibble: u8) -> i16 {
        let step = STEP[self.index as usize];
        self.index = (self.index + INDEX[usize::from(nibble & 15)]).clamp(0, 88);
        let delta = i32::from(nibble & 7);
        let diff = ((2 * delta + 1) * step) >> 3;
        let p = if nibble & 8 != 0 { self.predictor - diff } else { self.predictor + diff };
        self.predictor = p.clamp(-32768, 32767);
        self.predictor as i16
    }

    /// One nibble, QuickTime form (shift-and-add, as in the reference
    /// implementation).
    #[inline]
    pub(crate) fn expand_qt(&mut self, nibble: u8) -> i16 {
        let step = STEP[self.index as usize];
        self.index = (self.index + INDEX[usize::from(nibble & 15)]).clamp(0, 88);
        let mut diff = step >> 3;
        if nibble & 4 != 0 {
            diff += step;
        }
        if nibble & 2 != 0 {
            diff += step >> 1;
        }
        if nibble & 1 != 0 {
            diff += step >> 2;
        }
        let p = if nibble & 8 != 0 { self.predictor - diff } else { self.predictor + diff };
        self.predictor = p.clamp(-32768, 32767);
        self.predictor as i16
    }

    /// Start a QuickTime packet from its 2-byte header. The previous
    /// predictor is kept when the header only rounds it (ffmpeg behaviour).
    pub(crate) fn qt_header(&mut self, hdr: u16) -> bool {
        let h = i32::from(hdr as i16);
        let index = h & 0x7F;
        let pred = h & !0x7F;
        if self.index != index || (pred - self.predictor).abs() > 0x7F {
            self.index = index;
            self.predictor = pred;
        }
        self.index <= 88
    }
}

const ADAPT: [i32; 16] = [230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230];
const COEF1: [i32; 7] = [256, 512, 0, 192, 240, 460, 392];
const COEF2: [i32; 7] = [0, -256, 0, 64, 0, -208, -232];

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Ms {
    pub c1: i32,
    pub c2: i32,
    pub idelta: i32,
    pub s1: i32,
    pub s2: i32,
}

impl Ms {
    /// Set the predictor; `false` if the index is invalid.
    pub(crate) fn predictor(&mut self, i: u8) -> bool {
        let i = usize::from(i);
        if i >= COEF1.len() {
            return false;
        }
        self.c1 = COEF1[i];
        self.c2 = COEF2[i];
        true
    }

    #[inline]
    pub(crate) fn expand(&mut self, nibble: u8) -> i16 {
        // C division truncates towards zero; so does Rust's.
        let mut p = (self.s1 * self.c1 + self.s2 * self.c2) / 256;
        let signed = if nibble & 8 != 0 { i32::from(nibble) - 16 } else { i32::from(nibble) };
        p += signed * self.idelta;
        self.s2 = self.s1;
        self.s1 = p.clamp(-32768, 32767);
        self.idelta = ((ADAPT[usize::from(nibble & 15)] * self.idelta) >> 8).clamp(16, i32::MAX / 768);
        self.s1 as i16
    }
}
