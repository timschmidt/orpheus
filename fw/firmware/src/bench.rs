//! `card bench [MiB] [hs [XX]]` (PERF-SD): the card's throughput through
//! the file system, measured in the card task. A test file of `MiB` MiB
//! (default 8, at most 256), `/.orpheus/test/bench.bin` (the folder the
//! suite's fixtures use), is written and read
//! back in 32 KiB pieces (the file system turns whole sectors of a cluster
//! run into one multi-block transfer straight into the piece), the data
//! checked, the file removed. The rates count the pieces' own time: the
//! card task rests 2 ms after each piece (thread mode must run: a 16 MiB
//! run without rests starved the UI into the watchdog, 2026-11-27). With
//! `hs` the card runs at its high-speed clock for the run
//! (`Card::high_speed`), with `hs XX` and the delay chains `XX` (hex,
//! `SD_CTL` bits 23:16; U-SD-1), and is re-initialised to the default
//! clock after.
//!
//! Reply: `bench w=<KB/s> r=<KB/s> mib=<n> bad=<words> clk=<level> sd=<kHz>`
//! or `bench failed <step> state=<SD_STATE> cmd=<index>`.

use atj2157_hal::time;
use orpheus_fs::{OpenMode, Volume};
use yoton_bsp::card::Card;

use crate::audio::reply;

const PIECE: usize = 32 * 1024;
const PATH: &str = "/.orpheus/test/bench.bin";

/// Word `i` of piece `k`: never the same twice in a file of ≤ 256 MiB.
fn word(k: u32, i: u32) -> u32 {
    (k << 13 | i) ^ 0xA5C3_0F96
}

fn fill(buf: &mut [u8], k: u32) {
    for (i, w) in buf.chunks_exact_mut(4).enumerate() {
        w.copy_from_slice(&word(k, i as u32).to_le_bytes());
    }
}

/// Words of `buf` that differ from piece `k`.
fn check(buf: &[u8], k: u32) -> u32 {
    buf.chunks_exact(4).enumerate().filter(|(i, w)| **w != word(k, *i as u32).to_le_bytes()).count() as u32
}

/// KB/s of `bytes` in `us`.
fn kbps(bytes: u64, us: u64) -> u64 {
    bytes * 1000 / us.max(1)
}

pub async fn card_bench(vol: &mut Volume<'static, Card>, arena: &mut [u8], args: &str) {
    let mut it = args.split_whitespace();
    let mib: u32 = it.next().and_then(|a| a.parse().ok()).unwrap_or(8).clamp(1, 256);
    let hs = it.next() == Some("hs");
    let delays = it.next().and_then(|t| u8::from_str_radix(t, 16).ok());
    let Some(buf) = arena.get_mut(..PIECE) else { return };
    let khz = if hs { vol.device().high_speed(50_000).await.unwrap_or(0) } else { 24_000 };
    if let Some(d) = delays {
        Card::set_timing(d);
    }
    let pieces = mib * (1024 * 1024 / PIECE as u32);
    let bytes = u64::from(pieces) * PIECE as u64;
    let mut failed: Option<&[u8]> = None;
    // Microseconds of the writes, then of the reads; words that differ.
    let mut us = [0u64; 2];
    let mut bad = 0u32;
    // One open file: written, rewound, read (each `Volume` call is a
    // future of its own in this state machine: the code window is full).
    match vol.open(PATH, OpenMode::Truncate).await {
        Ok(mut f) => {
            'run: for (phase, t) in us.iter_mut().enumerate() {
                f.set_position(0);
                for k in 0..pieces {
                    if phase == 0 {
                        fill(buf, k);
                    }
                    let t0 = time::now_us();
                    let ok = if phase == 0 {
                        vol.write(&mut f, buf).await.is_ok()
                    } else {
                        vol.read(&mut f, buf).await.ok() == Some(PIECE)
                    };
                    *t += time::now_us() - t0;
                    if !ok {
                        failed = Some(if phase == 0 { b"write" } else { b"read" });
                        break 'run;
                    }
                    if phase == 1 {
                        bad += check(buf, k);
                    }
                    embassy_time::Timer::after_millis(2).await;
                }
            }
            let _ = vol.close(f).await;
        }
        Err(_) => failed = Some(b"create"),
    }
    let (tw, tr) = (us[0], us[1]);
    let st = vol.device().stats();
    let _ = vol.remove(PATH).await;
    let _ = vol.flush().await;
    if hs && vol.device().reinit().await.is_err() {
        failed = failed.or(Some(b"reinit"));
    }
    let lvl = atj2157_hal::clock::current() as u64;
    reply(|w| {
        if let Some(step) = failed {
            w.s(b"bench failed ")
                .s(step)
                .s(b" state=")
                .u(u64::from(st.last_err_state))
                .s(b" cmd=")
                .u(u64::from(st.last_err_cmd));
            return;
        }
        w.s(b"bench w=")
            .u(kbps(bytes, tw))
            .s(b" r=")
            .u(kbps(bytes, tr))
            .s(b" mib=")
            .u(u64::from(mib))
            .s(b" bad=")
            .u(u64::from(bad))
            .s(b" clk=")
            .u(lvl)
            .s(b" sd=")
            .u(u64::from(khz));
    });
}

