//! The fuzz targets' committed corpora and regression inputs, replayed on
//! stable through the same bodies the cargo-fuzz targets run (VER-4); and
//! the seed inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-usb --test
//! fuzz_corpus seeds` rewrites `tests/fuzz-corpus/<target>/seed-*`).

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;

#[test]
fn usb_scsi_corpus() {
    replay::replay("usb_scsi", bodies::usb_scsi);
}

#[test]
fn usb_devchan_corpus() {
    replay::replay("usb_devchan", bodies::usb_devchan);
}

/// Operation encoder for `usb_scsi`.
struct Script(Vec<u8>);

impl Script {
    fn new(sel: u8) -> Self {
        Script(vec![sel])
    }
    fn cmd_in(&mut self, len: u16, cdb: &[u8]) -> &mut Self {
        self.0.push(0x80 | (cdb.len() as u8 - 1));
        self.0.extend_from_slice(&len.to_le_bytes());
        self.0.extend_from_slice(cdb);
        self
    }
    /// OUT command with `packets` full data packets.
    fn cmd_out(&mut self, len: u16, cdb: &[u8], packets: u8) -> &mut Self {
        self.0.push(cdb.len() as u8 - 1);
        self.0.extend_from_slice(&len.to_le_bytes());
        self.0.extend_from_slice(cdb);
        self.0.push(packets);
        self.0.extend(std::iter::repeat_n(0, usize::from(packets)));
        self
    }
    fn raw(&mut self, b: &[u8]) -> &mut Self {
        self.0.extend_from_slice(&[0xC0, b.len() as u8]);
        self.0.extend_from_slice(b);
        self
    }
}

/// The commands a host sends after attaching a disk.
fn disk_session(sel: u8) -> Vec<u8> {
    let mut s = Script::new(sel);
    s.cmd_in(36, &[0x12, 0, 0, 0, 36, 0])
        .cmd_in(0, &[0x00, 0, 0, 0, 0, 0])
        .cmd_in(18, &[0x03, 0, 0, 0, 18, 0])
        .cmd_in(8, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0])
        .cmd_in(252, &[0x23, 0, 0, 0, 0, 0, 0, 0, 252, 0])
        .cmd_in(192, &[0x1A, 0, 0x3F, 0, 192, 0])
        .cmd_in(8, &[0x5A, 0, 0x3F, 0, 0, 0, 0, 0, 8, 0])
        .cmd_in(1024, &[0x28, 0, 0, 0, 0, 2, 0, 0, 2, 0])
        .cmd_out(1024, &[0x2A, 0, 0, 0, 0, 3, 0, 0, 2, 0], 2)
        .cmd_in(512, &[0xA8, 0, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0])
        .cmd_in(0, &[0x35, 0, 0, 0, 0, 0, 0, 0, 0, 0])
        .cmd_in(0, &[0x2F, 0, 0, 0, 0, 0, 0, 0, 1, 0])
        .cmd_in(0, &[0x1E, 0, 0, 0, 1, 0])
        .cmd_in(16, &[0xA0, 0, 0, 0, 0, 0, 0, 0, 0, 16, 0, 0])
        .cmd_in(0, &[0x1B, 0, 0, 0, 2, 0]);
    s.0
}

fn cd_session(sel: u8) -> Vec<u8> {
    let mut s = Script::new(sel);
    s.cmd_in(36, &[0x12, 0, 0, 0, 36, 0])
        .cmd_in(8, &[0x4A, 1, 0, 0, 0x10, 0, 0, 0, 8, 0])
        .cmd_in(8, &[0x46, 0, 0, 0, 0, 0, 0, 0, 8, 0])
        .cmd_in(0x100, &[0x46, 2, 0, 0, 0, 0, 0, 1, 0, 0])
        .cmd_in(12, &[0x43, 0, 0, 0, 0, 0, 0, 0, 12, 0])
        .cmd_in(804, &[0x43, 2, 2, 0, 0, 0, 0, 3, 0x24, 0])
        .cmd_in(12, &[0x43, 0, 1, 0, 0, 0, 0, 0, 12, 0])
        .cmd_in(34, &[0x51, 0, 0, 0, 0, 0, 0, 0, 34, 0])
        .cmd_in(28, &[0x52, 1, 0, 0, 0, 1, 0, 0, 28, 0])
        .cmd_in(8, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0])
        .cmd_in(4096, &[0x28, 0, 0, 0, 0, 16, 0, 0, 2, 0])
        .cmd_in(2052, &[0xAD, 0, 0, 0, 0, 0, 0, 0, 8, 4, 0, 0])
        .cmd_in(8, &[0xBD, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0])
        .cmd_in(20, &[0x5A, 0, 0x2A, 0, 0, 0, 0, 0, 20, 0])
        .cmd_in(8, &[0xAC, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0])
        .cmd_in(0, &[0x1B, 0, 0, 0, 2, 0]);
    s.0
}

