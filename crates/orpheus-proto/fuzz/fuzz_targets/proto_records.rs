#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_proto_fuzz::proto_records(data));
