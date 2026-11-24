//! Fuzz target: the dispatcher with an arbitrary extension hint (`inputs::dispatch`).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::dispatch(data));
