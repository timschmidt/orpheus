mod assembled;
mod ranges;

use yoton_audio::View;

use super::*;

#[test]
fn crc_check_value() {
    // CRC-32/MPEG-2 without the final inversion and with init 0 is
    // CRC-32/BZIP2's core: check value of "123456789" for init 0, no xor.
    assert_eq!(crc_update(0, b"123456789"), 0x89A1_897F);
}

#[test]
fn header_parse_and_crc() {
    let mut p = b"OggS\0\x02".to_vec();
    p.extend_from_slice(&7u64.to_le_bytes());
    p.extend_from_slice(&0x55u32.to_le_bytes());
    p.extend_from_slice(&3u32.to_le_bytes());
    p.extend_from_slice(&[0; 4]);
    p.extend_from_slice(&[2, 255, 4]);
    p.extend_from_slice(&[9; 259]);
    let c = crc_update(0, &p);
    p[22..26].copy_from_slice(&c.to_le_bytes());
    // Split across the ring wrap anywhere: same header, same CRC.
    for cut in [0, 1, 22, 24, 27, 29, 100, p.len()] {
        let v = View::new(&p[..cut], &p[cut..]);
        let Parse::Page(h) = PageHeader::parse(&v) else { panic!("cut {cut}") };
        assert_eq!((h.serial, h.seq, h.granule, h.body_len, h.header_len), (0x55, 3, 7, 259, 29));
        assert!(h.bos() && !h.eos() && !h.continued());
        assert_eq!(page_crc(&v, h.total()), h.crc);
    }
    assert_eq!(PageHeader::parse(&View::new(&p[..28], &[])), Parse::Need(29));
    p[5] = 0x08;
    assert_eq!(PageHeader::parse(&View::new(&p, &[])), Parse::NotPage);
}
