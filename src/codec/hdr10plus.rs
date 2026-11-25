//! HDR10+ (SMPTE ST 2094-40) detection in HEVC SEI and ITU-T T.35 payloads.

use crate::scan;

/// Scan an HEVC video frame (length-prefixed NAL units) for HDR10+ SEI metadata
/// (SMPTE ST 2094-40).
///
/// Returns `true` if a registered user data SEI with country_code 0xB5 (USA),
/// provider_code 0x003C (Samsung), and provider_oriented_code 0x0001 is found.
pub(crate) fn scan_hevc_frame_for_hdr10plus(frame: &[u8], nal_length_size: usize) -> bool {
    if !(1..=4).contains(&nal_length_size) {
        return false;
    }

    let mut offset = 0;
    // Each NAL unit asks for the first SEI header candidate from its start
    // on. The searches resume where the last one stopped, so the pass stays
    // linear in the frame however many NAL units it holds.
    let mut candidates = scan::ForwardSearch::new();
    while frame.len() - offset >= nal_length_size {
        let Some(candidate) = candidates.find(offset + nal_length_size, |from| {
            scan::find_hevc_sei_nal_header_candidate(frame, from)
                .map(|candidate| (candidate.offset, candidate))
                .ok_or(frame.len())
        }) else {
            return false;
        };
        let nal_len = read_be_length(&frame[offset..], nal_length_size);
        offset += nal_length_size;
        let Some(nal_end) = nal_end(offset, nal_len, frame.len()) else {
            break;
        };
        if candidate.offset >= nal_end {
            offset = nal_end;
            continue;
        }
        let nal_data = &frame[offset..nal_end];
        // HEVC NAL header is 2 bytes. nal_unit_type is bits 1-6 of byte 0.
        if nal_data.len() >= 3 && candidate.offset == offset {
            let nal_type = candidate.nal_type;
            // PREFIX_SEI_NUT = 39, SUFFIX_SEI_NUT = 40
            let rbsp = scan::h2645_unescape_rbsp(&nal_data[2..]);
            if (nal_type == 39 || nal_type == 40) && check_sei_for_hdr10plus(rbsp.as_ref()) {
                return true;
            }
        }
        offset = nal_end;
    }
    false
}

/// Where a NAL unit of `nal_len` bytes starting at `offset` ends, or `None`
/// when it is empty or runs past `frame_len`. The length is compared with
/// the bytes left rather than added to `offset`, so a four-byte length near
/// the top of the address space cannot wrap on a 32-bit target.
fn nal_end(offset: usize, nal_len: u32, frame_len: usize) -> Option<usize> {
    let left = frame_len.checked_sub(offset)?;
    let nal_len = usize::try_from(nal_len).ok()?;
    (nal_len != 0 && nal_len <= left).then(|| offset + nal_len)
}

/// Read a big-endian integer of `size` (at most four) bytes from the start
/// of `data`.
fn read_be_length(data: &[u8], size: usize) -> u32 {
    let mut val = 0_u32;
    for &b in &data[..size] {
        val = (val << 8) | u32::from(b);
    }
    val
}

/// Parse SEI message payloads and check for SMPTE ST 2094-40 (HDR10+).
fn check_sei_for_hdr10plus(sei_rbsp: &[u8]) -> bool {
    let mut offset = 0;
    while offset < sei_rbsp.len() {
        // Parse payload_type (ff_byte run + last_byte).
        let mut payload_type: u32 = 0;
        while offset < sei_rbsp.len() && sei_rbsp[offset] == 0xFF {
            payload_type = payload_type.saturating_add(255);
            offset += 1;
        }
        if offset >= sei_rbsp.len() {
            break;
        }
        payload_type = payload_type.saturating_add(u32::from(sei_rbsp[offset]));
        offset += 1;

        // Parse payload_size.
        let mut payload_size: u32 = 0;
        while offset < sei_rbsp.len() && sei_rbsp[offset] == 0xFF {
            payload_size = payload_size.saturating_add(255);
            offset += 1;
        }
        if offset >= sei_rbsp.len() {
            break;
        }
        payload_size = payload_size.saturating_add(u32::from(sei_rbsp[offset]));
        offset += 1;

        let Some(payload_end) = offset
            .checked_add(payload_size as usize)
            .filter(|end| *end <= sei_rbsp.len())
        else {
            break;
        };

        // user_data_registered_itu_t_t35, payload_type = 4
        if payload_type == 4 && payload_size >= 5 {
            let p = &sei_rbsp[offset..payload_end];
            if scan_itu_t35_payload_for_hdr10plus(p) {
                return true;
            }
        }

        offset = payload_end;
    }
    false
}

