//! Deterministic bitstream authoring for signals no encoder in the toolchain
//! writes from the command line: SEI messages and Dolby Vision RPU NAL units
//! in H.264/HEVC, ITU-T T.35 metadata OBUs in AV1, A/53 caption user data in
//! MPEG-2 video, and scrambling markers in MPEG transport and program
//! streams.
//!
//! Every pass works on an elementary stream (or a finished TS/PS file) in the
//! recipe's work directory; FFmpeg then muxes the result with `-c copy`, so
//! the containers around the authored units are FFmpeg's own.

use crate::corpus::{Result, error, fail};

/// What to add to every access unit of an H.264/HEVC Annex B stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NalInsert {
    /// SEI user_data_registered_itu_t_t35 carrying ATSC A/53 cc_data with
    /// a CEA-608 CC1 pair and a CEA-708 service 1 packet.
    Captions,
    /// SEI mastering display colour volume (137) and content light level (144).
    StaticHdr,
    /// SEI user_data_registered_itu_t_t35 with SMPTE ST 2094-40 (HDR10+).
    Hdr10Plus,
    /// A Dolby Vision RPU in an HEVC UNSPEC62 NAL unit after the last VCL
    /// unit of the access unit.
    DoviRpu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NalCodec {
    H264,
    Hevc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Inject {
    AnnexB {
        codec: NalCodec,
        inserts: Vec<NalInsert>,
    },
    /// Insert an HDR10+ ITU-T T.35 metadata OBU after every temporal
    /// delimiter of a low-overhead AV1 OBU stream.
    Av1Hdr10Plus,
    /// Insert A/53 caption user data before the first slice of every MPEG-2 picture.
    Mpeg2Captions,
    /// Set transport_scrambling_control on every elementary-stream packet and
    /// replace its payload with keystream-masked bytes, as a real scrambler
    /// leaves it: nothing in the payload is readable.
    TsScramble { packet_size: usize },
    /// Set PES_scrambling_control on every video PES packet and mask its
    /// payload the same way.
    PsScramble,
    /// Add a DOVI video stream descriptor (tag 0xB0) to the PMT entry of the
    /// first video stream of a 188-byte TS. FFmpeg 8.1.1's TS muxer writes
    /// none, whatever side data the source carries.
    TsDoviDescriptor { record: super::post::DoviRecord },
}

pub(crate) fn apply(inject: &Inject, data: &[u8]) -> Result<Vec<u8>> {
    match inject {
        Inject::AnnexB { codec, inserts } => annex_b(data, *codec, inserts),
        Inject::Av1Hdr10Plus => av1_hdr10plus(data),
        Inject::Mpeg2Captions => mpeg2_captions(data),
        Inject::TsScramble { packet_size } => ts_scramble(data, *packet_size),
        Inject::PsScramble => ps_scramble(data),
        Inject::TsDoviDescriptor { record } => ts_dovi_descriptor(data, *record),
    }
}

// ---------------------------------------------------------------------------
// Bit writing and payloads
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    used: u8,
}

impl Bits {
    fn put(&mut self, value: u64, width: u32) {
        for bit in (0..width).rev() {
            if self.used == 0 {
                self.bytes.push(0);
            }
            if (value >> bit) & 1 == 1 {
                let last = self.bytes.len() - 1;
                self.bytes[last] |= 0x80 >> self.used;
            }
            self.used = (self.used + 1) % 8;
        }
    }

    /// Unsigned Exp-Golomb.
    fn ue(&mut self, value: u64) {
        let code = value + 1;
        let width = 64 - code.leading_zeros();
        self.put(0, width - 1);
        self.put(code, width);
    }

    /// Signed Exp-Golomb.
    fn se(&mut self, value: i64) {
        let mapped = if value > 0 {
            2 * value as u64 - 1
        } else {
            2 * value.unsigned_abs()
        };
        self.ue(mapped);
    }

