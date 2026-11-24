//! MPEG transport stream packet mutation (188-byte TS, 192-byte M2TS,
//! 204-byte TS with Reed-Solomon parity).
//!
//! Mutations act on packet fields the demuxer reads: PID, payload-unit start,
//! scrambling and adaptation-field control, continuity counter, adaptation
//! field length, and the PSI section length and CRC; or on whole packets
//! (duplicate, drop, swap). Bytes before the first sync stay untouched.

use crate::rng::{Rng, perturb};

pub const SIZES: [usize; 3] = [188, 192, 204];

/// Where the 188-byte packets start and how far apart they are, if at least
/// three consecutive packets line up.
pub fn layout(data: &[u8]) -> Option<(usize, usize)> {
    for size in SIZES {
        let lead = size - 188;
        for start in 0..size.min(data.len()) {
            let sync = start + lead;
            if (0..3).all(|k| data.get(sync + k * size) == Some(&0x47)) {
                return Some((start, size));
            }
        }
    }
    None
}

fn pids(data: &[u8], start: usize, size: usize) -> Vec<u16> {
    let mut out: Vec<u16> = data[start..]
        .chunks_exact(size)
        .map(|p| u16::from_be_bytes([p[size - 187] & 0x1F, p[size - 186]]))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

pub fn mutate(data: &[u8], seed: u64, max_size: usize) -> Option<Vec<u8>> {
    let (start, size) = layout(data)?;
    let lead = size - 188;
    let mut rng = Rng::new(seed);
    let mut out = data.to_vec();
    let known = pids(data, start, size);
    for _ in 0..=rng.below(3) {
        let count_now = (out.len() - start) / size;
        if count_now == 0 {
            break;
        }
        let at = start + rng.below(count_now) * size;
        let h = at + lead; // the sync byte
        match rng.below(9) {
            0 => {
                let pid = match rng.below(3) {
                    0 => *rng.pick(&known),
                    1 => *rng.pick(&[0u16, 1, 0x10, 0x11, 0x100, 0x1011, 0x1FFF]),
                    _ => rng.below(0x2000) as u16,
                };
                out[h + 1] = (out[h + 1] & 0xE0) | (pid >> 8) as u8;
                out[h + 2] = pid as u8;
            }
            1 => out[h + 1] ^= 0x40, // payload_unit_start_indicator
            2 => out[h + 3] = (out[h + 3] & 0x3F) | ((rng.below(4) as u8) << 6),
            3 => out[h + 3] = (out[h + 3] & 0xCF) | ((rng.below(4) as u8) << 4),
            4 => out[h + 3] = (out[h + 3] & 0xF0) | rng.below(16) as u8,
            5 => {
                // Adaptation field length at the edges.
                out[h + 3] |= 0x20;
                out[h + 4] = *rng.pick(&[0u8, 1, 182, 183, 184, 0xFF]);
            }
            6 => {
                // PSI: the section length and the CRC, behind the pointer field.
                if out[h + 1] & 0x40 != 0 {
                    let pointer = out[h + 4] as usize;
                    let section = h + 5 + pointer;
                    if section + 3 <= h + 188 {
                        match rng.below(3) {
                            0 => out[section + 1] ^= 1 << rng.below(4),
                            1 => out[section + 2] = rng.next_u64() as u8,
                            _ => perturb(&mut out[section..h + 188], &mut rng),
                        }
                    }
                }
            }
            7 => {
                let packet = out[at..at + size].to_vec();
                match rng.below(3) {
                    0 => {
                        out.splice(at..at, packet);
                    }
                    1 if count_now > 1 => {
                        out.drain(at..at + size);
                    }
                    _ => {
                        let other = start + rng.below(count_now) * size;
                        let b = out[other..other + size].to_vec();
                        out[at..at + size].copy_from_slice(&b);
                        out[other..other + size].copy_from_slice(&packet);
                    }
                }
            }
            _ => perturb(&mut out[h + 4..h + 188], &mut rng),
        }
    }
    out.truncate(max_size);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_finds_each_packet_size() {
        for size in SIZES {
            let mut data = vec![0u8; size * 4 + 5];
            for k in 0..4 {
                data[5 + k * size + size - 188] = 0x47;
            }
            assert_eq!(layout(&data), Some((5, size)), "size {size}");
        }
        assert_eq!(layout(b"no sync here"), None);
    }

    #[test]
    fn mutation_keeps_sync_and_is_deterministic() {
        let ts = crate::build::ts_with_es(
            0x1B,
            &[],
            &crate::build::annexb(&[&crate::params::H264_SPS[..], &crate::params::H264_PPS[..]]),
        );
        let mut changed = 0;
        for seed in 0..200 {
            let a = mutate(&ts, seed, 1 << 20).unwrap();
            assert_eq!(a, mutate(&ts, seed, 1 << 20).unwrap());
            assert!(a.chunks_exact(188).all(|p| p[0] == 0x47));
            changed += usize::from(a != ts);
        }
        assert!(
            changed > 150,
            "only {changed} of 200 seeds changed the input"
        );
    }
}
