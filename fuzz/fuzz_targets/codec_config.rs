//! Fuzz target: codec configuration records (avcC, hvcC, av1C, vpcC, esds, dOps, CodecPrivate, colour and HDR boxes, TS descriptors) inside valid containers (`inputs::codec_config`).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::codec_config(data));
