use std::vec::Vec;

use yoton_audio::{SeekSource, SliceSource};

use crate::assembled::{Next, Packet, PacketReader};
use crate::*;

/// Write one page holding `segs` (lacing values) over `body`.
fn page(out: &mut Vec<u8>, flags: u8, granule: u64, serial: u32, seq: u32, lacing: &[u8], body: &[u8]) {
    let start = out.len();
    out.extend_from_slice(b"OggS");
    out.push(0);
    out.push(flags);
    out.extend_from_slice(&granule.to_le_bytes());
    out.extend_from_slice(&serial.to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.push(lacing.len() as u8);
    out.extend_from_slice(lacing);
    out.extend_from_slice(body);
    let crc = crc_update(0, &out[start..]);
    out[start + 22..start + 26].copy_from_slice(&crc.to_le_bytes());
}

/// A simple muxer: packets of the given lengths (byte value = packet index),
/// at most `per_page` body bytes per page, granule = index of the packet
/// times 100 for the last packet ending on the page.
fn mux(serial: u32, lens: &[usize], per_page: usize) -> (Vec<u8>, Vec<Vec<u8>>) {
    let packets: Vec<Vec<u8>> =
        lens.iter().enumerate().map(|(i, &n)| (0..n).map(|k| (i * 7 + k) as u8).collect()).collect();
    let mut out = Vec::new();
    let mut seq = 0;
    // BOS page with an identification packet of its own.
    page(&mut out, FLAG_BOS, 0, serial, seq, &[4], b"HEAD");
    seq += 1;
    let mut lacing = Vec::new();
    let mut body = Vec::new();
    let mut continued = false;
    let mut last_done: Option<usize> = None;
    for (i, p) in packets.iter().enumerate() {
        let mut off = 0;
        loop {
            let n = (p.len() - off).min(255);
            lacing.push(n as u8);
            body.extend_from_slice(&p[off..off + n]);
            off += n;
            let ends = n < 255;
            if ends {
                last_done = Some(i);
            }
            if body.len() >= per_page || lacing.len() == 255 {
                let g = last_done.map_or(NO_GRANULE, |k| (k as u64 + 1) * 100);
                let eos = ends && i + 1 == packets.len();
                let flags = u8::from(continued) | if eos { FLAG_EOS } else { 0 };
                page(&mut out, flags, g, serial, seq, &lacing, &body);
                seq += 1;
                continued = !ends;
                lacing.clear();
                body.clear();
                last_done = None;
            }
            if ends {
                break;
            }
        }
    }
    if !lacing.is_empty() {
        let g = last_done.map_or(NO_GRANULE, |k| (k as u64 + 1) * 100);
        page(&mut out, u8::from(continued) | FLAG_EOS, g, serial, seq, &lacing, &body);
    }
    (out, packets)
}

fn read_all(file: &[u8], ring: usize, chunk: usize) -> (Vec<(Vec<u8>, Packet)>, usize) {
    let mut src = if ring == 0 { SliceSource::new(file) } else { SliceSource::ring(file, ring) };
    let mut r = PacketReader::new();
    let mut buf = [0u8; 4096];
    let mut out = Vec::new();
    let mut corrupt = 0;
    for _ in 0..1_000_000 {
        match r.next_packet(&mut src, &mut buf) {
            Next::Packet(p) => {
                if r.selected().is_none() {
                    r.select(p.serial);
                }
                out.push((buf[..p.len].to_vec(), p));
            }
            Next::NeedData(n) => {
                assert!(n > src.available() && n <= MAX_PEEK);
                assert!(!src.at_end());
                if chunk == 0 { src.refill() } else { src.trickle(chunk) }
            }
            Next::End => return (out, corrupt),
            Next::Corrupt => corrupt += 1,
        }
    }
    panic!("no end");
}

#[test]
fn reassembles_across_pages_and_ring_wraps() {
    let lens = [10, 255, 0, 510, 3000, 1, 254, 256, 9000, 77];
    let (file, packets) = mux(0x1234, &lens, 700);
    for (ring, chunk) in [(0, 0), (4096, 0), (4096, 333), (1024, 1)] {
        let (got, corrupt) = read_all(&file, ring.max(if ring == 0 { 0 } else { 2048 }), chunk);
        assert_eq!(corrupt, 0);
        assert_eq!(got.len(), packets.len() + 1, "ring {ring} chunk {chunk}");
        assert_eq!(&got[0].0, b"HEAD");
        assert!(got[0].1.bos);
        for (i, (g, p)) in got[1..].iter().enumerate() {
            if lens[i] <= 4096 {
                assert_eq!(g, &packets[i], "packet {i}");
                assert!(!p.truncated);
            } else {
                assert!(p.truncated);
                assert_eq!(&g[..], &packets[i][..4096]);
            }
            assert!(!p.after_gap);
        }
        assert!(got.last().is_some_and(|(_, p)| p.eos && p.granule == Some(1000)));
    }
}

#[test]
fn corrupt_page_is_skipped_and_flagged() {
    let lens = [100, 100, 100, 100, 100, 100];
    let (mut file, _) = mux(7, &lens, 200);
    // Flip a byte in the body of the third page.
    let mut pos = 0;
    let mut starts = Vec::new();
    while let Some(k) = file[pos..].windows(4).position(|w| w == b"OggS") {
        starts.push(pos + k);
        pos += k + 1;
    }
    file[starts[2] + 40] ^= 0x55;
    let (got, corrupt) = read_all(&file, 0, 0);
    assert!(corrupt > 0);
    assert!(got.iter().any(|(_, p)| p.after_gap));
    assert!(got.len() < lens.len() + 1);
}

#[test]
fn other_streams_are_skipped() {
    let (a, pa) = mux(1, &[300, 40, 600], 256);
    let (b, _) = mux(2, &[50, 50, 50, 50], 64);
    // Interleave: both BOS pages first, then alternate the rest.
    let split = |f: &[u8]| -> Vec<Vec<u8>> {
        let mut v = Vec::new();
        let mut i = 0;
        while i < f.len() {
            let v0 = View::new(&f[i..], &[]);
            let Parse::Page(h) = PageHeader::parse(&v0) else { panic!() };
            v.push(f[i..i + h.total()].to_vec());
            i += h.total();
        }
        v
    };
    let (sa, sb) = (split(&a), split(&b));
    let mut file = Vec::new();
    file.extend_from_slice(&sa[0]);
    file.extend_from_slice(&sb[0]);
    for k in 1..sa.len().max(sb.len()) {
        if let Some(p) = sa.get(k) {
            file.extend_from_slice(p);
        }
        if let Some(p) = sb.get(k) {
            file.extend_from_slice(p);
        }
    }
    let (got, corrupt) = read_all(&file, 0, 0);
    assert_eq!(corrupt, 0);
    // The reader selected the first BOS stream; the second BOS packet was
    // delivered before selection only if it came first.
    let ours: Vec<_> = got.iter().filter(|(_, p)| p.serial == 1).collect();
    assert_eq!(ours.len(), 4);
    for (i, (g, _)) in ours[1..].iter().enumerate() {
        assert_eq!(g, &pa[i]);
    }
}

#[test]
fn bisection_finds_the_last_page_before_target() {
    let lens: Vec<usize> = (0..2000).map(|i| 40 + (i * 37) % 300).collect();
    let (file, packets) = mux(9, &lens, 1000);
    // Linear reference: (page offset, granule) of every page.
    let mut pages = Vec::new();
    let mut i = 0;
    while i < file.len() {
        let Parse::Page(h) = PageHeader::parse(&View::new(&file[i..], &[])) else { panic!() };
        pages.push((i as u64, h.granule));
        i += h.total();
    }
    for target in [0u64, 50, 99, 100, 101, 5000, 77_777, 150_000, 199_999, 200_000, 10_000_000] {
        let mut src = SliceSource::ring(&file, 8192);
        let mut r = PacketReader::new();
        r.select(9);
        let mut b = Bisect::new(pages[1].0, file.len() as u64, target, 9);
        let found = loop {
            match b.step(&mut src).unwrap() {
                SeekStep::NeedData(n) => {
                    assert!(n > src.available());
                    src.refill();
                }
                SeekStep::Done(f) => break f,
            }
        };
        let want = pages.iter().rev().find(|&&(_, g)| g != NO_GRANULE && g <= target && g > 0).copied();
        assert_eq!(found, want, "target {target}");
        // Resume after that page: the next packet starts at its granule.
        if let Some((pos, g)) = found {
            src.seek(pos).unwrap();
            src.refill();
            r.restart(true);
            let mut buf = [0u8; 512];
            let k = (g / 100) as usize;
            loop {
                match r.next_packet(&mut src, &mut buf) {
                    Next::Packet(p) => {
                        if k < packets.len() {
                            assert_eq!(&buf[..p.len], &packets[k][..], "target {target}");
                        }
                        break;
                    }
                    Next::NeedData(_) => src.refill(),
                    Next::End => {
                        assert_eq!(k, packets.len());
                        break;
                    }
                    Next::Corrupt => panic!(),
                }
            }
        }
    }
}

#[test]
fn garbage_never_panics_or_loops() {
    let (file, _) = mux(3, &[500, 20, 3000, 10], 400);
    let mut x = 0x9E37_79B9u32;
    for n in 0..300 {
        let mut f = file.clone();
        for _ in 0..(1 + n % 20) {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let i = x as usize % f.len();
            f[i] ^= (x >> 8) as u8;
        }
        f.truncate(f.len() - (x as usize % 64));
        let _ = read_all(&f, 32 * 1024, 1000);
    }
}
