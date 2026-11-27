//! `card bench [MiB] [hs]` (PERF-SD): the card's throughput through the
//! file system, measured in the card task. A test file of `MiB` MiB
//! (default 8, at most 256) under `/.orpheus/test/` is written and read
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
const DIR: &str = "/.orpheus/test";
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
    let _ = vol.create_dir(DIR).await;
    let pieces = mib * (1024 * 1024 / PIECE as u32);
    let bytes = u64::from(pieces) * PIECE as u64;
    let mut failed: Option<&[u8]> = None;
    let (mut tw, mut tr) = (0u64, 0u64);
    match vol.open(PATH, OpenMode::Truncate).await {
        Ok(mut f) => {
            for k in 0..pieces {
                fill(buf, k);
                let t = time::now_us();
                let r = vol.write(&mut f, buf).await;
                tw += time::now_us() - t;
                if r.is_err() {
                    failed = Some(b"write");
                    break;
                }
                embassy_time::Timer::after_millis(2).await;
            }
            if vol.close(f).await.is_err() || vol.flush().await.is_err() {
                failed = failed.or(Some(b"close"));
            }
        }
        Err(_) => failed = Some(b"create"),
    }
    let mut bad = 0u32;
    if failed.is_none() {
        match vol.open(PATH, OpenMode::Read).await {
            Ok(mut f) => {
                for k in 0..pieces {
                    let t = time::now_us();
                    let r = vol.read(&mut f, buf).await;
                    tr += time::now_us() - t;
                    if r.ok() != Some(PIECE) {
                        failed = Some(b"read");
                        break;
                    }
                    bad += check(buf, k);
                    embassy_time::Timer::after_millis(2).await;
                }
                let _ = vol.close(f).await;
            }
            Err(_) => failed = Some(b"open"),
        }
    }
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
