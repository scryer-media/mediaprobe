//! Fuzz target: DV, through the dispatcher with the `.dv` hint.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "dv"));
