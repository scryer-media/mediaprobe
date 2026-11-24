//! Fuzz target: transport streams with a packet mutator: PIDs, unit start, scrambling and adaptation control, continuity, PSI lengths and CRCs, packet order.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "ts"));

libfuzzer_sys::fuzz_mutator!(|data: &mut [u8], size: usize, max_size: usize, seed: u32| {
    mediaprobe_fuzz::custom_mutate(
        mediaprobe_fuzz::ts::mutate,
        data,
        size,
        max_size,
        seed,
        libfuzzer_sys::fuzzer_mutate,
    )
});
