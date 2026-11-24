//! Fuzz target: FLV, through the dispatcher with the `.flv` hint.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "flv"));