/// Blocks read per trial of [`hs_scan`], at each of [`SCAN_LBAS`].
const SCAN_BLOCKS: usize = 64;
/// Where [`hs_scan`] reads (spread over the card; read only).
const SCAN_LBAS: [u32; 4] = [0, 32_768, 1_048_576, 8_000_000];

/// A word hash of `buf` (FNV-1a over words).
fn hash(buf: &[u8]) -> u32 {
    buf.chunks_exact(4).fold(0x811C_9DC5u32, |h, w| {
        (h ^ u32::from_le_bytes([w[0], w[1], w[2], w[3]])).wrapping_mul(0x0100_0193)
    })
}

/// `card timing [kHz] [clks]` (U-SD-1, PERF-SD round 2): read-only
/// trials of the card's high-speed timing. The blocks at [`SCAN_LBAS`] are
/// read at the default clock first; then for every delay chain setting
/// (`SD_CTL` bits 23:16, RDELAY 0–15 × WDELAY 15, 8, 4, 0) the card is
/// re-initialised, switched to high speed at the fastest clock ≤ `kHz`
/// (default 50,000: 48 MHz), and the same blocks read again and compared.
/// With `clks`, `SD_EN.CLKS` is set too. The card ends re-initialised at
/// the default clock.
///
/// Reply: `hs scan sd=<kHz> clks=<0|1> w<W>:<16 marks>…` with one mark
/// per RDELAY: `.` same data, `x` different data, `e` a failed command,
/// `-` the card refused the switch, `r` the re-initialisation failed; then
/// `best=<delays> <KB/s>` for the first setting whose reads all matched
/// (the time of its reads).
pub async fn hs_scan(vol: &mut Volume<'static, Card>, arena: &mut [u8], args: &str) {
    let mut it = args.split_whitespace();
    let max_khz: u32 = it.next().and_then(|a| a.parse().ok()).unwrap_or(50_000);
    let clks = it.next() == Some("clks");
    let Some(buf) = arena.get_mut(..SCAN_BLOCKS * 512) else { return };
    let card = vol.device();
    let mut want = [0u32; SCAN_LBAS.len()];
    for (w, &lba) in want.iter_mut().zip(&SCAN_LBAS) {
        let (s, _) = buf.as_chunks_mut::<512>();
        if orpheus_fs::BlockDevice::read(card, lba, s).await.is_err() {
            reply(|w| {
                w.s(b"hs scan failed: reference read");
            });
            return;
        }
        *w = hash(buf);
    }
    const WDELAYS: [u8; 4] = [15, 8, 4, 0];
    let mut marks = [[b'?'; 16]; WDELAYS.len()];
    let (mut khz, mut best) = (0u32, None::<(u8, u64)>);
    for (row, &wd) in marks.iter_mut().zip(&WDELAYS) {
        for (rd, m) in row.iter_mut().enumerate() {
            let delays = (rd as u8) << 4 | wd;
            if card.reinit().await.is_err() {
                *m = b'r';
                continue;
            }
            khz = match card.high_speed(max_khz).await {
                Ok(k) if k > 25_000 => k,
                _ => {
                    *m = b'-';
                    continue;
                }
            };
            Card::set_timing(delays);
            Card::set_clks(clks);
            *m = b'.';
            let t0 = time::now_us();
            for (&w, &lba) in want.iter().zip(&SCAN_LBAS) {
                let (s, _) = buf.as_chunks_mut::<512>();
                if orpheus_fs::BlockDevice::read(card, lba, s).await.is_err() {
                    *m = b'e';
                    break;
                }
                if hash(buf) != w {
                    *m = b'x';
                }
            }
            if *m == b'.' && best.is_none() {
                best = Some((delays, time::now_us() - t0));
            }
            embassy_time::Timer::after_millis(2).await;
        }
    }
    let _ = card.reinit().await;
    reply(|w| {
        w.s(b"hs scan sd=").u(u64::from(khz)).s(b" clks=").u(u64::from(clks));
        for (row, &wd) in marks.iter().zip(&WDELAYS) {
            w.s(b" w").u(u64::from(wd)).s(b":").s(row);
        }
        if let Some((d, us)) = best {
            w.s(b" best=").u(u64::from(d)).s(b" ").u(kbps((SCAN_LBAS.len() * SCAN_BLOCKS * 512) as u64, us));
        }
    });
}
