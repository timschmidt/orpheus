#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_midi_fuzz::midi_parse(data));