/// Check raw ITU-T T.35 payload bytes for HDR10+ metadata.
pub(crate) fn scan_itu_t35_payload_for_hdr10plus(payload: &[u8]) -> bool {
    scan::find_hdr10plus_itu_t35_candidate(payload, 0) == Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hevc_hdr10plus_scan_unescapes_sei_rbsp() {
        let mut sei_rbsp = vec![0x00, 0x00, 0x03, 0x04, 0x06];
        sei_rbsp.extend_from_slice(&[0xB5, 0x00, 0x3C, 0x00, 0x01, 0x04]);
        assert!(!check_sei_for_hdr10plus(&sei_rbsp));

        let nal_len = 2 + sei_rbsp.len();
        let mut frame = Vec::new();
        frame.extend_from_slice(&(nal_len as u16).to_be_bytes());
        frame.extend_from_slice(&[0x4E, 0x01]);
        frame.extend_from_slice(&sei_rbsp);

        assert!(scan_hevc_frame_for_hdr10plus(&frame, 2));
    }

    #[test]
    fn scan_hevc_frame_hdr10plus_found() {
        // Build a synthetic HEVC frame with a PREFIX_SEI NAL containing HDR10+ metadata.
        // NAL length prefix = 4 bytes.
        // NAL header: 0x4E (type=39 PREFIX_SEI, layer_id=0), 0x01 (temporal_id_plus1=1)
        // SEI: payload_type=4, payload_size=7, country=0xB5, provider=0x003C, oriented=0x0001, filler
        let sei_payload = [
            0x04, // payload_type = 4
            0x07, // payload_size = 7
            0xB5, // country_code (USA)
            0x00, 0x3C, // provider_code (Samsung)
            0x00, 0x01, // provider_oriented_code (HDR10+)
            0x04, 0x00, // application_identifier + filler
        ];
        let nal_header = [0x4E, 0x01]; // PREFIX_SEI_NUT
        let nal_len = (nal_header.len() + sei_payload.len()) as u32;
        let mut frame = Vec::new();
        frame.extend_from_slice(&nal_len.to_be_bytes());
        frame.extend_from_slice(&nal_header);
        frame.extend_from_slice(&sei_payload);
        assert!(scan_hevc_frame_for_hdr10plus(&frame, 4));
    }

    /// The frame walk as it was before its candidate searches resumed one
    /// another: a fresh search from every NAL unit.
    fn rescanning_reference(frame: &[u8], nal_length_size: usize) -> bool {
        if !(1..=4).contains(&nal_length_size) {
            return false;
        }
        let mut offset = 0;
        while offset + nal_length_size <= frame.len() {
            let Some(candidate) =
                scan::find_hevc_sei_nal_header_candidate(frame, offset + nal_length_size)
            else {
                return false;
            };
            let nal_len = read_be_length(&frame[offset..], nal_length_size) as usize;
            offset += nal_length_size;
            if nal_len == 0 || offset + nal_len > frame.len() {
                break;
            }
            if candidate.offset >= offset + nal_len {
                offset += nal_len;
                continue;
            }
            let nal_data = &frame[offset..offset + nal_len];
            if nal_data.len() >= 3 && candidate.offset == offset {
                let rbsp = scan::h2645_unescape_rbsp(&nal_data[2..]);
                if matches!(candidate.nal_type, 39 | 40) && check_sei_for_hdr10plus(rbsp.as_ref()) {
                    return true;
                }
            }
            offset += nal_len;
        }
        false
    }

    #[test]
    fn resumed_candidate_searches_match_a_search_per_nal_unit() {
        // Frames of short NAL units over an alphabet rich in SEI header
        // bytes, HDR10+ payload bytes and length bytes, so candidates fall
        // inside, between and across NAL units.
        let alphabet = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x07, 0x4E, 0x4F, 0x50, 0x51, 0xB5, 0x3C, 0xFF,
        ];
        let mut state = 0x2545_f491_u32;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let payload = [
            0x4E, 0x01, 0x04, 0x07, 0xB5, 0x00, 0x3C, 0x00, 0x01, 0x04, 0x00,
        ];
        let mut found = 0;
        for case in 0..4_000 {
            let nal_length_size = 1 + case % 4;
            let mut frame = Vec::new();
            for _ in 0..(next() % 12) {
                let sei = next() % 9 == 0;
                let len = if sei {
                    payload.len() - (next() % 2) as usize
                } else {
                    (next() % 9) as usize
                };
                let length = (len as u32).to_be_bytes();
                frame.extend_from_slice(&length[4 - nal_length_size..]);
                if sei {
                    frame.extend_from_slice(&payload[..len]);
                } else {
                    frame.extend((0..len).map(|_| alphabet[next() as usize % alphabet.len()]));
                }
            }
            if next() % 3 == 0 {
                frame.extend((0..next() % 4).map(|_| alphabet[next() as usize % alphabet.len()]));
            }
            let expected = rescanning_reference(&frame, nal_length_size);
            found += usize::from(expected);
            assert_eq!(
                scan_hevc_frame_for_hdr10plus(&frame, nal_length_size),
                expected,
                "{frame:02x?} with {nal_length_size}-byte lengths"
            );
        }
        assert!(found > 0, "the generated frames must include HDR10+ hits");
    }

    #[test]
    fn nal_bounds_do_not_wrap_at_the_top_of_the_address_space() {
        // A four-byte length past the frame's end must end the walk on every
        // target width: on a 32-bit target `offset + nal_len` used to wrap
        // to a small offset and loop over the same bytes.
        assert_eq!(nal_end(4, u32::MAX, 5), None);
        assert_eq!(nal_end(4, u32::MAX - 3, 5), None);
        assert_eq!(nal_end(usize::MAX - 2, 4, usize::MAX), None);
        assert_eq!(nal_end(usize::MAX - 4, 4, usize::MAX), Some(usize::MAX));
        assert_eq!(nal_end(4, 0, 5), None);
        assert_eq!(nal_end(4, 1, 5), Some(5));
        assert_eq!(nal_end(6, 1, 5), None);
        for frame in [
            [0xFF, 0xFF, 0xFF, 0xFC, 0x4E].as_slice(),
            &[0xFF, 0xFF, 0xFF, 0xFF, 0x4E, 0x01, 0x04],
        ] {
            assert!(!scan_hevc_frame_for_hdr10plus(frame, 4));
        }
    }

    #[test]
    fn sei_sizes_saturate_instead_of_overflowing() {
        let mut rbsp = vec![0xFF; 64];
        rbsp.push(0x04);
        rbsp.extend([0xFF; 16]);
        rbsp.push(0x05);
        assert!(!check_sei_for_hdr10plus(&rbsp));
    }

    #[test]
    fn scan_hevc_frame_hdr10plus_not_found() {
        // Frame with a non-SEI NAL (type=1, VCL slice)
        let nal_header = [0x02, 0x01]; // type=1 (TRAIL_R)
        let nal_data = [0x00; 8];
        let nal_len = (nal_header.len() + nal_data.len()) as u32;
        let mut frame = Vec::new();
        frame.extend_from_slice(&nal_len.to_be_bytes());
        frame.extend_from_slice(&nal_header);
        frame.extend_from_slice(&nal_data);
        assert!(!scan_hevc_frame_for_hdr10plus(&frame, 4));
    }
}
