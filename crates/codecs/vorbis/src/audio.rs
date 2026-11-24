//! Audio packet decode (Vorbis I §4.3): floors, residues, inverse
//! coupling, floor curve × residue, inverse MDCT, overlap-add.
//!
//! Memory per channel: the spectrum / DCT-IV buffer (n/2 floats) and the
//! part of the previous block still needed for overlap (n/4 floats). The
//! MDCT output is never materialised: with u the DCT-IV of the spectrum
//! (M = n/2), the IMDCT output is y[t] = u[t + M/2] for t < M/2,
//! −u[3M/2 − 1 − t] for t < 3M/2 and −u[t − 3M/2] beyond, so the right half
//! of a block lives entirely in u[0..n/4].

use crate::bits::{Bits, ilog};
use crate::math;
use crate::setup::{Floor1, MAX_X, Mapping, Residue, Setup};

/// floor1_inverse_dB_table (Vorbis I §10.1).
#[rustfmt::skip]
#[allow(clippy::excessive_precision, clippy::unreadable_literal)]
static INV_DB: [f32; 256] = [
    1.0649863e-07, 1.1341951e-07, 1.2079015e-07, 1.2863978e-07, 1.3699951e-07, 1.4590251e-07, 1.5538408e-07, 1.6548181e-07,
    1.7623575e-07, 1.8768855e-07, 1.9988561e-07, 2.1287530e-07, 2.2670913e-07, 2.4144197e-07, 2.5713223e-07, 2.7384213e-07,
    2.9163793e-07, 3.1059021e-07, 3.3077411e-07, 3.5226968e-07, 3.7516214e-07, 3.9954229e-07, 4.2550680e-07, 4.5315863e-07,
    4.8260743e-07, 5.1396998e-07, 5.4737065e-07, 5.8294187e-07, 6.2082472e-07, 6.6116941e-07, 7.0413592e-07, 7.4989464e-07,
    7.9862701e-07, 8.5052630e-07, 9.0579828e-07, 9.6466216e-07, 1.0273513e-06, 1.0941144e-06, 1.1652161e-06, 1.2409384e-06,
    1.3215816e-06, 1.4074654e-06, 1.4989305e-06, 1.5963394e-06, 1.7000785e-06, 1.8105592e-06, 1.9282195e-06, 2.0535261e-06,
    2.1869758e-06, 2.3290978e-06, 2.4804557e-06, 2.6416497e-06, 2.8133190e-06, 2.9961443e-06, 3.1908506e-06, 3.3982101e-06,
    3.6190449e-06, 3.8542308e-06, 4.1047004e-06, 4.3714470e-06, 4.6555282e-06, 4.9580707e-06, 5.2802740e-06, 5.6234160e-06,
    5.9888572e-06, 6.3780469e-06, 6.7925283e-06, 7.2339451e-06, 7.7040476e-06, 8.2047000e-06, 8.7378876e-06, 9.3057248e-06,
    9.9104632e-06, 1.0554501e-05, 1.1240392e-05, 1.1970856e-05, 1.2748789e-05, 1.3577278e-05, 1.4459606e-05, 1.5399272e-05,
    1.6400004e-05, 1.7465768e-05, 1.8600792e-05, 1.9809576e-05, 2.1096914e-05, 2.2467911e-05, 2.3928002e-05, 2.5482978e-05,
    2.7139006e-05, 2.8902651e-05, 3.0780908e-05, 3.2781225e-05, 3.4911534e-05, 3.7180282e-05, 3.9596466e-05, 4.2169667e-05,
    4.4910090e-05, 4.7828601e-05, 5.0936773e-05, 5.4246931e-05, 5.7772202e-05, 6.1526565e-05, 6.5524908e-05, 6.9783085e-05,
    7.4317983e-05, 7.9147585e-05, 8.4291040e-05, 8.9768747e-05, 9.5602426e-05, 0.00010181521, 0.00010843174, 0.00011547824,
    0.00012298267, 0.00013097477, 0.00013948625, 0.00014855085, 0.00015820453, 0.00016848555, 0.00017943469, 0.00019109536,
    0.00020351382, 0.00021673929, 0.00023082423, 0.00024582449, 0.00026179955, 0.00027881276, 0.00029693158, 0.00031622787,
    0.00033677814, 0.00035866388, 0.00038197188, 0.00040679456, 0.00043323036, 0.00046138411, 0.00049136745, 0.00052329927,
    0.00055730621, 0.00059352311, 0.00063209358, 0.00067317058, 0.00071691700, 0.00076350630, 0.00081312324, 0.00086596457,
    0.00092223983, 0.00098217216, 0.0010459992, 0.0011139742, 0.0011863665, 0.0012634633, 0.0013455702, 0.0014330129,
    0.0015261382, 0.0016253153, 0.0017309374, 0.0018434235, 0.0019632195, 0.0020908006, 0.0022266726, 0.0023713743,
    0.0025254795, 0.0026895994, 0.0028643847, 0.0030505286, 0.0032487691, 0.0034598925, 0.0036847358, 0.0039241906,
    0.0041792066, 0.0044507950, 0.0047400328, 0.0050480668, 0.0053761186, 0.0057254891, 0.0060975636, 0.0064938176,
    0.0069158225, 0.0073652516, 0.0078438871, 0.0083536271, 0.0088964928, 0.009474637, 0.010090352, 0.010746080,
    0.011444421, 0.012188144, 0.012980198, 0.013823725, 0.014722068, 0.015678791, 0.016697687, 0.017782797,
    0.018938423, 0.020169149, 0.021479854, 0.022875735, 0.024362330, 0.025945531, 0.027631618, 0.029427276,
    0.031339626, 0.033376252, 0.035545228, 0.037855157, 0.040315199, 0.042935108, 0.045725273, 0.048696758,
    0.051861348, 0.055231591, 0.058820850, 0.062643361, 0.066714279, 0.071049749, 0.075666962, 0.080584227,
    0.085821044, 0.091398179, 0.097337747, 0.10366330, 0.11039993, 0.11757434, 0.12521498, 0.13335215,
    0.14201813, 0.15124727, 0.16107617, 0.17154380, 0.18269168, 0.19456402, 0.20720788, 0.22067342,
    0.23501402, 0.25028656, 0.26655159, 0.28387361, 0.30232132, 0.32196786, 0.34289114, 0.36517414,
    0.38890521, 0.41417847, 0.44109412, 0.46975890, 0.50028648, 0.53279791, 0.56742212, 0.60429640,
    0.64356699, 0.68538959, 0.72993007, 0.77736504, 0.82788260, 0.88168307, 0.9389798, 1.,
];

