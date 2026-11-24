//! Fuzz target: every dispatched SIMD scanner against its scalar fallback.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::simd_differential(data));
