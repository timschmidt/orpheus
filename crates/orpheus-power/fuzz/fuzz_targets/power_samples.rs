#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_power_fuzz::power_samples(data));
