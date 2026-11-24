//! Fuzz target: read budgets on a 1 TiB sparse file whose head and tail are the input.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::budget(data));
