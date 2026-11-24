//! Parameter sets the builders wrap fuzz input around, so a mutated SEI or
//! configuration record sits behind a stream the parser accepts.
//!
//! The NAL units come from the corpus's own synthetic fixtures (x264 and
//! x265 encoding a generated test pattern), emulation prevention included.

/// H.264 High profile SPS, 64x36 (from `codec/h264_high.mkv`).
pub const H264_SPS: [u8; 22] = [
    0x67, 0x64, 0x00, 0x0a, 0xac, 0xb4, 0x21, 0x36, 0x02, 0x20, 0x00, 0x00, 0x03, 0x00, 0x20, 0x00,
    0x00, 0x06, 0x01, 0xe2, 0x44, 0xd4,
];

/// H.264 PPS matching [`H264_SPS`].
pub const H264_PPS: [u8; 4] = [0x68, 0xee, 0x0f, 0xcb];

/// HEVC Main 10 VPS (from `codec/hevc_main10.mkv`).
pub const HEVC_VPS: [u8; 24] = [
    0x40, 0x01, 0x0c, 0x01, 0xff, 0xff, 0x02, 0x20, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03,
    0x00, 0x00, 0x03, 0x00, 0x1e, 0x95, 0x94, 0x09,
];

/// HEVC Main 10 SPS matching [`HEVC_VPS`].
pub const HEVC_SPS: [u8; 42] = [
    0x42, 0x01, 0x01, 0x02, 0x20, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03,
    0x00, 0x1e, 0xa0, 0x20, 0x81, 0x04, 0xd9, 0x65, 0x65, 0x4a, 0x4c, 0x2f, 0x01, 0x68, 0x08, 0x00,
    0x00, 0x03, 0x00, 0x08, 0x00, 0x00, 0x03, 0x00, 0xc0, 0x40,
];

/// HEVC PPS matching [`HEVC_SPS`].
pub const HEVC_PPS: [u8; 6] = [0x44, 0x01, 0xc0, 0x73, 0xc0, 0x89];

/// An `avcC` record carrying [`H264_SPS`] and [`H264_PPS`].
pub const H264_AVCC: [u8; 37] = {
    let mut out = [0u8; 37];
    out[0] = 1;
    out[1] = H264_SPS[1];
    out[2] = H264_SPS[2];
    out[3] = H264_SPS[3];
    out[4] = 0xff; // four-byte NAL lengths
    out[5] = 0xe1; // one SPS
    out[6] = 0;
    out[7] = H264_SPS.len() as u8;
    let mut i = 0;
    while i < H264_SPS.len() {
        out[8 + i] = H264_SPS[i];
        i += 1;
    }
    let at = 8 + H264_SPS.len();
    out[at] = 1; // one PPS
    out[at + 1] = 0;
    out[at + 2] = H264_PPS.len() as u8;
    let mut j = 0;
    while j < H264_PPS.len() {
        out[at + 3 + j] = H264_PPS[j];
        j += 1;
    }
    out
};

/// An `hvcC` record carrying [`HEVC_VPS`], [`HEVC_SPS`] and [`HEVC_PPS`].
pub fn hevc_hvcc() -> Vec<u8> {
    let mut out = vec![1u8];
    // general_profile_space..general_level_idc: the 12 profile-tier-level
    // bytes that follow the SPS's NAL header and its first byte.
    out.extend_from_slice(&unescape(&HEVC_SPS)[3..15]);
    out.extend_from_slice(&[0xf0, 0x00, 0xfc, 0xfd, 0xfa, 0xfa, 0x00, 0x00, 0x0f]);
    out.push(3); // three arrays
    for (kind, nal) in [
        (32u8, &HEVC_VPS[..]),
        (33, &HEVC_SPS[..]),
        (34, &HEVC_PPS[..]),
    ] {
        out.push(0x80 | kind);
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

/// Remove emulation-prevention bytes (`00 00 03` becomes `00 00`).
pub fn unescape(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// Insert emulation-prevention bytes so `payload` cannot contain a start code.
pub fn escape(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + payload.len() / 64);
    let mut zeros = 0;
    for &b in payload {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_round_trips_and_removes_start_codes() {
        for input in [&[0u8, 0, 0, 1, 0, 0, 3, 0, 0][..], &[0; 9], &[1, 2, 3]] {
            let escaped = escape(input);
            assert_eq!(unescape(&escaped), input);
            assert!(!escaped.windows(3).any(|w| w == [0, 0, 1] || w == [0, 0, 0]));
        }
    }

    #[test]
    fn records_carry_the_parameter_sets() {
        assert_eq!(&H264_AVCC[8..30], &H264_SPS);
        assert_eq!(&H264_AVCC[33..], &H264_PPS);
        let hvcc = hevc_hvcc();
        assert_eq!(hvcc[0], 1);
        assert_eq!(hvcc[22], 3);
        assert!(hvcc.windows(HEVC_SPS.len()).any(|w| w == HEVC_SPS));
    }
}
