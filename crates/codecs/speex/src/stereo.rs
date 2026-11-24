//! Intensity stereo (libspeex `stereo.c`, float build): the in-band request
//! handler and `speex_decode_stereo_int`.

use crate::bits::Bits;
use crate::math;
use crate::tables;

static E_RATIO_QUANT: [f32; 4] = [0.25, 0.315, 0.397, 0.5];

#[derive(Debug, Clone, Copy)]
pub struct Stereo {
    balance: f32,
    e_ratio: f32,
    smooth_left: f32,
    smooth_right: f32,
}

impl Default for Stereo {
    fn default() -> Self {
        Self::new()
    }
}

impl Stereo {
    /// `speex_stereo_state_reset` (float).
    #[must_use]
    pub const fn new() -> Self {
        Self { balance: 1.0, e_ratio: 0.5, smooth_left: 1.0, smooth_right: 1.0 }
    }

    /// `speex_std_stereo_request_handler`.
    pub fn request(&mut self, buf: &[u8], bits: &mut Bits) {
        let neg = bits.unpack(buf, 1) != 0;
        let dexp = bits.unpack(buf, 5) as usize & 31;
        self.balance = tables::BALANCE[if neg { 32 + dexp } else { dexp }];
        self.e_ratio = E_RATIO_QUANT[bits.unpack(buf, 2) as usize & 3];
    }

    /// `speex_decode_stereo_int`: `data[..n]` holds a mono frame; it becomes
    /// `n` interleaved stereo frames in `data[..2n]`.
    pub fn decode(&mut self, data: &mut [i16], n: usize) {
        let e_right = 1.0 / math::sqrt(self.e_ratio * (1.0 + self.balance));
        let e_left = math::sqrt(self.balance) * e_right;
        for i in (0..n).rev() {
            let tmp = f32::from(data[i]);
            self.smooth_left = self.smooth_left * 0.98 + e_left * 0.02;
            self.smooth_right = self.smooth_right * 0.98 + e_right * 0.02;
            // C's float-to-short conversion: truncate (x86 converts to a
            // 32-bit integer and keeps the low 16 bits).
            data[2 * i] = (self.smooth_left * tmp) as i32 as i16;
            data[2 * i + 1] = (self.smooth_right * tmp) as i32 as i16;
        }
    }
}
