//! Fuzz target: QuickTime, through the dispatcher with the `.mov` hint.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "mov"));
