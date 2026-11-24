#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_otp_fuzz::otp_uri(data));