    /// Zero bits up to the next byte boundary.
    fn align(&mut self) {
        if self.used != 0 {
            self.put(0, u32::from(8 - self.used));
        }
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

/// SMPTE ST 2094-40 application 4, version 1: one window, a 400 cd/m²
/// target display, MaxSCL 1000, nine distribution percentiles, no
/// tone-mapping curve.
pub(crate) fn hdr10plus_t35() -> Vec<u8> {
    let mut bits = Bits::default();
    for (value, width) in [(0xB5, 8), (0x003C, 16), (0x0001, 16), (4, 8), (1, 8)] {
        bits.put(value, width);
    }
    bits.put(1, 2); // num_windows
    bits.put(400, 27); // targeted_system_display_maximum_luminance
    bits.put(0, 1); // targeted_system_display_actual_peak_luminance_flag
    for _ in 0..3 {
        bits.put(100_000, 17); // maxscl, 0.00001 cd/m² units / 10
    }
    bits.put(10_000, 17); // average_maxrgb
    let percentiles: [(u64, u64); 9] = [
        (1, 0),
        (5, 100),
        (10, 200),
        (25, 400),
        (50, 1000),
        (75, 2000),
        (90, 5000),
        (95, 8000),
        (99, 10_000),
    ];
    bits.put(percentiles.len() as u64, 4);
    for (percentage, value) in percentiles {
        bits.put(percentage, 7);
        bits.put(value, 17);
    }
    bits.put(0, 10); // fraction_bright_pixels
    bits.put(0, 1); // mastering_display_actual_peak_luminance_flag
    bits.put(0, 1); // tone_mapping_flag
    bits.put(0, 1); // color_saturation_mapping_flag
    bits.finish()
}

/// ATSC A/53 user data (after the T.35 prefix or MPEG-2 `GA94`): type 3,
/// four cc triplets: a CEA-608 field-1 "resume caption loading" pair (CC1),
/// then a CEA-708 DTVCC packet holding one byte of service 1.
fn a53_cc_data() -> Vec<u8> {
    let odd = |b: u8| {
        b | if (b & 0x7f).count_ones().is_multiple_of(2) {
            0x80
        } else {
            0
        }
    };
    // cc_valid=1, cc_type: 0 = 608 field 1, 3 = DTVCC packet start, 2 = DTVCC data.
    let triplets: [[u8; 3]; 3] = [
        [0xFC, odd(0x14), odd(0x20)],
        // packet_size_code 1 => 1 byte header + 1 byte data: sequence 0, size 1.
        [0xFF, 0x01, 0x21],
        // service block: service 1, block size 1, then one data byte.
        [0xFE, 0x41, 0x20],
    ];
    let mut out = vec![0x03, 0x40 | triplets.len() as u8, 0xFF];
    for triplet in triplets {
        out.extend_from_slice(&triplet);
    }
    out.push(0xFF);
    out
}

fn a53_t35() -> Vec<u8> {
    let mut out = vec![0xB5, 0x00, 0x31, b'G', b'A', b'9', b'4'];
    out.extend(a53_cc_data());
    out
}

/// SMPTE ST 2086 in SEI order (G, B, R, white; 0.00002 units), 1000 / 0.005 cd/m².
fn mastering_display() -> Vec<u8> {
    let mut out = Vec::new();
    for value in [
        13_250u16, 34_500, 7_500, 3_000, 34_000, 16_000, 15_635, 16_450,
    ] {
        out.extend_from_slice(&value.to_be_bytes());
    }
    out.extend_from_slice(&10_000_000u32.to_be_bytes());
    out.extend_from_slice(&50u32.to_be_bytes());
    out
}

fn content_light() -> Vec<u8> {
    [1000u16.to_be_bytes(), 400u16.to_be_bytes()].concat()
}

fn sei_message(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![kind];
    let mut size = payload.len();
    while size >= 255 {
        out.push(255);
        size -= 255;
    }
    out.push(size as u8);
    out.extend_from_slice(payload);
    out
}

/// Insert emulation-prevention bytes into an RBSP.
fn escape(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + 8);
    let mut zeros = 0;
    for &byte in rbsp {
        if zeros >= 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    out
}

fn sei_nal(codec: NalCodec, messages: &[Vec<u8>]) -> Vec<u8> {
    let mut rbsp: Vec<u8> = messages.concat();
    rbsp.push(0x80);
    let mut nal = match codec {
        NalCodec::H264 => vec![0x06],
        // prefix SEI (39), layer 0, temporal id 1.
        NalCodec::Hevc => vec![39 << 1, 0x01],
    };
    nal.extend(escape(&rbsp));
    nal
}

/// A complete profile-8 style Dolby Vision RPU with an identity mapping:
/// 10-bit base layer, 12-bit VDR, one linear piece per component
/// (f(x) = x), no residual layer and no display-management metadata,
/// followed by CRC-32/MPEG-2 and the 0x80 terminator.
pub(crate) fn dovi_rpu_nal() -> Vec<u8> {
    const DENOM: u32 = 23;
    let mut bits = Bits::default();
    bits.put(0x19, 8); // rpu_nal_prefix
    bits.put(2, 6); // rpu_type
    bits.put(18, 11); // rpu_format
    bits.put(1, 4); // vdr_rpu_profile
    bits.put(0, 4); // vdr_rpu_level
    bits.put(1, 1); // vdr_seq_info_present_flag
    bits.put(0, 1); // chroma_resampling_explicit_filter_flag
    bits.put(0, 2); // coefficient_data_type: fixed point
    bits.ue(u64::from(DENOM)); // coefficient_log2_denom
    bits.put(1, 2); // vdr_rpu_normalized_idc
    bits.put(0, 1); // bl_video_full_range_flag
    bits.ue(2); // bl_bit_depth_minus8
    bits.ue(2); // el_bit_depth_minus8
    bits.ue(4); // vdr_bit_depth_minus8
    bits.put(0, 1); // spatial_resampling_filter_flag
    bits.put(0, 3); // reserved_zero_3bits
    bits.put(0, 1); // el_spatial_resampling_filter_flag
    bits.put(1, 1); // disable_residual_flag
    bits.put(0, 1); // vdr_dm_metadata_present_flag
    bits.put(0, 1); // use_prev_vdr_rpu_flag
    bits.ue(0); // vdr_rpu_id
    bits.ue(0); // mapping_color_space
    bits.ue(0); // mapping_chroma_format_idc
    for _ in 0..3 {
        bits.ue(0); // num_pivots_minus2
        bits.put(0, 10); // pred_pivot_value[0]
        bits.put(1023, 10); // pred_pivot_value[1] (delta)
    }
    bits.ue(0); // num_x_partitions_minus1
    bits.ue(0); // num_y_partitions_minus1
    for _ in 0..3 {
        bits.ue(0); // mapping_idc: polynomial
        bits.ue(0); // poly_order_minus1: linear
        bits.put(0, 1); // linear_interp_flag
        for coefficient in [0i64, 1] {
            bits.se(coefficient); // integer part
            bits.put(0, DENOM); // fractional part
        }
    }
    bits.align();
    let mut payload = bits.finish();
    let crc = crc32_mpeg(&payload[1..]);
    payload.extend_from_slice(&crc.to_be_bytes());
    payload.push(0x80);
    let mut nal = vec![62 << 1, 0x01];
    nal.extend(escape(&payload));
    nal
}

fn crc32_mpeg(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

// ---------------------------------------------------------------------------
// Annex B
// ---------------------------------------------------------------------------

/// Split an Annex B stream into NAL units (without start codes).
fn nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut units = Vec::with_capacity(starts.len());
    for (n, &start) in starts.iter().enumerate() {
        let mut end = starts.get(n + 1).map_or(data.len(), |next| next - 3);
        while end > start && data[end - 1] == 0 {
            end -= 1;
        }
        if end > start {
            units.push(&data[start..end]);
        }
    }
    units
}

fn is_vcl(codec: NalCodec, nal: &[u8]) -> bool {
    match codec {
        NalCodec::H264 => matches!(nal[0] & 0x1f, 1..=5),
        NalCodec::Hevc => (nal[0] >> 1) & 0x3f < 32,
    }
}

/// The first slice of a picture: first_mb_in_slice == 0 (H.264, ue(v) "1")
/// or first_slice_segment_in_pic_flag (HEVC).
fn starts_picture(codec: NalCodec, nal: &[u8]) -> bool {
    let header = match codec {
        NalCodec::H264 => 1,
        NalCodec::Hevc => 2,
    };
    is_vcl(codec, nal) && nal.get(header).is_some_and(|b| b & 0x80 != 0)
}

fn annex_b(data: &[u8], codec: NalCodec, inserts: &[NalInsert]) -> Result<Vec<u8>> {
    if inserts.contains(&NalInsert::DoviRpu) && codec != NalCodec::Hevc {
        return fail("Dolby Vision RPU NAL units are authored for HEVC only");
    }
    let units = nal_units(data);
    if !units.iter().any(|u| starts_picture(codec, u)) {
        return fail("no picture starts in the Annex B stream");
    }
    let mut messages = Vec::new();
    for insert in inserts {
        match insert {
            NalInsert::Captions => messages.push(sei_message(4, &a53_t35())),
            NalInsert::Hdr10Plus => messages.push(sei_message(4, &hdr10plus_t35())),
            NalInsert::StaticHdr => {
                messages.push(sei_message(137, &mastering_display()));
                messages.push(sei_message(144, &content_light()));
            }
            NalInsert::DoviRpu => {}
        }
    }
    let sei = (!messages.is_empty()).then(|| sei_nal(codec, &messages));
    let rpu = inserts.contains(&NalInsert::DoviRpu).then(dovi_rpu_nal);
    let mut out = Vec::with_capacity(data.len() + data.len() / 8);
    let emit = |out: &mut Vec<u8>, nal: &[u8]| {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
    };
    let mut in_picture = false;
    let mut index = 0;
    while index < units.len() {
        let unit = units[index];
        let vcl = is_vcl(codec, unit);
        if !vcl && in_picture {
            // The access unit ended with the previous VCL unit.
            if let Some(rpu) = &rpu {
                emit(&mut out, rpu);
            }
            in_picture = false;
        }
        if starts_picture(codec, unit) {
            if in_picture && let Some(rpu) = &rpu {
                emit(&mut out, rpu);
            }
            if let Some(sei) = &sei {
                emit(&mut out, sei);
            }
            in_picture = true;
        }
        emit(&mut out, unit);
        index += 1;
    }
    if in_picture && let Some(rpu) = &rpu {
        emit(&mut out, rpu);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// AV1
// ---------------------------------------------------------------------------

fn leb128(mut value: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

fn read_leb128(data: &[u8], at: &mut usize) -> Result<usize> {
    let mut value = 0usize;
    for shift in 0..8 {
        let byte = *data.get(*at).ok_or_else(|| error("truncated leb128"))?;
        *at += 1;
        value |= usize::from(byte & 0x7f) << (shift * 7);
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    fail("leb128 too long")
}

fn av1_hdr10plus(data: &[u8]) -> Result<Vec<u8>> {
    let mut payload = leb128(4); // metadata_type: ITU-T T.35
    payload.extend(hdr10plus_t35());
    payload.push(0x80); // trailing bits
    let mut obu = vec![(5 << 3) | 0x02]; // OBU_METADATA, has_size_field
    obu.extend(leb128(payload.len()));
    obu.extend(payload);
    let mut out = Vec::with_capacity(data.len() + 64);
    let mut at = 0;
    let mut delimiters = 0;
    while at < data.len() {
        let start = at;
        let header = data[at];
        if header & 0x80 != 0 || header & 0x02 == 0 {
            return fail("AV1 OBU without a size field: write the stream with -f obu");
        }
        at += 1 + usize::from(header & 0x04 != 0);
        let size = read_leb128(data, &mut at)?;
        let end = at
            .checked_add(size)
            .filter(|end| *end <= data.len())
            .ok_or_else(|| error("truncated OBU"))?;
        out.extend_from_slice(&data[start..end]);
        if (header >> 3) & 0x0f == 2 {
            out.extend_from_slice(&obu);
            delimiters += 1;
        }
        at = end;
    }
    if delimiters == 0 {
        return fail("no temporal delimiters in the AV1 stream");
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// MPEG-2 video, TS and PS
// ---------------------------------------------------------------------------

fn mpeg2_captions(data: &[u8]) -> Result<Vec<u8>> {
    let mut user_data = vec![0, 0, 1, 0xB2, b'G', b'A', b'9', b'4'];
    user_data.extend(a53_cc_data());
    let mut out = Vec::with_capacity(data.len() + data.len() / 16);
    let mut pending = false;
    let mut inserted = 0;
    let mut at = 0;
    while at < data.len() {
        let start_code =
            at + 4 <= data.len() && data[at] == 0 && data[at + 1] == 0 && data[at + 2] == 1;
        if start_code {
            let code = data[at + 3];
            if code == 0x00 {
                pending = true;
            } else if pending && (0x01..=0xAF).contains(&code) {
                out.extend_from_slice(&user_data);
                inserted += 1;
                pending = false;
            }
        }
        out.push(data[at]);
        at += 1;
    }
    if inserted == 0 {
        return fail("no MPEG-2 pictures found");
    }
    Ok(out)
}

fn ts_scramble(data: &[u8], packet_size: usize) -> Result<Vec<u8>> {
    let offset = packet_size - 188;
    if !data.len().is_multiple_of(packet_size) {
        return fail("not a whole number of TS packets");
    }
    let pid_of = |packet: &[u8]| (u16::from(packet[1] & 0x1f) << 8) | u16::from(packet[2]);
    // PMT PIDs come from the PAT, so PSI stays clear whatever PIDs the muxer chose.
    let mut pmts = Vec::new();
    for packet in data.chunks_exact(packet_size) {
        let packet = &packet[offset..];
        if packet[0] != 0x47 {
            return fail("lost TS sync");
        }
        if pid_of(packet) != 0 || packet[1] & 0x40 == 0 {
            continue;
        }
        let mut at = 4;
        if packet[3] & 0x20 != 0 {
            at += 1 + usize::from(packet[4]);
        }
        at += 1 + usize::from(*packet.get(at).unwrap_or(&0));
        let Some(section) = packet.get(at..) else {
            continue;
        };
        if section.len() < 8 || section[0] != 0 {
            continue;
        }
        let length = usize::from(u16::from_be_bytes([section[1], section[2]]) & 0x0fff);
        let end = (3 + length).saturating_sub(4).min(section.len());
        for entry in section.get(8..end).unwrap_or_default().chunks_exact(4) {
            let program = u16::from_be_bytes([entry[0], entry[1]]);
            let pid = (u16::from(entry[2] & 0x1f) << 8) | u16::from(entry[3]);
            if program != 0 && !pmts.contains(&pid) {
                pmts.push(pid);
            }
        }
    }
    if pmts.is_empty() {
        return fail("no PAT names a PMT");
    }
    let mut out = data.to_vec();
    let mut marked = 0;
    for packet in out.chunks_exact_mut(packet_size) {
        let packet = &mut packet[offset..];
        let pid = pid_of(packet);
        let has_payload = packet[3] & 0x10 != 0;
        if has_payload && pid >= 0x20 && pid != 0x1fff && !pmts.contains(&pid) {
            packet[3] = (packet[3] & 0x3f) | 0x80;
            let start = if packet[3] & 0x20 != 0 {
                5 + usize::from(packet[4])
            } else {
                4
            };
            if let Some(payload) = packet.get_mut(start..) {
                mask(payload, marked);
            }
            marked += 1;
        }
    }
    if marked == 0 {
        return fail("no elementary-stream packets to mark");
    }
    Ok(out)
}

/// The descriptor body is the first five bytes of the configuration record
/// (ETSI GS CCM 001, 4.3: version, profile, level, layer flags and, with a
/// base layer present, the compatibility id).
fn ts_dovi_descriptor(data: &[u8], record: super::post::DoviRecord) -> Result<Vec<u8>> {
    if !data.len().is_multiple_of(188) {
        return fail("not a whole number of 188-byte TS packets");
    }
    if !record.bl {
        return fail("a DOVI descriptor without a base layer needs a dependency PID");
    }
    let pid_of = |packet: &[u8]| (u16::from(packet[1] & 0x1f) << 8) | u16::from(packet[2]);
    let payload_start = |packet: &[u8]| {
        if packet[3] & 0x20 != 0 {
            5 + usize::from(packet[4])
        } else {
            4
        }
    };
    let mut pmt_pids = Vec::new();
    for packet in data.chunks_exact(188) {
        if packet[0] != 0x47 {
            return fail("lost TS sync");
        }
        if pid_of(packet) != 0 || packet[1] & 0x40 == 0 {
            continue;
        }
        let at = payload_start(packet);
        let section = &packet[at + 1 + usize::from(packet[at])..];
        let length = usize::from(u16::from_be_bytes([section[1], section[2]]) & 0x0fff);
        for entry in section[8..(3 + length - 4).min(section.len())].chunks_exact(4) {
            if u16::from_be_bytes([entry[0], entry[1]]) != 0 {
                pmt_pids.push((u16::from(entry[2] & 0x1f) << 8) | u16::from(entry[3]));
            }
        }
    }
    let descriptor: Vec<u8> = [&[0xB0, 5][..], &record.bytes()[..5]].concat();
    let mut out = data.to_vec();
    let mut edited = 0;
    for packet in out.chunks_exact_mut(188) {
        if !pmt_pids.contains(&pid_of(packet)) || packet[1] & 0x40 == 0 {
            continue;
        }
        let at = payload_start(packet);
        let start = at + 1 + usize::from(packet[at]);
        let section = packet[start..].to_vec();
        let length = usize::from(u16::from_be_bytes([section[1], section[2]]) & 0x0fff);
        if section[0] != 0x02 || 3 + length > section.len() {
            return fail("PMT section is not whole in one packet");
        }
        let body = &section[..3 + length - 4];
        let program_info = usize::from(u16::from_be_bytes([body[10], body[11]]) & 0x0fff);
        let mut rebuilt = body[..12 + program_info].to_vec();
        let mut at_entry = 12 + program_info;
        let mut added = false;
        while at_entry + 5 <= body.len() {
            let stream_type = body[at_entry];
            let info =
                usize::from(u16::from_be_bytes([body[at_entry + 3], body[at_entry + 4]]) & 0x0fff);
            let end = at_entry + 5 + info;
            let mut entry = body[at_entry..end].to_vec();
            if !added && matches!(stream_type, 0x1B | 0x24) {
                entry.extend_from_slice(&descriptor);
                let info = info + descriptor.len();
                entry[3] = (entry[3] & 0xf0) | (info >> 8) as u8;
                entry[4] = info as u8;
                added = true;
            }
            rebuilt.extend_from_slice(&entry);
            at_entry = end;
        }
        if !added {
            return fail("the PMT names no H.264 or HEVC stream");
        }
        let length = rebuilt.len() + 4 - 3;
        rebuilt[1] = (rebuilt[1] & 0xf0) | (length >> 8) as u8;
        rebuilt[2] = length as u8;
        let crc = crc32_mpeg(&rebuilt);
        rebuilt.extend_from_slice(&crc.to_be_bytes());
        if start + rebuilt.len() > 188 {
            return fail("the PMT outgrows its packet");
        }
        packet[start..start + rebuilt.len()].copy_from_slice(&rebuilt);
        packet[start + rebuilt.len()..].fill(0xFF);
        edited += 1;
    }
    if edited == 0 {
        return fail("no PMT packet to edit");
    }
    Ok(out)
}

/// XOR a deterministic xorshift keystream, seeded per packet, over a payload.
fn mask(payload: &mut [u8], packet: usize) {
    let mut state =
        0x9E37_79B9_7F4A_7C15u64 ^ (packet as u64).wrapping_mul(0xA24B_AED4_963E_E407) | 1;
    for byte in payload {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte ^= state as u8;
    }
}

fn ps_scramble(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = data.to_vec();
    let mut marked = 0;
    let mut at = 0;
    while at + 7 <= out.len() {
        if at + 9 <= out.len()
            && out[at] == 0
            && out[at + 1] == 0
            && out[at + 2] == 1
            && (0xE0..=0xEF).contains(&out[at + 3])
            && out[at + 6] & 0xC0 == 0x80
        {
            out[at + 6] = (out[at + 6] & 0xCF) | 0x10;
            let end = (at + 6 + usize::from(u16::from_be_bytes([out[at + 4], out[at + 5]])))
                .min(out.len());
            let start = (at + 9 + usize::from(out[at + 8])).min(end);
            mask(&mut out[start..end], marked);
            marked += 1;
            at = end.max(at + 6);
        } else {
            at += 1;
        }
    }
    if marked == 0 {
        return fail("no MPEG-2 PES video packets to mark");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hdr10plus_payload_carries_the_signature_the_parser_scans_for() {
        assert!(hdr10plus_t35().starts_with(&[0xB5, 0x00, 0x3C, 0x00, 0x01, 0x04, 0x01]));
    }

    #[test]
    fn exp_golomb_codes_match_the_spec_tables() {
        let mut bits = Bits::default();
        bits.ue(0); // 1
        bits.ue(2); // 011
        bits.se(1); // 010
        bits.se(-1); // 011
        bits.align();
        assert_eq!(bits.finish(), [0b1011_0100, 0b1100_0000]);
    }

    #[test]
    fn rpu_is_a_type_62_nal_whose_crc_checks() {
        let nal = dovi_rpu_nal();
        assert_eq!(nal[..3], [124, 1, 0x19]);
        // Unescape, drop the 0x80 terminator, and run CRC-32/MPEG-2 over
        // everything after the prefix including the stored CRC: residue 0.
        let mut rbsp = Vec::new();
        let mut zeros = 0;
        for &byte in &nal[2..] {
            if zeros >= 2 && byte == 3 {
                zeros = 0;
                continue;
            }
            zeros = if byte == 0 { zeros + 1 } else { 0 };
            rbsp.push(byte);
        }
        assert_eq!(rbsp.pop(), Some(0x80));
        assert_eq!(crc32_mpeg(&rbsp[1..]), 0);
    }

    #[test]
    fn escaping_breaks_every_start_code_emulation() {
        let escaped = escape(&[0, 0, 1, 0, 0, 0, 0, 0, 3]);
        assert_eq!(escaped, [0, 0, 3, 1, 0, 0, 3, 0, 0, 3, 0, 3]);
        assert!(escaped.windows(3).all(|w| w != [0, 0, 1]));
    }

    #[test]
    fn annex_b_injection_adds_one_sei_and_rpu_per_picture() {
        // AUD, SPS-ish, IDR slice (first_slice flag), slice 2, then next picture.
        let mut stream = Vec::new();
        for nal in [
            &[35 << 1, 1, 0x50][..],
            &[33 << 1, 1, 9],
            &[19 << 1, 1, 0x80, 1],
            &[1 << 1, 1, 0x00, 2],
            &[35 << 1, 1, 0x50],
            &[1 << 1, 1, 0x80, 3],
        ] {
            stream.extend_from_slice(&[0, 0, 0, 1]);
            stream.extend_from_slice(nal);
        }
        let out = annex_b(
            &stream,
            NalCodec::Hevc,
            &[NalInsert::StaticHdr, NalInsert::DoviRpu],
        )
        .unwrap();
        let units = nal_units(&out);
        let types: Vec<u8> = units.iter().map(|u| (u[0] >> 1) & 0x3f).collect();
        assert_eq!(types, [35, 33, 39, 19, 1, 62, 35, 39, 1, 62]);
    }

    #[test]
    fn av1_injection_follows_each_temporal_delimiter() {
        let stream = [0x12, 0x00, 0x0A, 0x01, 0xAA, 0x12, 0x00];
        let out = av1_hdr10plus(&stream).unwrap();
        assert_eq!(out.iter().filter(|b| **b == 0x2A).count(), 2);
        assert!(av1_hdr10plus(&[0x10, 0x00]).is_err());
    }

    #[test]
    fn caption_user_data_is_well_formed_a53() {
        let cc = a53_cc_data();
        assert_eq!(cc[0], 3);
        assert_eq!(usize::from(cc[1] & 31), 3);
        assert_eq!(cc[3 + 3 * 3], 0xFF);
    }
}
