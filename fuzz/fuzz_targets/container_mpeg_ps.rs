//! Fuzz target: MPEG program stream, through the dispatcher with the `.mpg` hint.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "mpg"));
