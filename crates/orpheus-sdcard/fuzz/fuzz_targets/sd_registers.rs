#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_sdcard_fuzz::sd_registers(data));
