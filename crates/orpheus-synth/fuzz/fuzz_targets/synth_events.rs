#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_synth_fuzz::synth_events(data));
