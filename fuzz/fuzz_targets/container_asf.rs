//! Fuzz target: ASF (WMV), through the dispatcher with the `.wmv` hint.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "wmv"));
