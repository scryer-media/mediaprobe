//! Fuzz target: Blu-ray transport stream (192-byte packets), through the dispatcher with the `.m2ts` hint.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "m2ts"));
