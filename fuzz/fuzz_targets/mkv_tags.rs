//! Fuzz target: Matroska tag collection on a `Tags` element of many targets and simple tags.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::mkv_tags(data));
