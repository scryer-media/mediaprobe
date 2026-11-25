//! H.264 decoder configuration record (avcC, ISO/IEC 14496-15 5.3.3.1).

use super::sps::read_extension;
use crate::codec::bits::BitReader;

/// The fields of an avcC record the crate reads.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AvcConfig<'a> {
    pub configuration_version: u8,
    pub profile_indication: u8,
    /// The SPS NAL units, at most 31 (the count is 5 bits).
    pub sps: Vec<&'a [u8]>,
    pub pps_count: u8,
    /// From the high-profile extension, when the record carries one.
    pub bit_depth_luma_minus8: Option<u8>,
}

/// A forward-only reader over a byte slice.
struct Bytes<'a> {
    data: &'a [u8],
}

impl<'a> Bytes<'a> {
    fn u8(&mut self) -> Option<u8> {
        let (&byte, rest) = self.data.split_first()?;
        self.data = rest;
        Some(byte)
    }

    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.data.split_at_checked(len)?;
        self.data = rest;
        Some(head)
    }

    /// A 16-bit length followed by that many bytes.
    fn sized(&mut self) -> Option<&'a [u8]> {
        let len = u16::from_be_bytes([self.u8()?, self.u8()?]);
        self.take(usize::from(len))
    }
}

impl<'a> AvcConfig<'a> {
    /// Parses an avcC record. Returns `None` when a declared parameter set
    /// runs past the data, or a parameter set extension does not parse.
    /// Bytes after the last field read are ignored.
    pub(crate) fn parse(data: &'a [u8]) -> Option<Self> {
        let mut bytes = Bytes { data };
        let configuration_version = bytes.u8()?;
        let profile_indication = bytes.u8()?;
        bytes.u8()?; // profile_compatibility
        bytes.u8()?; // AVCLevelIndication
        bytes.u8()?; // lengthSizeMinusOne
        let sps_count = bytes.u8()? & 0x1F;
        let mut sps = Vec::new();
        for _ in 0..sps_count {
            sps.push(bytes.sized()?);
        }
        let pps_count = bytes.u8()?;
        for _ in 0..pps_count {
            bytes.sized()?;
        }

        // Some records leave the extension out for profiles that should
        // carry it; that is accepted.
        let mut bit_depth_luma_minus8 = None;
        if !matches!(profile_indication, 66 | 77 | 88) && !bytes.data.is_empty() {
            bytes.u8()?; // chroma_format
            bit_depth_luma_minus8 = Some(bytes.u8()? & 0x07);
            bytes.u8()?; // bit_depth_chroma_minus8
            for _ in 0..bytes.u8()? {
                read_extension(&mut BitReader::new(bytes.sized()?))?;
            }
        }

        Some(Self {
            configuration_version,
            profile_indication,
            sps,
            pps_count,
            bit_depth_luma_minus8,
        })
    }
}
