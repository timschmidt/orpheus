#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_input_fuzz::input_samples(data));
