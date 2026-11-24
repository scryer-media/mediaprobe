//! The fuzz harness for mediaprobe: input builders, structure-aware
//! mutators and target bodies shared by `fuzz_targets/` and the regression
//! test.
//!
//! Everything here, the target bodies included, also runs on stable Rust
//! (`cargo nextest run --manifest-path fuzz/Cargo.toml`), where libFuzzer
//! does not.

pub mod boxes;
pub mod build;
pub mod ebml;
pub mod inputs;
pub mod nav;
pub mod params;
pub mod regressions;
pub mod rng;
pub mod ts;

pub mod shim;
pub mod source;
pub mod targets;

/// A structure-aware mutator: the input, a seed and the size limit in; the
/// mutated input out, or `None` when the input lacks the structure.
pub type Mutator = fn(&[u8], u64, usize) -> Option<Vec<u8>>;

/// Glue for `libfuzzer_sys::fuzz_mutator!`. Three mutations in four are
/// structural; the fourth, and any input without the structure, goes to
/// libFuzzer's own byte-level mutator (`fallback`), so its strategies and
/// the dictionaries still apply.
pub fn custom_mutate(
    mutator: Mutator,
    data: &mut [u8],
    size: usize,
    max_size: usize,
    seed: u32,
    fallback: fn(&mut [u8], usize, usize) -> usize,
) -> usize {
    if !seed.is_multiple_of(4)
        && let Some(out) = mutator(&data[..size], seed as u64, max_size)
    {
        let n = out.len().min(max_size).min(data.len());
        data[..n].copy_from_slice(&out[..n]);
        return n;
    }
    fallback(data, size, max_size)
}

#[cfg(test)]
mod tests {
    #[test]
    fn custom_mutate_uses_the_structure_or_falls_back() {
        fn grow(data: &[u8], _: u64, _: usize) -> Option<Vec<u8>> {
            (!data.is_empty()).then(|| [data, b"+"].concat())
        }
        fn fallback(_: &mut [u8], size: usize, _: usize) -> usize {
            size.saturating_sub(1)
        }
        let mut buf = [0u8; 16];
        buf[..3].copy_from_slice(b"abc");
        assert_eq!(super::custom_mutate(grow, &mut buf, 3, 16, 1, fallback), 4);
        assert_eq!(&buf[..4], b"abc+");
        assert_eq!(super::custom_mutate(grow, &mut buf, 4, 16, 4, fallback), 3);
        assert_eq!(super::custom_mutate(grow, &mut buf, 0, 16, 1, fallback), 0);
    }
}
