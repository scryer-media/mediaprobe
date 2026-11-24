//! Fuzz target: MP4 with a box-tree mutator: lying sizes, swapped types, duplicated, dropped and reordered boxes.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::container(data, "mp4"));

libfuzzer_sys::fuzz_mutator!(|data: &mut [u8], size: usize, max_size: usize, seed: u32| {
    mediaprobe_fuzz::custom_mutate(
        mediaprobe_fuzz::boxes::mutate,
        data,
        size,
        max_size,
        seed,
        libfuzzer_sys::fuzzer_mutate,
    )
});
