#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_img_fuzz::thumb(data));
