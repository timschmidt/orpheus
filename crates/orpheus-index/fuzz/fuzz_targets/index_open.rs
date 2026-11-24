#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_index_fuzz::index_open(data));