/// Decode-time buffers, all in the arena.
pub(crate) struct Buffers<'a> {
    /// Spectrum, then DCT-IV output: `ch × half_long`.
    pub buf: &'a mut [f32],
    /// Previous block's u[0..n/4]: `ch × quarter_long`.
    pub prev: &'a mut [f32],
    pub scratch: &'a mut [[f32; 2]],
    pub quarter: &'a mut [f32],
    pub win: [&'a mut [f32]; 2],
    /// Floor Y values per channel.
    pub fy: &'a mut [i32],
    pub classes: &'a mut [u8],
}

/// Block geometry of one packet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Block {
    pub n: usize,
    pub long: bool,
    pub left_start: usize,
    pub left_end: usize,
    pub right_start: usize,
    pub right_end: usize,
}

impl Block {
    pub(crate) fn new(bs: [u32; 2], long: bool, prev_long: bool, next_long: bool) -> Self {
        let n = 1usize << bs[usize::from(long)];
        let n0 = 1usize << bs[0];
        let (left_start, left_end) =
            if !long || prev_long { (0, n / 2) } else { ((n - n0) / 4, (n + n0) / 4) };
        let (right_start, right_end) =
            if !long || next_long { (n / 2, n) } else { ((3 * n - n0) / 4, (3 * n + n0) / 4) };
        Self { n, long, left_start, left_end, right_start, right_end }
    }
}

