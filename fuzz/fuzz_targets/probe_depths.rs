//! Fuzz target: `probe_source` end to end at each `ProbeDepth`, the first byte picking the depth.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::probe_depths(data));
