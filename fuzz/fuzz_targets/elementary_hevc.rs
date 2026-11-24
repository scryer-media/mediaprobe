//! Fuzz target: raw HEVC Annex B, through the dispatcher with the `.hevc` hint.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "hevc"));
