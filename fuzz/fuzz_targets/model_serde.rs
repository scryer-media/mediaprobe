//! Fuzz target: stored analysis records: JSON that loads as `AnalysisDetails` round-trips unchanged.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::model_serde(data));
