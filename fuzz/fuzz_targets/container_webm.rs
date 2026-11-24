//! Fuzz target: WebM, through the dispatcher with the `.webm` hint.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "webm"));