/// Floor 1 for one channel: decode the Y list (Vorbis I §7.2.3).
fn floor1_decode(f: &Floor1, s: &Setup<'_>, b: &mut Bits<'_>, y: &mut [i32]) -> bool {
    if !b.bit() {
        return false;
    }
    let range = [256u32, 128, 86, 64][usize::from(f.multiplier.clamp(1, 4)) - 1];
    let bits = ilog(range - 1);
    y[0] = b.read(bits) as i32;
    y[1] = b.read(bits) as i32;
    let mut off = 2usize;
    for i in 0..usize::from(f.partitions) {
        let c = usize::from(f.class[i]);
        let cdim = usize::from(f.dims[c]);
        let cbits = u32::from(f.subs[c]);
        let csub = (1u32 << cbits) - 1;
        let mut cval = 0u32;
        if cbits > 0 {
            let Some(v) = s.books[usize::from(f.master[c])].decode(s.nodes, b) else { return false };
            cval = v;
        }
        for _ in 0..cdim {
            let book = f.subbooks[c][(cval & csub) as usize];
            cval >>= cbits;
            let v = if book >= 0 {
                match s.books[book as usize].decode(s.nodes, b) {
                    Some(v) => v as i32,
                    None => return false,
                }
            } else {
                0
            };
            if off < MAX_X {
                y[off] = v;
            }
            off += 1;
        }
    }
    !b.eop
}

fn render_point(x0: i32, y0: i32, x1: i32, y1: i32, x: i32) -> i32 {
    let dy = y1 - y0;
    let adx = (x1 - x0).max(1);
    let off = dy.abs() * (x - x0) / adx;
    if dy < 0 { y0 - off } else { y0 + off }
}

/// Apply the floor curve to `v` (length n/2): `v[x] *= inv_dB(curve[x])`.
fn floor1_apply(f: &Floor1, y: &mut [i32], v: &mut [f32]) {
    let n = usize::from(f.values).min(MAX_X);
    let range = [256i32, 128, 86, 64][usize::from(f.multiplier.clamp(1, 4)) - 1];
    let mut step2 = [false; MAX_X];
    step2[0] = true;
    step2[1] = true;
    let mut fy = [0i32; MAX_X];
    fy[0] = y[0];
    fy[1] = y[1];
    for i in 2..n {
        let (lo, hi) = (usize::from(f.low[i]), usize::from(f.high[i]));
        let predicted =
            render_point(i32::from(f.x[lo]), fy[lo], i32::from(f.x[hi]), fy[hi], i32::from(f.x[i]));
        let val = y[i];
        let highroom = range - predicted;
        let lowroom = predicted;
        let room = highroom.min(lowroom) * 2;
        if val != 0 {
            step2[lo] = true;
            step2[hi] = true;
            step2[i] = true;
            fy[i] = if val >= room {
                if highroom > lowroom { val - lowroom + predicted } else { predicted - val + highroom - 1 }
            } else if val & 1 == 1 {
                predicted - (val + 1) / 2
            } else {
                predicted + val / 2
            };
        } else {
            fy[i] = predicted;
        }
    }
    for v in fy.iter_mut().take(n) {
        *v = (*v).clamp(0, range - 1);
    }
    let mult = i32::from(f.multiplier);
    let len = v.len();
    let (mut lx, mut ly) = (0usize, fy[usize::from(f.sorted[0])] * mult);
    let mut hx = 0usize;
    let mut hy = ly;
    for k in 1..n {
        let i = usize::from(f.sorted[k]);
        if step2[i] {
            hy = fy[i] * mult;
            hx = usize::from(f.x[i]);
            line(lx, ly, hx, hy, v);
            lx = hx;
            ly = hy;
        }
    }
    if hx < len {
        line(hx, hy, len, hy, v);
    }
    let _ = len;
}

