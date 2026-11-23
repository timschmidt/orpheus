//! `yoton-codec-conformance run <corpus> [--filter S] [--no-robust] [--seeks N]`
//! `yoton-codec-conformance fuzz-smoke <corpus> [--iters N]`
//!
//! Run by `cargo xtask test codecs` / `fuzz-smoke`. Prints one line per file
//! and a verdict line last; exits non-zero on any failure.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use yoton_codec_conformance::checks::{self, Entry, Opts, Row};
use yoton_codec_conformance::{fuzz, rng::Rng};

struct Args {
    cmd: String,
    dir: PathBuf,
    filter: Option<String>,
    robust: bool,
    seeks: usize,
    iters: usize,
}

fn args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().ok_or("usage: run|fuzz-smoke <corpus dir> [options]")?;
    let dir = PathBuf::from(it.next().ok_or("missing corpus directory")?);
    let mut a = Args { cmd, dir, filter: None, robust: true, seeks: 20, iters: 2000 };
    while let Some(o) = it.next() {
        match o.as_str() {
            "--filter" => a.filter = it.next(),
            "--no-robust" => a.robust = false,
            "--seeks" => a.seeks = it.next().and_then(|s| s.parse().ok()).ok_or("--seeks N")?,
            "--iters" => a.iters = it.next().and_then(|s| s.parse().ok()).ok_or("--iters N")?,
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(a)
}

fn parallel<T: Send + Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    let r = f(item);
                    if let Ok(mut o) = out.lock() {
                        o[i] = Some(r);
                    }
                }
            });
        }
    });
    out.into_inner().unwrap_or_default().into_iter().flatten().collect()
}

fn kib(b: usize) -> String {
    format!("{:.1}", b as f64 / 1024.0)
}

fn print_row(r: &Row) {
    let (fmt, frames) = match &r.info {
        Some(i) => (
            format!("{:?} {}Hz {}ch {}b", i.codec, i.rate, i.channels, i.bits),
            i.frames.map_or("?".into(), |f| f.to_string()),
        ),
        None => ("-".into(), "-".into()),
    };
    let status = if r.failures.is_empty() { "ok  " } else { "FAIL" };
    println!(
        "{status} {:<34} {:<26} frames {:<8} ref: {:<40} md5 {:<5} seek {:<14} robust {:<8} arena {}/{} KiB  {:.0} ns/frame {:.0}x",
        r.name,
        fmt,
        frames,
        r.reference,
        r.md5.map_or("-".to_string(), |m| m.to_string()),
        r.seek,
        r.robust,
        kib(r.arena_used),
        kib(r.budget),
        r.ns_per_frame,
        r.realtime
    );
    for f in &r.failures {
        println!("     - {f}");
    }
}

fn run(a: &Args) -> Result<bool, String> {
    let mut entries = checks::manifest(&a.dir)?;
    if let Some(f) = &a.filter {
        entries.retain(|e| e.name.contains(f.as_str()));
    }
    let opts = Opts { robust: a.robust, seeks: a.seeks };
    let rows = parallel(&entries, |e: &Entry| checks::check(e, opts));
    let mut by_codec: BTreeMap<String, (usize, usize, usize, f64)> = BTreeMap::new();
    let mut failed = 0;
    for r in &rows {
        print_row(r);
        if !r.failures.is_empty() {
            failed += 1;
        }
        let key = r.info.map_or_else(|| "refused".to_string(), |i| format!("{:?}", i.codec));
        let e = by_codec.entry(key).or_default();
        e.0 += 1;
        e.1 += usize::from(!r.failures.is_empty());
        e.2 = e.2.max(r.arena_used);
        if r.ns_per_frame > 0.0 {
            e.3 = e.3.max(r.ns_per_frame);
        }
    }
    println!();
    println!("codec        files  failed  arena high-water  slowest ns/frame (host)");
    for (k, (n, f, hw, ns)) in &by_codec {
        println!("{k:<12} {n:>5}  {f:>6}  {:>10} B      {ns:>8.0}", hw);
    }
    println!();
    println!("{} files, {} failed", rows.len(), failed);
    Ok(failed == 0)
}

fn fuzz_smoke(a: &Args) -> Result<bool, String> {
    let entries = checks::manifest(&a.dir)?;
    let mut seeds: Vec<Vec<u8>> = Vec::new();
    for e in &entries {
        if let Ok(mut f) = std::fs::read(&e.file) {
            // Short seeds keep the smoke run quick.
            f.truncate(48 * 1024);
            seeds.push(f);
        }
    }
    if seeds.is_empty() {
        return Err("no seeds".into());
    }
    let jobs: Vec<(fuzz::Target, usize)> =
        fuzz::TARGETS.iter().flat_map(|&t| (0..8).map(move |k| (t, k))).collect();
    let per = a.iters / 8 + 1;
    let seeds = &seeds;
    parallel(&jobs, |&((name, f), k)| {
        let mut rng = Rng::new(0xF022 ^ (k as u64) << 32 ^ name.len() as u64);
        for i in 0..per {
            let seed = &seeds[rng.below(seeds.len() as u64) as usize];
            let input = fuzz::mutate(seed, &mut rng);
            yoton_codec_conformance::set_context(&format!("fuzz {name} case {k}/{i}"));
            f(&input);
        }
    });
    println!("fuzz smoke: {} targets x {} mutated inputs, no failure", fuzz::TARGETS.len(), per * 8);
    Ok(true)
}

fn main() -> ExitCode {
    yoton_codec_conformance::install_panic_hook();
    let a = match args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let r = match a.cmd.as_str() {
        "run" => run(&a),
        "fuzz-smoke" => fuzz_smoke(&a),
        other => Err(format!("unknown command {other}")),
    };
    match r {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}
