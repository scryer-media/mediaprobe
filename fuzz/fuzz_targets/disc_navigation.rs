//! Fuzz target: disc images with mutation confined to IFO, index, movie-object, playlist and clip-information files.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| mediaprobe_fuzz::targets::disc(data));

libfuzzer_sys::fuzz_mutator!(|data: &mut [u8], size: usize, max_size: usize, seed: u32| {
    mediaprobe_fuzz::custom_mutate(
        mediaprobe_fuzz::nav::mutate,
        data,
        size,
        max_size,
        seed,
        libfuzzer_sys::fuzzer_mutate,
    )
});
