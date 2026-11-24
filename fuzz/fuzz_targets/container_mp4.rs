//! Fuzz target: MP4, through the dispatcher with the `.mp4` hint.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "mp4"));
