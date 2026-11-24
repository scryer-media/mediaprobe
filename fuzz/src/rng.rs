//! A small deterministic generator for the mutators and builders. libFuzzer
//! hands each mutation a seed; everything derived from it must replay
//! exactly, so nothing here reads a clock or global state.

#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // splitmix64 of the seed, so seeds 0 and 1 do not start in lockstep.
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        Self((z ^ (z >> 31)) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform-enough index below `n`; `n` must be non-zero.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    pub fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// Values at the edges of 8-, 16-, 32- and 64-bit fields, the ones length and
/// count checks get wrong.
pub const INTERESTING: [u64; 18] = [
    0,
    1,
    2,
    7,
    8,
    0x7F,
    0x80,
    0xFF,
    0x100,
    0x7FFF,
    0xFFFF,
    0x1_0000,
    0x7FFF_FFFF,
    0x8000_0000,
    0xFFFF_FFFF,
    0x1_0000_0000,
    0x7FFF_FFFF_FFFF_FFFF,
    u64::MAX,
];

/// Overwrite up to 8 bytes at `at` with a big-endian interesting value, or
/// flip a few bits, staying inside `bytes`.
pub fn perturb(bytes: &mut [u8], rng: &mut Rng) {
    if bytes.is_empty() {
        return;
    }
    let at = rng.below(bytes.len());
    match rng.below(3) {
        0 => {
            let width = *rng.pick(&[1usize, 2, 4, 8]);
            let value = rng.pick(&INTERESTING).to_be_bytes();
            let end = (at + width).min(bytes.len());
            bytes[at..end].copy_from_slice(&value[8 - (end - at)..]);
        }
        1 => {
            for _ in 0..=rng.below(4) {
                let i = rng.below(bytes.len());
                bytes[i] ^= 1 << rng.below(8);
            }
        }
        _ => bytes[at] = rng.next_u64() as u8,
    }
}
