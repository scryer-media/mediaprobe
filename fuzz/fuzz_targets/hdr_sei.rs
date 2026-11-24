//! Fuzz target: SEI messages: mastering display, content light level, HDR10+ and Dolby Vision T.35, captions, frame packing (`inputs::hdr_sei`).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::hdr_sei(data));
