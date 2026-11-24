//! Mutation confined to disc navigation structures.
//!
//! A disc image is megabytes of video around a few kilobytes of navigation:
//! DVD IFO files (`DVDVIDEO-VMG`, `DVDVIDEO-VTS`) and Blu-ray `index.bdmv`
//! (`INDX`), `MovieObject.bdmv` (`MOBJ`), playlists (`MPLS`) and clip
//! information (`HDMV`). Byte-level mutation of a whole image almost never
//! touches them. This mutator finds those files by their magic numbers and
//! mutates only inside them, at field-aligned offsets, leaving the UDF and
//! ISO 9660 structures that lead the reader there intact.

use crate::rng::{Rng, perturb};

pub const MAGICS: [&[u8]; 6] = [
    b"DVDVIDEO-VMG",
    b"DVDVIDEO-VTS",
    b"INDX",
    b"MOBJ",
    b"MPLS",
    b"HDMV",
];

/// How far past a magic number a navigation file is assumed to reach.
pub const REGION: usize = 64 * 1024;

/// Navigation files start on a 2048-byte sector; only magics there count.
pub fn regions(data: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for sector in (0..data.len()).step_by(2048) {
        let tail = &data[sector..];
        if MAGICS.iter().any(|m| tail.starts_with(m)) {
            out.push((sector, (sector + REGION).min(data.len())));
        }
    }
    out
}

pub fn mutate(data: &[u8], seed: u64, max_size: usize) -> Option<Vec<u8>> {
    let regions = regions(data);
    if regions.is_empty() {
        return None;
    }
    let mut rng = Rng::new(seed);
    let mut out = data.to_vec();
    for _ in 0..=rng.below(4) {
        let (start, end) = *rng.pick(&regions);
        // Skip the magic itself most of the time: a wrong magic only tests
        // the first comparison.
        let from = if rng.chance(16) {
            start
        } else {
            (start + 12).min(end)
        };
        perturb(&mut out[from..end], &mut rng);
    }
    out.truncate(max_size);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_sector_aligned_magics_count_and_mutation_stays_inside() {
        let mut image = vec![0u8; 2048 * 40];
        image[2048 * 3..2048 * 3 + 12].copy_from_slice(b"DVDVIDEO-VTS");
        image[2048 * 5 + 7..2048 * 5 + 11].copy_from_slice(b"MPLS"); // unaligned
        assert_eq!(regions(&image), vec![(2048 * 3, 2048 * 3 + REGION)]);
        for seed in 0..100 {
            let out = mutate(&image, seed, usize::MAX).unwrap();
            assert_eq!(out.len(), image.len());
            assert_eq!(out[..2048 * 3], image[..2048 * 3]);
            assert_eq!(out[2048 * 3 + REGION..], image[2048 * 3 + REGION..]);
        }
        assert!(mutate(&[0; 4096], 1, usize::MAX).is_none());
    }
}
