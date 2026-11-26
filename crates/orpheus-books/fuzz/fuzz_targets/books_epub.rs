#![no_main]
libfuzzer_sys::fuzz_target!(|data: &[u8]| orpheus_books_fuzz::books_epub(data));
