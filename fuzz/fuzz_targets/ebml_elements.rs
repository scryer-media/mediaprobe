//! Fuzz target: Matroska with an EBML element mutator: unknown and lying sizes, size widths, IDs, duplicated, dropped and reordered elements.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "mkv"));

libfuzzer_sys::fuzz_mutator!(|data: &mut [u8], size: usize, max_size: usize, seed: u32| {
    mediaprobe_fuzz::custom_mutate(
        mediaprobe_fuzz::ebml::mutate,
        data,
        size,
        max_size,
        seed,
        libfuzzer_sys::fuzzer_mutate,
    )
});
