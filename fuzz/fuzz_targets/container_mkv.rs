//! Fuzz target: Matroska, through the dispatcher with the `.mkv` hint.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "mkv"));
