#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_settings_fuzz::settings_mount(data));
