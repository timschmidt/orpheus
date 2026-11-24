# Profile the firmware

On this player most slowness is **paging**: code that is not in the 32 KiB
code cache is read from flash 1 KiB at a time (about 180 µs per line). The
tools below measure it; the fix is usually to pack the hot code together.

## Counters on the running player

```sh
cargo xtask dev stats          # uptime, idle time, pager loads and faults, UI and decode times, clock level
cargo xtask dev stats --irq    # interrupts per line
cargo xtask dev cmd status     # playback: underruns by cause, decoder µs and line loads per call (dec:)
```

Take two `stats` readings some seconds apart and divide: **line loads per
second** during playback is the number to watch (for example ≈ 2,650/s
during Opus playback with 0 underruns). Underruns in `status` are split by
cause (`fill`, `dec`, `serve`, …), which says which part of the loop ran
late.

## Packing hot code

[`hot.toml`](../../../hot.toml) lists **clusters**: ordered patterns of
symbol names (demangled globs) or section names. `cargo xtask pagelink`
places each cluster's code together, starting on a 1 KiB line, in the
pattern order, so a loop that fits about 24 lines runs without faults once
warm.

1. Find what runs per frame or per block: the decoder's frame loop, the
   card read path of a refill, the small functions called every block.
2. Add a cluster with those symbols, most frequent first. Look at
   `target/orpheus/firmware.map` to check where they landed.
3. Measure again on the player; keep the cluster only if line loads went
   down. Record before/after numbers in STATUS.

Watch for **inlining**: LLVM may inline a function into a large paged
caller, so a symbol you listed disappears and its code travels with the
caller. `#[inline(never)]` keeps it separate.

## Size and stack

```sh
cargo xtask size               # resident text/rodata/data/bss, paged size per crate, RAM by region
cargo xtask metrics            # appends this commit's numbers to metrics/*.csv
```

`cargo xtask dev info` shows how much stack stayed free (the high-water
mark); the 40 largest stack frames are in `metrics/static.csv`.

## On the host

Decoder speed without the pager: `cargo xtask test codecs` prints decode
times; the pager's behaviour can be replayed from traces in
`orpheus-pager`'s host tests. A PC-sampling profiler that rewrites
`hot.toml` from the device (`xtask dev prof`) is planned (T-8), not built.
