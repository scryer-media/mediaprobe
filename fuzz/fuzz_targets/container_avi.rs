//! Fuzz target: AVI, through the dispatcher with the `.avi` hint.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "avi"));
