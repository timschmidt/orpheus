#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_bt_fuzz::bt_tx(data));
