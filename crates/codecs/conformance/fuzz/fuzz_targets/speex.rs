#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_codec_conformance::fuzz::speex(data));
