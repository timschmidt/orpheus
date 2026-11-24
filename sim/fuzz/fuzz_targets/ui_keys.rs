#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_sim_fuzz::ui_keys(data));
