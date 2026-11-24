#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_library_fuzz::library_open(data));
