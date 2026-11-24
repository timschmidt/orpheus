# Add a fuzz target

Every parser, decoder, protocol handler and state machine that takes
outside data has a `cargo fuzz` target ([07 §3.4](../../plan/07-tooling-testing.md)).
Findings stay in the corpus forever as regression tests.

Each fuzz project is a small crate of its own beside the crate it tests,
for example `crates/orpheus-proto/fuzz/`:

```
crates/orpheus-proto/fuzz/
├── Cargo.toml              its own [workspace]; nightly only
├── src/lib.rs              the target bodies: plain functions
└── fuzz_targets/
    └── proto_records.rs    libfuzzer_sys::fuzz_target!(|d| body(d))
```

1. **Write the body** in `src/lib.rs` as `pub fn name(data: &[u8])`. It
   must not panic for any input and should check invariants with
   `assert!` (decoded length within bounds, a round trip that matches).
2. **Add the binary** in `fuzz_targets/` and a `[[bin]]` entry in the fuzz
   crate's `Cargo.toml` (with `test = false`, `doc = false`).
3. **Replay on stable.** The tested crate's `tests/fuzz_corpus.rs` runs
   every committed input through the same bodies in `cargo test`, so the
   corpus is checked without a nightly. Add the target there.
4. **Seeds.** Committed corpus: `tests/fuzz-corpus/<target>/seed-*`
   (rewritten by `cargo xtask test fuzz-seeds`), and a dictionary
   `tests/fuzz-corpus/<target>.dict` for formats with magic words.
5. **Run it** (needs a nightly and `cargo-fuzz`):

   ```sh
   cd crates/orpheus-proto/fuzz
   cargo +nightly fuzz run proto_records ../../../tests/fuzz-corpus/proto_records
   ```

   `cargo xtask test fuzz-smoke` runs every target for 60 s.
6. **A finding** is minimised (`cargo fuzz tmin`), fixed, and its input
   kept under `tests/regress/<target>/` with a name that says what it
   found. Record it in the findings table of STATUS.
7. **Coverage**: `cargo xtask cover --fuzz` shows what the corpus reaches.

The simulator's UI key-sequence target (`sim/fuzz`) works the same way;
its inputs are key scripts.
