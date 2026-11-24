#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_player_fuzz::player_queue(data));