/// Bresenham-style line of floor values from (x0, y0) to (x1, y1)
/// (exclusive), multiplying `v` (positions beyond `v` are dropped).
fn line(x0: usize, y0: i32, x1: usize, y1: i32, v: &mut [f32]) {
    if x1 <= x0 {
        return;
    }
    let dy = y1 - y0;
    let adx = (x1 - x0) as i32;
    let base = dy / adx;
    let sy = if dy < 0 { base - 1 } else { base + 1 };
    let ady = dy.abs() - base.abs() * adx;
    let mut y = y0;
    let mut err = 0;
    let end = x1.min(v.len());
    if x0 < end {
        v[x0] *= INV_DB[y.clamp(0, 255) as usize];
    }
    for x in x0 + 1..end {
        err += ady;
        if err >= adx {
            err -= adx;
            y += sy;
        } else {
            y += base;
        }
        v[x] *= INV_DB[y.clamp(0, 255) as usize];
    }
}

/// Residue decode for one submap (Vorbis I §8.6). `vecs` are the channel
/// indices (into `buf`, stride `stride`) of this submap's channels, `skip`
/// which of them carry no residue. Returns `false` on a decode error.
#[allow(clippy::too_many_arguments)]
fn residue(
    r: &Residue,
    s: &Setup<'_>,
    b: &mut Bits<'_>,
    buf: &mut [f32],
    stride: usize,
    half: usize,
    vecs: &[usize],
    skip: &[bool],
    classes: &mut [u8],
) {
    let nv = vecs.len();
    if nv == 0 {
        return;
    }
    let (count, vec_len) = if r.kind == 2 {
        if skip.iter().all(|&s| s) {
            return;
        }
        (1usize, half * nv)
    } else {
        (nv, half)
    };
    let begin = (r.begin as usize).min(vec_len);
    let end = (r.end as usize).min(vec_len);
    let psize = r.psize as usize;
    let parts = (end - begin) / psize;
    if parts == 0 {
        return;
    }
    let cb = &s.books[usize::from(r.classbook)];
    let cwpc = usize::from(cb.dims);
    let cstride = parts + cwpc;
    if cwpc == 0 || cstride * count > classes.len() {
        return;
    }
    let classes_n = u32::from(r.classes).max(1);
    // Element `k` of vector `j`.
    let mut add = |j: usize, k: usize, x: f32| {
        let (c, pos) = if r.kind == 2 { (vecs[k % nv], k / nv) } else { (vecs[j], k) };
        if pos < half
            && let Some(e) = buf.get_mut(c * stride + pos)
        {
            *e += x;
        }
    };
    let decode_vec = |j: usize| r.kind == 2 || !skip[j];
    for pass in 0..8 {
        let mut p = 0usize;
        while p < parts {
            if pass == 0 {
                for j in 0..count {
                    if !decode_vec(j) {
                        continue;
                    }
                    let Some(mut t) = cb.decode(s.nodes, b) else { return };
                    for i in (0..cwpc).rev() {
                        classes[j * cstride + p + i] = (t % classes_n) as u8;
                        t /= classes_n;
                    }
                }
            }
            for _ in 0..cwpc {
                if p >= parts {
                    break;
                }
                for j in 0..count {
                    if !decode_vec(j) {
                        continue;
                    }
                    let c = usize::from(classes[j * cstride + p]);
                    let book = r.books.get(c).map_or(-1, |bk| bk[pass]);
                    if book < 0 {
                        continue;
                    }
                    let bk = &s.books[book as usize];
                    let dims = usize::from(bk.dims);
                    if dims == 0 {
                        return;
                    }
                    let off = begin + p * psize;
                    if r.kind == 0 {
                        let step = psize / dims;
                        for i in 0..step {
                            let Some(e) = bk.decode(s.nodes, b) else { return };
                            bk.vq(s.vals, e, |d, x| add(j, off + i + d * step, x));
                        }
                    } else {
                        let mut i = 0;
                        while i < psize {
                            let Some(e) = bk.decode(s.nodes, b) else { return };
                            bk.vq(s.vals, e, |d, x| {
                                if i + d < psize {
                                    add(j, off + i + d, x);
                                }
                            });
                            i += dims;
                        }
                    }
                }
                p += 1;
            }
        }
    }
}

