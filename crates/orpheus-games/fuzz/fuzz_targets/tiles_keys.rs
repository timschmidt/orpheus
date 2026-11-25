#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_games_fuzz::tiles_keys(data));