fn scsi_seeds() -> Vec<(String, Vec<u8>)> {
    let mut v = vec![
        ("disk".to_string(), disk_session(0x10)),
        ("disk-ro".to_string(), disk_session(0x11)),
        ("absent".to_string(), disk_session(0x12)),
        ("disk-fs".to_string(), disk_session(0x80)),
        ("cd".to_string(), cd_session(3)),
        ("dvd".to_string(), cd_session(4)),
    ];
    let mut s = Script::new(0);
    s.raw(b"USBC garbage").cmd_in(11, &[0xCC, 0, 0, 0, 0, 0, 0, 11, 0, 0, 0, 0]).cmd_in(2, &[0xCB, 0x21]);
    s.cmd_in(512, &[0x28, 0, 0, 0, 0, 63, 0, 0, 2, 0]).cmd_out(
        512,
        &[0x2A, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 1, 0],
        1,
    );
    s.cmd_in(512, &[0x28, 0, 0, 0, 0, 0, 0, 0, 2, 0]).cmd_out(0, &[0x12, 0, 0, 0, 36, 0], 0);
    v.push(("phase-errors".to_string(), s.0));
    v
}

fn setup(rt: u8, req: u8, value: u16, index: u16, length: u16) -> Vec<u8> {
    let mut b = vec![rt, req];
    b.extend_from_slice(&value.to_le_bytes());
    b.extend_from_slice(&index.to_le_bytes());
    b.extend_from_slice(&length.to_le_bytes());
    b
}

fn devchan_seeds() -> Vec<(String, Vec<u8>)> {
    use orpheus_proto::devchan as dc;
    let mut v = Vec::new();
    for dev in [0u8, 1] {
        let mut d = vec![dev];
        let mut i = |req: u8, value: u16, index: u16, length: u16, buf4: u8| {
            d.extend(setup(0xC0, req, value, index, length));
            d.push(buf4);
        };
        i(dc::REQ_INFO, 0, 0, 32, 16);
        i(dc::REQ_LOG, 0, 0, 64, 16);
        i(dc::REQ_LOG, 2000, 0, 64, 16);
        i(dc::REQ_FRAME, 0x7FC0, 0, 64, 16);
        i(dc::REQ_STATS, 3, 0, 64, 16);
        i(dc::REQ_TIME, 0, 0, 7, 2);
        i(dc::REQ_TEST, 0, 0, 64, 16);
        i(dc::REQ_PEEK, 0x1000, 0x0010, 16, 4);
        i(dc::REQ_PEEK, 0x0003, 0, 16, 4);
        let mut o = |req: u8, value: u16, data: &[u8]| {
            d.extend(setup(0x40, req, value, 0, data.len() as u16));
            d.push(data.len() as u8);
            d.extend_from_slice(data);
        };
        o(dc::REQ_KEYS, 0, &[1, 1, 1, 0, 4, 1, 4]);
        o(dc::REQ_TIME, 0, &[0xEA, 0x07, 11, 24, 12, 41, 0]);
        o(dc::REQ_TEST, 0, b"bt status");
        o(dc::REQ_PUT, 0, &[0x55; 40]);
        o(dc::REQ_REBOOT, dc::REBOOT_NORMAL, &[]);
        // Not ours: standard and class requests.
        d.extend(setup(0x80, 6, 0x0100, 0, 18));
        d.push(5);
        d.extend(setup(0x21, 0xFF, 0, 0, 0));
        d.push(0);
        v.push((format!("requests-{dev}"), d));
    }
    v
}

#[test]
fn seeds() {
    let s = scsi_seeds();
    for (_, d) in &s {
        bodies::usb_scsi(d);
    }
    let d = devchan_seeds();
    for (_, x) in &d {
        bodies::usb_devchan(x);
    }
    replay::write_seeds("usb_scsi", &s);
    replay::write_seeds("usb_devchan", &d);
}
