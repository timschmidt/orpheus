#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_tts_fuzz::tts_lexicon(data));
