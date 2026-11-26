#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_seq_fuzz::seq_load(data));
