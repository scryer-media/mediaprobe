//! Fuzz target: MPEG transport stream, through the dispatcher with the `.ts` hint.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "ts"));
