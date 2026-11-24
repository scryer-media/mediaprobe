//! Fuzz target: Ogg, through the dispatcher with the `.ogg` hint.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "ogg"));
