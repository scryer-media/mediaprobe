//! Fuzz target: every Dolby Vision signal: dvcC/dvvC/dvwC records, block-addition mappings, the TS descriptor, RPU and EL NAL units (`inputs::dolby_vision`).
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::dolby_vision(data));
