//! Fuzz target: DVD and Blu-ray disc images (UDF and ISO 9660) through the path API.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::disc(data));
