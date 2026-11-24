//! Frame decoding and the bit reservoir.
//!
//! Adapted from mbop3 0.1.0 `decoder.rs` (MIT OR Apache-2.0, derived from
//! minimp3, CC0): the decoder state is one `#[repr(C)]` `Pod` struct so that
//! it can be placed in the codec arena, and frame sync moved out to the
//! stream layer in `lib.rs`, which hands over exactly one validated frame.

use crate::bits::BitReader;
use crate::header::{HDR_SIZE, Header};
use crate::layer3::{self, GrInfo};
use crate::synth::{HIST_ROWS, synth_granule};

const MAX_BITRESERVOIR_BYTES: usize = 511;

/// Samples (all channels) a frame can produce.
pub(crate) const MAX_SAMPLES_PER_FRAME: usize = 1152 * 2;

/// All decoder memory, including per-frame working memory (about 12 KiB).
/// Field order keeps the struct free of padding (required by `Pod`).
#[derive(Clone, Copy, bytemuck::Zeroable, bytemuck::Pod)]
#[repr(C)]
pub(crate) struct State {
    mdct_overlap: [[f32; 288]; 2],
    synth_hist: [f32; HIST_ROWS * 64],
    // Per-frame work (not carried between frames).
    grbuf: [f32; 576 * 2],
    scf: [f32; 40],
    reserv: i32,
    free_format_bytes: i32,
    gr_info: [GrInfo; 4],
    header: [u8; 4],
    reserv_buf: [u8; MAX_BITRESERVOIR_BYTES],
    /// Per channel: bands of mdct_overlap that may be nonzero.
    overlap_bands: [u8; 2],
    ist_pos: [[u8; 39]; 2],
    /// Whether grbuf's channel 1 half may be nonzero.
    grbuf1_dirty: u8,
}

impl State {
    /// Forget the stream (as at its start, or after a seek).
    pub(crate) fn reset(&mut self) {
        self.mdct_overlap = [[0.0; 288]; 2];
        self.synth_hist = [0.0; HIST_ROWS * 64];
        self.reserv = 0;
        self.free_format_bytes = 0;
        self.header = [0; 4];
        self.reserv_buf = [0; MAX_BITRESERVOIR_BYTES];
        self.overlap_bands = [0; 2];
    }

