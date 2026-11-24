//! Fuzz target: DVD VOB program stream, through the dispatcher with the `.vob` hint.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "vob"));