/// Decode one audio packet into the channels' u vectors (`buf`). Returns
/// the block geometry, or `None` for a packet to drop.
pub(crate) fn decode_packet(
    s: &Setup<'_>,
    bs: [u32; 2],
    ch: usize,
    bufs: &mut Buffers<'_>,
    b: &mut Bits<'_>,
) -> Option<Block> {
    if b.bit() {
        return None; // a header packet
    }
    let mode_bits = ilog(s.modes.len() as u32 - 1);
    let mode = *s.modes.get(b.read(mode_bits) as usize)?;
    let long = mode.long != 0;
    let (prev_long, next_long) = if long { (b.bit(), b.bit()) } else { (false, false) };
    if b.eop {
        return None;
    }
    let blk = Block::new(bs, long, prev_long, next_long);
    let half = blk.n / 2;
    let stride = bufs.buf.len() / ch;
    let map: &Mapping = s.mappings.get(usize::from(mode.mapping))?;
    for c in 0..ch {
        bufs.buf[c * stride..c * stride + half].fill(0.0);
    }
    // Floors.
    let mut used = [false; 8];
    for (c, u) in used.iter_mut().enumerate().take(ch) {
        let sub = usize::from(map.mux[c]);
        let f = s.floors.get(usize::from(map.floor[sub]))?;
        *u = floor1_decode(f, s, b, &mut bufs.fy[c * MAX_X..(c + 1) * MAX_X]);
    }
    // Non-zero propagation through coupling.
    let mut nonzero = used;
    for k in 0..usize::from(map.steps) {
        let (m, a) = (usize::from(map.mag[k]), usize::from(map.ang[k]));
        if nonzero[m] || nonzero[a] {
            nonzero[m] = true;
            nonzero[a] = true;
        }
    }
    // Residues, per submap.
    for sub in 0..usize::from(map.submaps) {
        let mut vecs = [0usize; 8];
        let mut skip = [false; 8];
        let mut nv = 0;
        for c in 0..ch {
            if usize::from(map.mux[c]) == sub {
                vecs[nv] = c;
                skip[nv] = !nonzero[c];
                nv += 1;
            }
        }
        let r = s.residues.get(usize::from(map.residue[sub]))?;
        residue(r, s, b, bufs.buf, stride, half, &vecs[..nv], &skip[..nv], bufs.classes);
    }
    // Inverse coupling, last step first.
    for k in (0..usize::from(map.steps)).rev() {
        let (m, a) = (usize::from(map.mag[k]), usize::from(map.ang[k]));
        for i in 0..half {
            let mv = bufs.buf[m * stride + i];
            let av = bufs.buf[a * stride + i];
            let (nm, na) = if mv > 0.0 {
                if av > 0.0 { (mv, mv - av) } else { (mv + av, mv) }
            } else if av > 0.0 {
                (mv, mv + av)
            } else {
                (mv - av, mv)
            };
            bufs.buf[m * stride + i] = nm;
            bufs.buf[a * stride + i] = na;
        }
    }
    // Floor curve × residue, then the DCT-IV.
    for c in 0..ch {
        let v = &mut bufs.buf[c * stride..c * stride + half];
        if used[c] {
            let sub = usize::from(map.mux[c]);
            let f = &s.floors[usize::from(map.floor[sub])];
            floor1_apply(f, &mut bufs.fy[c * MAX_X..(c + 1) * MAX_X], v);
            math::dct4(v, bufs.scratch, bufs.quarter);
        } else {
            v.fill(0.0);
        }
    }
    Some(blk)
}

/// IMDCT output sample `t` of a block of size `n` from its u vector.
#[inline]
pub(crate) fn y(u: &[f32], n: usize, t: usize) -> f32 {
    let q = n / 4;
    if t < q {
        u[t + q]
    } else if t < 3 * q {
        -u[3 * q - 1 - t]
    } else {
        -u[t - 3 * q]
    }
}
