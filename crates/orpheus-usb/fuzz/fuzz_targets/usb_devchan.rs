#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_usb_fuzz::usb_devchan(data));
