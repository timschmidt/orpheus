#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| yoton_codec_conformance::fuzz::lyrics(data));
