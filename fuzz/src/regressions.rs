//! Inputs that once broke the parser, as byte literals, each replayed by
//! `fuzz/tests/regressions.rs` through the target that found it. Add one
//! per crash, minimised, with the target and what it broke. A crash is
//! recorded when it is found and marked `open` until the parser is fixed.

/// Which target body replays a regression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// `targets::container` with this extension.
    Container(&'static str),
    Dispatch,
    CodecConfig,
    Bitstream,
    HdrSei,
    DolbyVision,
    Disc,
    SimdDifferential,
    Budget,
    ProbeDepths,
    MkvTags,
    ModelSerde,
}

pub struct Regression {
    pub name: &'static str,
    pub target: Target,
    /// What went wrong, for the person reading a failure.
    pub finding: &'static str,
    pub input: &'static [u8],
    /// The parser still fails on this input. The replay expects the
    /// failure, so the suite stays green while the finding is known, and
    /// fails the moment a fix lands, until this is set to `false`.
    pub open: bool,
}

pub const REGRESSIONS: &[Regression] = &[
    Regression {
        open: false,
        name: "hevc_sps_ctb_below_min_tb",
        target: Target::CodecConfig,
        finding: "scuffle-h265 0.2.2 subtracts min_tb_log2_size_y from ctb_log2_size_y \
                  unchecked (sps/mod.rs:303), so an SPS whose transform block exceeds its \
                  coding tree block panics on overflow. Found by the bit-flip robustness \
                  test on Matroska HEVC Main; minimised to the SPS alone in a Matroska \
                  hvcC. Closed: the SPS block sizes are read and checked before the SPS \
                  reaches the dependency.",
        // Selector 24: Matroska V_MPEGH/ISO/HEVC with this CodecPrivate, an
        // hvcC holding one SPS.
        input: &[
            0x18, 0x01, 0x01, 0x60, 0x00, 0x00, 0x00, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1E,
            0xF0, 0x00, 0xF8, 0xFD, 0xF8, 0xF8, 0x00, 0x00, 0x0F, 0x01, 0x21, 0x00, 0x01, 0x00,
            0x28, 0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03,
            0x00, 0x00, 0x03, 0x00, 0x1E, 0xA0, 0x20, 0x81, 0x05, 0x96, 0x56, 0xD4, 0xA4, 0xC2,
            0xF0, 0x16, 0x80, 0x80, 0x00, 0x00, 0x03, 0x00, 0x80, 0x00, 0x00, 0x0C, 0x04,
        ],
    },
    Regression {
        open: false,
        name: "h264_sps_crop_exceeds_height",
        target: Target::CodecConfig,
        finding: "scuffle-h264 0.2.2 subtracts the frame crop from the coded height \
                  unchecked (sps/mod.rs:720), so a crop larger than the picture panics on \
                  overflow. Found by the bit-flip robustness test on a 65x33 Matroska \
                  H.264 fixture; one flipped bit in the avcC. The crate now computes the picture size itself, checked.",
        // Selector 23: Matroska V_MPEG4/ISO/AVC with this CodecPrivate.
        input: &[
            0x17, 0x01, 0xF4, 0x00, 0x0A, 0xFF, 0xE1, 0x00, 0x1A, 0x67, 0xF4, 0x00, 0x0A, 0x91,
            0x96, 0x85, 0x7E, 0x10, 0x80, 0x30, 0x11, 0x00, 0x00, 0x03, 0x00, 0x01, 0x00, 0x00,
            0x03, 0x00, 0x30, 0x0F, 0x12, 0x26, 0xA0, 0x01, 0x00, 0x05, 0x68, 0xCE, 0x0F, 0x19,
            0x20, 0xFF, 0xF8, 0xF8, 0x00,
        ],
    },
    Regression {
        open: false,
        name: "h264_sps_crop_exceeds_width",
        target: Target::CodecConfig,
        finding: "scuffle-h264 0.2.2 subtracts the frame crop from the coded width \
                  unchecked (sps/mod.rs:731). Found by the codec_config fuzz target in \
                  the first short run. The crate now computes the picture size itself, checked.",
        input: &[
            0x00, 0x27, 0x51, 0xE0, 0x0B, 0x25, 0x2A, 0x3B, 0x3B, 0xB1, 0x13, 0x3B, 0xB1, 0x13,
            0x10, 0x01,
        ],
    },
    Regression {
        open: false,
        name: "h264_sps_width_in_mbs_overflow",
        target: Target::Container("hevc"),
        finding: "scuffle-h264 0.2.2 computes (pic_width_in_mbs_minus1 + 1) * 16 unchecked \
                  (sps/mod.rs:728), so a huge exp-Golomb width overflows. The input is \
                  an H.264 SPS behind a .hevc hint: the content sniffer takes it as H.264. \
                  Found by the elementary_hevc fuzz target. The crate now computes the picture size itself, checked.",
        input: &[
            0x00, 0x00, 0x00, 0x01, 0x67, 0x40, 0x01, 0x0C, 0xEA, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x02, 0xFF, 0xFF, 0xFF, 0xC5, 0x60, 0x00, 0x00, 0xE1, 0x79, 0x65, 0x8F,
            0x76, 0x2C, 0xA4, 0x71, 0xED, 0xB9, 0x3D, 0x11, 0x84, 0x1D, 0x91, 0xF3, 0x4B, 0xA1,
            0x68, 0x20, 0x97, 0x8F, 0xFF, 0xFC, 0xFF, 0xFF, 0xFF, 0x79, 0x65, 0x03, 0x00, 0x70,
            0x91, 0xFF, 0xFF, 0xFC, 0xFF, 0x64, 0x91,
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regressions_are_named_once_and_not_empty() {
        let mut names = std::collections::BTreeSet::new();
        for r in REGRESSIONS {
            assert!(names.insert(r.name), "{} is listed twice", r.name);
            assert!(
                !r.input.is_empty() && !r.finding.is_empty(),
                "{} is incomplete",
                r.name
            );
        }
    }
}
