//! Bit readers over byte slices, plus a writer for building test bitstreams.

pub(crate) struct AudioBitReader<'a> {
    data: &'a [u8],
    pub(crate) bit_pos: usize,
}

impl<'a> AudioBitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    pub(crate) fn read_bit(&mut self) -> Option<u8> {
        Some(self.read_bits(1)? as u8)
    }

    pub(crate) fn read_bits(&mut self, count: usize) -> Option<u32> {
        if count == 0 || count > 32 || self.bit_pos + count > self.data.len() * 8 {
            return None;
        }

        let mut value = 0_u32;
        for _ in 0..count {
            let byte_index = self.bit_pos / 8;
            let bit_index = 7 - (self.bit_pos % 8);
            value = (value << 1) | u32::from((self.data[byte_index] >> bit_index) & 0x01);
            self.bit_pos += 1;
        }
        Some(value)
    }

    pub(crate) fn peek_bits(&self, count: usize) -> Option<u32> {
        if count == 0 || count > 32 || self.bit_pos + count > self.data.len() * 8 {
            return None;
        }

        let mut value = 0_u32;
        for bit_pos in self.bit_pos..(self.bit_pos + count) {
            let byte_index = bit_pos / 8;
            let bit_index = 7 - (bit_pos % 8);
            value = (value << 1) | u32::from((self.data[byte_index] >> bit_index) & 0x01);
        }
        Some(value)
    }

    pub(crate) fn skip_bits(&mut self, count: usize) -> Option<()> {
        if self.bit_pos + count > self.data.len() * 8 {
            return None;
        }
        self.bit_pos += count;
        Some(())
    }

    pub(crate) fn bits_left(&self) -> usize {
        self.data.len() * 8 - self.bit_pos
    }

    pub(crate) fn read_aac_audio_object_type(&mut self) -> Option<u8> {
        let object_type = self.read_bits(5)? as u8;
        if object_type == 31 {
            Some(32 + self.read_bits(6)? as u8)
        } else {
            Some(object_type)
        }
    }

    pub(crate) fn read_aac_sample_rate(&mut self) -> Option<u32> {
        const AAC_SAMPLE_RATES: [u32; 16] = [
            96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025,
            8_000, 7_350, 0, 0, 0,
        ];

        let sample_rate_index = self.read_bits(4)? as usize;
        if sample_rate_index == 0xF {
            self.read_bits(24)
        } else {
            AAC_SAMPLE_RATES.get(sample_rate_index).copied()
        }
    }
}

pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    pub(crate) fn read_bit(&mut self) -> Option<u8> {
        Some(self.read_bits(1)? as u8)
    }

    pub(crate) fn read_bits(&mut self, count: usize) -> Option<u32> {
        if count == 0 || count > 32 || self.bit_pos + count > self.data.len() * 8 {
            return None;
        }

        let mut value = 0_u32;
        for _ in 0..count {
            let byte_index = self.bit_pos / 8;
            let bit_index = 7 - (self.bit_pos % 8);
            value = (value << 1) | u32::from((self.data[byte_index] >> bit_index) & 0x01);
            self.bit_pos += 1;
        }

        Some(value)
    }

    pub(crate) fn skip_bits(&mut self, count: usize) -> Option<()> {
        if self.bit_pos + count > self.data.len() * 8 {
            return None;
        }
        self.bit_pos += count;
        Some(())
    }

    /// Bits read so far.
    pub(crate) fn position(&self) -> usize {
        self.bit_pos
    }
}

/// Zero bits before the terminating one bit that no Exp-Golomb value of at
/// most 64 bits can have.
pub(crate) const EXP_GOLOMB_ZERO_RUN: usize = 64;

/// Whether `data`, from bit `from_bit` on, holds `run` or more zero bits in a
/// row followed by a one bit.
///
/// `scuffle-expgolomb` shifts its result once for each leading zero bit with
/// no limit, so a prefix of 64 zeros can leave it zero and its closing
/// subtraction then overflows. A parameter set with such a run anywhere an
/// Exp-Golomb value could start is refused before it reaches that reader.
/// Zeros that run to the end of the data are not counted: with no one bit
/// after them the reader stops at the end instead.
pub(crate) fn has_terminated_zero_run(data: &[u8], from_bit: usize, run: usize) -> bool {
    let mut zeros = 0_usize;
    for (index, &byte) in data.iter().enumerate().skip(from_bit / 8) {
        // Bits before `from_bit` count as ones.
        let byte = if index == from_bit / 8 {
            byte | !(0xFF_u8 >> (from_bit % 8))
        } else {
            byte
        };
        if byte == 0 {
            zeros += 8;
            continue;
        }
        if zeros + byte.leading_zeros() as usize >= run {
            return true;
        }
        zeros = byte.trailing_zeros() as usize;
    }
    false
}

#[cfg(test)]
pub(crate) struct TestBitWriter {
    pub(crate) bytes: Vec<u8>,
    bit_pos: usize,
}

#[cfg(test)]
impl TestBitWriter {
    pub(crate) fn new() -> Self {
        Self {
            bytes: Vec::new(),
            bit_pos: 0,
        }
    }

    pub(crate) fn write_bits(&mut self, value: u64, count: usize) {
        for shift in (0..count).rev() {
            let bit = ((value >> shift) & 1) as u8;
            if self.bit_pos.is_multiple_of(8) {
                self.bytes.push(0);
            }
            let byte_index = self.bit_pos / 8;
            self.bytes[byte_index] |= bit << (7 - (self.bit_pos % 8));
            self.bit_pos += 1;
        }
    }

    pub(crate) fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod zero_run_tests {
    use super::has_terminated_zero_run;

    #[test]
    fn a_zero_run_counts_only_when_a_one_bit_ends_it() {
        let mut data = vec![0x80];
        data.extend_from_slice(&[0; 8]);
        assert!(!has_terminated_zero_run(&data, 0, 64), "runs to the end");
        data.push(0x01);
        assert!(has_terminated_zero_run(&data, 0, 64));
        // Seven after the first one bit, 64 more, and seven before the last.
        assert!(has_terminated_zero_run(&data, 0, 78));
        assert!(!has_terminated_zero_run(&data, 0, 79));
    }

    #[test]
    fn bits_before_the_start_are_not_part_of_a_run() {
        // 63 zero bits after the first byte's one bit, then a one bit.
        let mut data = vec![0x00];
        data.extend_from_slice(&[0; 7]);
        data.push(0x40);
        assert!(has_terminated_zero_run(&data, 0, 64));
        assert!(has_terminated_zero_run(&data, 1, 64));
        assert!(!has_terminated_zero_run(&data, 2, 64));
        assert!(!has_terminated_zero_run(&data, 16, 64));
        assert!(!has_terminated_zero_run(&[], 0, 64));
        assert!(!has_terminated_zero_run(&[0x01], 40, 1));
    }
}
