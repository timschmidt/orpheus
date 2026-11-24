use std::vec::Vec;

use orpheus_audio::{MAX_PEEK, SliceSource};

use crate::ranges::{Next, Packet, PageErr, Reader, peek_packets};
use crate::*;

/// Write one page of `serial` holding `segs` (lacing values) of `body`.
fn page(serial: u32, seq: u32, flags: u8, granule: u64, lacing: &[u8], body: &[u8]) -> Vec<u8> {
    let mut p = b"OggS\0".to_vec();
    p.push(flags);
    p.extend_from_slice(&granule.to_le_bytes());
    p.extend_from_slice(&serial.to_le_bytes());
    p.extend_from_slice(&seq.to_le_bytes());
    p.extend_from_slice(&[0; 4]);
    p.push(lacing.len() as u8);
    p.extend_from_slice(lacing);
    p.extend_from_slice(body);
    let c = crc_update(0, &p);
    p[22..26].copy_from_slice(&c.to_le_bytes());
    p
}

/// Packets laid out into pages of at most `max_segs` segments.
fn stream(packets: &[Vec<u8>], max_segs: usize, serial: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut lac, mut body): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new());
    let mut seq = 0;
    let mut continued = false;
    let mut granule = 0u64;
    let mut ends_here = false;
    for (i, p) in packets.iter().enumerate() {
        let mut rest = p.len();
        let mut off = 0;
        loop {
            let l = rest.min(255);
            lac.push(l as u8);
            body.extend_from_slice(&p[off..off + l]);
            off += l;
            rest -= l;
            let finished = l < 255;
            if finished {
                granule = (i as u64 + 1) * 100;
                ends_here = true;
            }
            if lac.len() == max_segs {
                let last = i == packets.len() - 1 && finished;
                let flags = u8::from(continued) | if seq == 0 { 2 } else { 0 } | if last { 4 } else { 0 };
                out.extend(page(
                    serial,
                    seq,
                    flags,
                    if ends_here { granule } else { NO_GRANULE },
                    &lac,
                    &body,
                ));
                seq += 1;
                continued = !finished;
                ends_here = false;
                lac.clear();
                body.clear();
            }
            if finished {
                break;
            }
        }
    }
    if !lac.is_empty() {
        let flags = u8::from(continued) | if seq == 0 { 2 } else { 0 } | 4;
        out.extend(page(serial, seq, flags, granule, &lac, &body));
    }
    out
}

fn read_all(data: &[u8], ring: usize, chunk: usize) -> Vec<(Vec<u8>, u64, bool)> {
    let mut src = SliceSource::ring(data, ring);
    let mut r = Reader::new();
    let mut out = Vec::new();
    for _ in 0..1_000_000 {
        match r.next(&mut src) {
            Next::Packet(p) => {
                let v = View::of(&src);
                let bytes: Vec<u8> = (0..p.len).map(|i| p.byte(&v, i)).collect();
                out.push((bytes, p.granule, p.eos));
                r.commit(&mut src, &p);
            }
            Next::NeedData(n) => {
                assert!(n > src.available() && !src.at_end());
                src.trickle(chunk);
            }
            Next::End => return out,
            Next::Corrupt => {}
        }
    }
    panic!("no end");
}

fn packets() -> Vec<Vec<u8>> {
    [3usize, 255, 0, 600, 1, 70_000 / 13, 254, 510, 2000]
        .iter()
        .enumerate()
        .map(|(i, &n)| (0..n).map(|k| (k * 7 + i) as u8).collect())
        .collect()
}

#[test]
fn packets_across_pages_and_ring_wraps() {
    let ps = packets();
    for segs in [1usize, 3, 17, 255] {
        let data = stream(&ps, segs, 0x1234);
        for (ring, chunk) in
            [(data.len().max(MAX_PEEK + 1), data.len()), (MAX_PEEK + 1, 333), (MAX_PEEK + 7, 4096)]
        {
            let got = read_all(&data, ring, chunk);
            let bytes: Vec<Vec<u8>> = got.iter().map(|g| g.0.clone()).collect();
            assert_eq!(bytes, ps, "segs {segs} ring {ring}");
            assert!(got.last().unwrap().2, "eos on the last packet");
            // A granule only on the last packet completing on its page.
            assert!(got.iter().filter(|g| g.1 != NO_GRANULE).all(|g| g.1 % 100 == 0));
        }
    }
}

#[test]
fn foreign_pages_and_damage() {
    let ps = packets();
    let mut data = stream(&ps[..3], 255, 7);
    let other = page(99, 0, 2, 5, &[3], b"xyz");
    data.splice(0..0, other.iter().copied());
    // Our stream first? No: the first page seen is the foreign one, so the
    // reader follows serial 99 and ends at our BOS.
    let got = read_all(&data, data.len(), data.len());
    assert_eq!(got.len(), 1);
    // A damaged byte in the middle: that page is skipped, the rest read.
    let mut data = stream(&ps, 3, 7);
    let mid = data.len() / 2;
    data[mid] ^= 0xFF;
    let got = read_all(&data, data.len().max(MAX_PEEK + 1), 999);
    let lens: Vec<usize> = got.iter().map(|g| g.0.len()).collect();
    let want: Vec<usize> = ps.iter().map(Vec::len).filter(|&l| l != 5384).collect();
    assert_eq!(lens, want, "only the damaged packet is lost");
}

#[test]
fn peek_packets_reads_headers_without_consuming() {
    let ps = packets();
    let data = stream(&ps, 17, 5);
    let v = View::new(&data, &[]);
    let mut out = [Packet::default(); 4];
    peek_packets(&v, true, &mut out).unwrap();
    for (p, want) in out.iter().zip(&ps) {
        let got: Vec<u8> = (0..p.len).map(|i| p.byte(&v, i)).collect();
        assert_eq!(&got, want);
    }
    // Cut short: more bytes are asked for.
    let v = View::new(&data[..100], &[]);
    assert!(matches!(peek_packets(&v, false, &mut out), Err(PageErr::Need(n)) if n > 100));
}
