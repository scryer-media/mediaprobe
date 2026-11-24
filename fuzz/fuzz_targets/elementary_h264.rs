//! Fuzz target: raw H.264 Annex B, through the dispatcher with the `.h264` hint.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "h264"));
