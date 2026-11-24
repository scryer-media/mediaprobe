//! Fuzz target: H.264 and HEVC NAL units, MPEG video and AV1 OBUs in raw, Matroska, MP4 and TS carriers (`inputs::bitstream`).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::bitstream(data));