    /// Decode one complete Layer III frame (`frame` starts with its header
    /// and is exactly the frame's length). Returns samples per channel
    /// written to `pcm` (0 when the bit reservoir the frame refers to is
    /// missing, e.g. right after a seek).
    pub(crate) fn decode_frame(&mut self, frame: &[u8], pcm: &mut [i16; MAX_SAMPLES_PER_FRAME]) -> usize {
        if frame.len() < HDR_SIZE {
            return 0;
        }
        let hdr = Header::at(frame);
        if self.header[0] != 0xFF || !Header(self.header).compare(hdr) {
            self.reset();
        }
        self.header = hdr.0;
        let frame_size = frame.len().min(i32::MAX as usize) as i32;
        let nch = if hdr.is_mono() { 1 } else { 2 };

        let payload = &frame[HDR_SIZE..];
        let mut bs_frame = BitReader::new(payload, frame_size - HDR_SIZE as i32);
        if hdr.is_crc() {
            bs_frame.get_bits(16);
        }
        let main_data_begin = layer3::read_side_info(&mut bs_frame, &mut self.gr_info, hdr);
        if main_data_begin < 0 || bs_frame.pos > bs_frame.limit {
            self.header[0] = 0;
            return 0;
        }
        // Scalefactor reuse (scfsi) can read these before they are written.
        self.ist_pos = [[0; 39]; 2];

        // Main data: the end of the bit reservoir, then this frame's payload
        // after the side info.
        let frame_start = (bs_frame.pos / 8) as usize;
        let frame_main_len = ((bs_frame.limit - bs_frame.pos) / 8) as usize;
        let frame_main = &payload[frame_start..frame_start + frame_main_len];
        let bytes_have = self.reserv.min(main_data_begin) as usize;
        let reserv_from = (self.reserv - main_data_begin).max(0) as usize;
        let success = self.reserv >= main_data_begin;

        let (save_pos, save_len) = {
            let head = &self.reserv_buf[reserv_from..reserv_from + bytes_have];
            let mut bs = BitReader::new_split(head, frame_main);
            if success {
                let granules = if hdr.test_mpeg1() { 2 } else { 1 };
                for igr in 0..granules {
                    let (ch0, ch1) = self.grbuf.split_at_mut(576);
                    ch0.fill(0.0);
                    if self.grbuf1_dirty != 0 || nch == 2 {
                        ch1.fill(0.0);
                    }
                    let gr = &self.gr_info[igr * nch..];
                    self.grbuf1_dirty = u8::from(nch == 2 || hdr.test_i_stereo() || gr[0].n_short_sfb != 0);
                    let active = decode_granule(
                        hdr,
                        &mut bs,
                        &self.gr_info[igr * nch..],
                        nch,
                        &mut self.grbuf,
                        &mut self.scf,
                        &mut self.ist_pos,
                        &mut self.mdct_overlap,
                        &mut self.overlap_bands,
                    );
                    synth_granule(
                        &mut self.synth_hist,
                        &mut self.grbuf,
                        nch,
                        active,
                        &mut pcm[igr * 576 * nch..],
                    );
                }
            }
            reservoir_to_save(&bs)
        };

        // Keep the last (up to 511) bytes of main data for the next frame.
        // They may start in the old reservoir, which is moved down first.
        let from_head = bytes_have.saturating_sub(save_pos).min(save_len);
        let head_start = reserv_from + save_pos.min(bytes_have);
        self.reserv_buf.copy_within(head_start..head_start + from_head, 0);
        let tail_start = (save_pos + from_head).saturating_sub(bytes_have);
        let from_tail = save_len - from_head;
        self.reserv_buf[from_head..save_len].copy_from_slice(&frame_main[tail_start..tail_start + from_tail]);
        self.reserv = save_len as i32;

        if success { hdr.frame_samples() as usize } else { 0 }
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_granule(
    hdr: Header,
    bs: &mut BitReader,
    gr_info: &[GrInfo],
    nch: usize,
    grbuf: &mut [f32; 1152],
    scf: &mut [f32; 40],
    ist_pos: &mut [[u8; 39]; 2],
    mdct_overlap: &mut [[f32; 288]; 2],
    overlap_bands: &mut [u8; 2],
) -> [bool; 2] {
    let mut active = [false; 2];
    // Values of each channel that may be nonzero. grbuf is zeroed before each
    // granule, so everything after what Huffman decoding wrote is zero.
    let mut nonzero = [576usize; 2];
    for ch in 0..nch {
        let layer3gr_limit = bs.pos + i32::from(gr_info[ch].part_23_length);
        layer3::decode_scalefactors(hdr, &mut ist_pos[ch], bs, &gr_info[ch], scf, ch);
        nonzero[ch] = layer3::huffman(&mut grbuf[576 * ch..], bs, &gr_info[ch], scf, layer3gr_limit);
    }
    if hdr.test_i_stereo() || hdr.is_ms_stereo() {
        // Stereo processing mixes the channels.
        let n = nonzero[0].max(nonzero[1]);
        nonzero = [n; 2];
    }

    if hdr.test_i_stereo() {
        layer3::intensity_stereo(grbuf, &mut ist_pos[1], gr_info, hdr);
    } else if hdr.is_ms_stereo() {
        layer3::midside_stereo(grbuf, 0, 576);
    }

    for ch in 0..nch {
        let gr = &gr_info[ch];
        let mut aa_bands = 31;
        let n_long_bands =
            (if gr.mixed_block_flag != 0 { 2 } else { 0 }) << u32::from(hdr.my_sample_rate() == 2);

        if gr.n_short_sfb != 0 {
            aa_bands = n_long_bands as i32 - 1;
            // What follows this channel's grbuf in minimp3's scratch struct.
            let (ch0, ch1) = grbuf.split_at_mut(576);
            let (g, spill): (&mut [f32], &mut [f32]) = if ch == 0 { (ch0, ch1) } else { (ch1, &mut scf[..]) };
            layer3::reorder(&mut g[n_long_bands * 18..], spill, &gr.sfbtab()[gr.n_long_sfb as usize..]);
        }
        // Bands that may be nonzero. The exact build does all the work
        // (skipped zeros could differ from minimp3's in sign).
        let mut bands = if cfg!(feature = "exact") || gr.n_short_sfb != 0 {
            32
        } else {
            nonzero[ch].div_ceil(18).min(32)
        };
        let g = &mut grbuf[576 * ch..576 * ch + 576];
        // Antialiasing mixes each band with the next.
        layer3::antialias(g, aa_bands.min(bands as i32));
        if bands > 0 {
            bands = (bands + 1).min(32);
        }
        // A band with zero input and zero overlap has zero output and overlap.
        let imdct_bands = bands.max(overlap_bands[ch] as usize);
        layer3::imdct_gr(g, &mut mdct_overlap[ch], gr.block_type, n_long_bands, imdct_bands);
        layer3::change_sign(g, imdct_bands);
        overlap_bands[ch] = bands as u8;
        active[ch] = imdct_bands > 0;
    }
    active
}

/// Which main data bytes to keep as the next frame's bit reservoir: (start, len).
fn reservoir_to_save(bs: &BitReader) -> (usize, usize) {
    let mut pos = ((bs.pos + 7) as u32 / 8) as i32;
    let mut remains = (bs.limit as u32 / 8) as i32 - pos;
    if remains > MAX_BITRESERVOIR_BYTES as i32 {
        pos += remains - MAX_BITRESERVOIR_BYTES as i32;
        remains = MAX_BITRESERVOIR_BYTES as i32;
    }
    (pos.max(0) as usize, remains.max(0) as usize)
}
