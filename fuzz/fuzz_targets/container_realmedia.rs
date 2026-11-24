//! Fuzz target: RealMedia, through the dispatcher with the `.rm` hint.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "rm"));
