//! Fuzz target: raw MPEG-1/2 video, through the dispatcher with the `.m2v` hint.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "m2v"));
