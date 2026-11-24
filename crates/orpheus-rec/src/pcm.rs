//! Capture word layouts (U-AUD-12).
//!
//! The ADC FIFO holds 24-bit samples in bits 31:8 of a 32-bit word, bits
//! 7:0 zero (datasheet §8.2.6.7). The capture DMA either narrows each word
//! to 16 bits itself (`FIFO_DMAWIDTH` = 16-bit and DMA width 16, the
//! vendor's setting; which 16 bits it keeps is U-AUD-12) or copies whole
//! words ("32-bit capture"), which this module turns into 16-bit samples:
//! the top 16 bits of each word, in place.
//!
//! The capture ring is seen as `i16` by the recorder, so a 32-bit word is
//! two neighbouring `i16`s, low half first (the player and every host
//! this runs on are little-endian).

/// The 16-bit sample of one FIFO word (bits 31:16, the 24-bit sample's top
/// 16 bits; truncated).
#[must_use]
pub const fn word_to_i16(w: u32) -> i16 {
    (w >> 16) as u16 as i16
}

/// The 24-bit sample of one FIFO word, sign-extended.
#[must_use]
pub const fn word_to_i24(w: u32) -> i32 {
    (w as i32) >> 8
}

/// A block of 32-bit FIFO words (seen as `i16` pairs, low half first) into
/// 16-bit samples in place: sample `k` is word `k`'s high half. Returns the
/// samples (half of `buf`; an odd last `i16` is ignored).
pub fn unpack_24in32(buf: &mut [i16]) -> &mut [i16] {
    let n = buf.len() / 2;
    // Forward: sample k is written at index k ≤ 2k + 1, behind every word
    // still to be read.
    for k in 0..n {
        buf[k] = buf[2 * k + 1];
    }
    &mut buf[..n]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::{Chain, ChainConfig};
    use crate::fake::{FakeAdc, Signal};
    use crate::meter::{Meter, db};

    /// A FIFO word for a 24-bit sample (bits 7:0 zero).
    fn word(s24: i32) -> u32 {
        (s24 << 8) as u32
    }

    /// Words as the DMA leaves them in an `i16` ring: low half, high half.
    fn as_ring(words: &[u32], out: &mut [i16]) {
        for (w, pair) in words.iter().zip(out.chunks_exact_mut(2)) {
            pair[0] = (*w & 0xFFFF) as u16 as i16;
            pair[1] = (*w >> 16) as u16 as i16;
        }
    }

    #[test]
    fn word_layout() {
        assert_eq!(word_to_i24(word(0x12_3456)), 0x12_3456);
        assert_eq!(word_to_i24(word(-1)), -1);
        assert_eq!(word_to_i24(word(-0x80_0000)), -0x80_0000);
        assert_eq!(word_to_i16(word(0x12_3456)), 0x1234);
        assert_eq!(word_to_i16(word(-0x80_0000)), i16::MIN);
        assert_eq!(word_to_i16(word(0x7F_FFFF)), i16::MAX);
        // Small negative values truncate towards −∞ (−1 LSB of 16 bits).
        assert_eq!(word_to_i16(word(-1)), -1);
        assert_eq!(word_to_i16(word(0xFF)), 0);
    }

    #[test]
    fn unpack_in_place() {
        let s24 = [0x12_3456, -0x12_3456, 0x7F_FFFF, -0x80_0000, 0, -1, 0x100, 0x00_00FF];
        let words = s24.map(word);
        let mut ring = [0i16; 16];
        as_ring(&words, &mut ring);
        // The low halves hold the sample's low byte and zeros: what a
        // 16-bit read of the wrong half would record.
        assert_eq!(ring[0], 0x5600);
        let out = unpack_24in32(&mut ring);
        assert_eq!(out.len(), 8);
        let want = words.map(word_to_i16);
        assert_eq!(out, &want);
        // Odd length: the last i16 is not half a word.
        let mut odd = [0i16, 7, 99];
        assert_eq!(unpack_24in32(&mut odd), &[7]);
        assert!(unpack_24in32(&mut []).is_empty());
    }

    #[test]
    fn stereo_frames_stay_paired() {
        // L, R words alternate; their high halves alternate the same way.
        let words = [word(0x10_0000), word(-0x20_0000), word(0x30_0000), word(-0x40_0000)];
        let mut ring = [0i16; 8];
        as_ring(&words, &mut ring);
        assert_eq!(unpack_24in32(&mut ring), &[0x1000, -0x2000, 0x3000, -0x4000]);
    }

    /// A −20 dBFS tone through the 24-in-32 path reads −20 dBFS on the
    /// meter; the same words read by their low halves do not.
    #[test]
    fn meter_reads_unpacked_level() {
        let mut adc = FakeAdc::new(44_100, 1, Signal::Tone { hz: 1000, amp: 3_277 }, 3);
        let mut pcm = [0i16; 1024];
        adc.fill(&mut pcm);
        // 24-bit words with some low-order content under each sample.
        let words: [u32; 1024] = core::array::from_fn(|i| word(i32::from(pcm[i]) * 256 + (i as i32 & 0xFF)));
        let mut ring = [0i16; 2048];
        as_ring(&words, &mut ring);
        let low_halves: [i16; 1024] = core::array::from_fn(|i| ring[2 * i]);

        let block = unpack_24in32(&mut ring);
        assert_eq!(block, &pcm[..]);
        let mut m = Meter::default();
        m.feed(block, 1, 23);
        let l = m.levels();
        assert!((-21..=-19).contains(&l.bar[0]), "{l:?}");
        assert_eq!(db(u32::from(l.peak[0])), l.bar[0]);

        // The wrong half: no relation to the signal's level.
        let mut wrong = Meter::default();
        wrong.feed(&low_halves, 1, 23);
        assert_ne!(wrong.levels().bar[0], l.bar[0]);
    }

    /// The chain meters the block it is given, after its own processing:
    /// speech-level input does not read as the floor.
    #[test]
    fn chain_meters_the_fed_block() {
        let mut adc = FakeAdc::new(44_100, 1, Signal::Tone { hz: 440, amp: 8_000 }, 5);
        let mut words = [0u32; 2048];
        let mut pcm = [0i16; 2048];
        adc.fill(&mut pcm);
        for (w, s) in words.iter_mut().zip(pcm) {
            *w = word(i32::from(s) << 8);
        }
        let mut ring = [0i16; 4096];
        as_ring(&words, &mut ring);
        let mut c = Chain::new(ChainConfig { lowcut: true, ..ChainConfig::default() }, 44_100, 1);
        let block = unpack_24in32(&mut ring);
        assert!(c.process(block));
        let l = c.levels();
        assert!(l.bar[0] > -15 && l.bar[0] < -9, "{l:?}");
        assert!(!l.clip);
    }
}
