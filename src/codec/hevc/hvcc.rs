//! HEVC decoder configuration record (`hvcC`, ISO/IEC 14496-15 8.3.2.1).

/// Bytes before the NAL unit arrays.
const HEADER_LEN: usize = 23;

/// NAL unit types an `hvcC` array may hold: VPS, SPS, PPS and the prefix
/// and suffix SEI.
const ARRAY_TYPES: [u8; 5] = [32, 33, 34, 39, 40];
const SPS_NUT: u8 = 33;

/// The fields of an `hvcC` record the crate reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hvcc<'a> {
    pub(crate) general_profile_idc: u8,
    pub(crate) bit_depth_luma_minus8: u8,
    /// The NAL unit arrays, already checked to be well formed.
    arrays: &'a [u8],
    array_count: u8,
}

/// Parses a version 1 `hvcC` record. Refuses one that is truncated before
/// its last NAL unit, has a reserved NAL length size, or holds an array of
/// a NAL unit type a record may not carry. Bytes after the last array are
/// ignored.
pub(crate) fn parse(data: &[u8]) -> Option<Hvcc<'_>> {
    let (header, arrays) = data.split_first_chunk::<HEADER_LEN>()?;
    let [
        version,
        profile,
        ..,
        luma_depth,
        _,
        _,
        _,
        lengths,
        array_count,
    ] = *header;
    if version != 1 || lengths & 0x03 == 2 {
        return None;
    }
    let record = Hvcc {
        general_profile_idc: profile & 0x1F,
        bit_depth_luma_minus8: luma_depth & 0x07,
        arrays,
        array_count,
    };
    let mut walk = record.walk();
    for _ in 0..array_count {
        let (nal_type, nal_count) = walk.array()?;
        if !ARRAY_TYPES.contains(&nal_type) {
            return None;
        }
        for _ in 0..nal_count {
            walk.nal()?;
        }
    }
    Some(record)
}

impl<'a> Hvcc<'a> {
    fn walk(&self) -> Walk<'a> {
        Walk { rest: self.arrays }
    }

    /// The SPS NAL units, in record order.
    pub(crate) fn sps_nal_units(&self) -> impl Iterator<Item = &'a [u8]> + use<'a> {
        let mut walk = self.walk();
        let mut arrays_left = self.array_count;
        let mut current: Option<(bool, u16)> = None;
        std::iter::from_fn(move || {
            loop {
                match current {
                    Some((is_sps, nal_left)) if nal_left > 0 => {
                        current = Some((is_sps, nal_left - 1));
                        let nal = walk.nal()?;
                        if is_sps {
                            return Some(nal);
                        }
                    }
                    _ => {
                        arrays_left = arrays_left.checked_sub(1)?;
                        let (nal_type, nal_count) = walk.array()?;
                        current = Some((nal_type == SPS_NUT, nal_count));
                    }
                }
            }
        })
    }
}

/// A cursor over the NAL unit arrays.
struct Walk<'a> {
    rest: &'a [u8],
}

impl<'a> Walk<'a> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let (head, rest) = self.rest.split_first_chunk::<N>()?;
        self.rest = rest;
        Some(*head)
    }

    /// An array header: its NAL unit type and NAL unit count.
    fn array(&mut self) -> Option<(u8, u16)> {
        let [kind, count_high, count_low] = self.take()?;
        Some((kind & 0x3F, u16::from_be_bytes([count_high, count_low])))
    }

    /// One length-prefixed NAL unit.
    fn nal(&mut self) -> Option<&'a [u8]> {
        let length = usize::from(u16::from_be_bytes(self.take()?));
        let (nal, rest) = self.rest.split_at_checked(length)?;
        self.rest = rest;
        Some(nal)
    }
}
