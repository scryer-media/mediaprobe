use super::*;
use crate::Codec;
use crate::codec::aac::find_adts_header;

#[test]
fn budget_exhausted_during_stream_probing_keeps_the_program_map_tracks() {
    // The program map names the tracks; stream probing only details them. The
    // window where the budget dies in between moves with any change in I/O
    // accounting, so look for it instead of pinning a limit.
    let fixture = include_bytes!("../../../tests/media/h264_aac.ts").as_slice();
    let analysis = (8_u64..128)
        .filter_map(|limit| {
            let mut source = std::io::Cursor::new(fixture);
            let analysis = crate::probe_bounded(
                &mut source,
                "ts",
                &crate::ProbeOptions::new()
                    .with_depth(crate::ProbeDepth::Quick)
                    .with_io_limit(limit),
            )
            .unwrap_or_else(|error| panic!("io limit {limit} failed the analysis: {error}"));
            analysis
                .report
                .warnings
                .iter()
                .any(|warning| warning.code == "ts_probe_read_budget")
                .then_some(analysis)
        })
        .next()
        .expect("some io limit must exhaust the budget inside stream probing");
    assert_eq!(crate::test_facts::video_codec(&analysis), Some("h264"));
    assert!(
        !analysis.programs.is_empty(),
        "the program map survives the probing gap"
    );
    // Duration comes from a timestamp scan that runs after probing, so it can
    // stay unknown here. What matters is that the tracks are still described:
    // the analyzer then reports this file as inconclusive rather than invalid.
    assert!(analysis.report.budget_exhausted);
    assert_eq!(analysis.report.status, crate::ProbeStatus::Incomplete);
}

#[test]
fn delayed_program_map_uses_buffered_source_reads() {
    struct Metered {
        input: std::io::Cursor<Vec<u8>>,
        reads: usize,
    }
    impl Read for Metered {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            self.reads += 1;
            self.input.read(bytes)
        }
    }
    impl Seek for Metered {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.input.seek(position)
        }
    }
    impl crate::source::MediaSource for Metered {
        fn len(&self) -> u64 {
            self.input.get_ref().len() as u64
        }
    }
    let mut null = [0xff; TS_PACKET_SIZE];
    null[..4].copy_from_slice(&[0x47, 0x1f, 0xff, 0x10]);
    let mut bytes = null.repeat(10_000);
    bytes.extend_from_slice(include_bytes!("../../../tests/media/h264_aac.ts"));
    let mut source = Metered {
        input: std::io::Cursor::new(bytes),
        reads: 0,
    };
    let raw = parse_ts_source(&mut source, ProbeDepth::Identify).unwrap();
    assert!(
        raw.tracks
            .iter()
            .any(|track| { track.codec_name.as_ref().map(crate::Codec::as_str) == Some("h264") })
    );
    assert!(
        source.reads < 200,
        "{} underlying reads for delayed PSI",
        source.reads
    );
}

#[test]
fn parses_dovi_descriptor_payload() {
    let descriptors = [
        DOVI_VIDEO_STREAM_DESCRIPTOR,
        5,
        1,
        0,
        0b0000_1111,
        0b0001_1101,
        0b1011_0000,
    ];
    let dovi = extract_dovi_config(&descriptors).unwrap();
    assert_eq!(dovi, vec![1, 0, 0b0000_1111, 0b0001_1101, 0b1011_0000]);
}

#[test]
fn dovi_descriptor_without_base_layer_skips_dependency_pid() {
    // Profile 7 enhancement-layer stream: bl_present 0, then a 13-bit dependency_pid
    // (0x1011) with 3 reserved bits, then compatibility id 6.
    let descriptors = [
        DOVI_VIDEO_STREAM_DESCRIPTOR,
        7,
        1,
        0,
        0x0E,
        0x36,
        0x80,
        0x8F,
        0x60,
    ];
    let dovi = extract_dovi_config(&descriptors).unwrap();
    assert_eq!(dovi, vec![1, 0, 0x0E, 0x36, 0x60]);

    // Without a trailing compatibility byte the id defaults to 0.
    let mut short = descriptors;
    short[1] = 6;
    let dovi = extract_dovi_config(&short[..8]).unwrap();
    assert_eq!(dovi, vec![1, 0, 0x0E, 0x36, 0x00]);
}

#[test]
fn last_dovi_descriptor_wins_and_short_ones_are_ignored() {
    let profile_7 = [DOVI_VIDEO_STREAM_DESCRIPTOR, 5, 1, 0, 0x0E, 0x37, 0x60];
    let profile_8 = [DOVI_VIDEO_STREAM_DESCRIPTOR, 5, 1, 0, 0x10, 0x35, 0x10];
    let short = [DOVI_VIDEO_STREAM_DESCRIPTOR, 3, 1, 0, 0x10];
    assert_eq!(
        extract_dovi_config(&[profile_7, profile_8].concat()),
        Some(vec![1, 0, 0x10, 0x35, 0x10])
    );
    assert_eq!(
        extract_dovi_config(&[&profile_7[..], &short[..]].concat()),
        Some(vec![1, 0, 0x0E, 0x37, 0x60])
    );
    assert_eq!(extract_dovi_config(&short), None);
}

#[test]
fn transport_enrichment_reports_packet_payload_and_short_source_limits() {
    for (pid, packets, exhausted) in [
        (0x1fff_u16, STREAM_PROBE_PACKET_LIMIT + 10, true),
        (
            256,
            STREAM_PROBE_MAX_BYTES_PER_PID as usize / 184 + STREAM_PROBE_BATCH_PACKETS * 2,
            true,
        ),
        (256, 5, false),
    ] {
        let mut packet = [0_u8; TS_PACKET_SIZE];
        packet[..4].copy_from_slice(&[SYNC_BYTE, (pid >> 8) as u8, pid as u8, 0x10]);
        let mut source = std::io::Cursor::new(packet.repeat(packets));
        let entries = [EsEntry {
            program_id: 1,
            stream_type: 0x87,
            pid: 256,
            descriptors: Vec::new(),
            dovi_config: None,
        }];
        let mut tracks = [build_track(&entries[0])];
        let mut report = crate::ProbeReport::default();
        enrich_tracks_from_probe(
            &mut source,
            &entries,
            &mut tracks,
            TsPacketLayout {
                raw_packet_size: TS_PACKET_SIZE,
                sync_offset: 0,
            },
            &mut report,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(report.status, crate::ProbeStatus::Incomplete);
        assert_eq!(report.budget_exhausted, exhausted);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.stream_id.as_deref() == Some("256")
                    && warning.code
                        == if exhausted {
                            "ts_stream_probe_budget"
                        } else {
                            "ts_stream_probe_incomplete"
                        })
        );
        assert!(tracks[0].channels.is_none());
        assert!(source.position() <= (STREAM_PROBE_PACKET_LIMIT * TS_PACKET_SIZE) as u64);
        if exhausted {
            assert!(source.position() < source.get_ref().len() as u64);
            if pid == 256 {
                let read_bound = (STREAM_PROBE_MAX_BYTES_PER_PID as usize).div_ceil(184)
                    + STREAM_PROBE_BATCH_PACKETS;
                assert!(
                    source.position() <= (read_bound * TS_PACKET_SIZE) as u64,
                    "read-ahead must stay within one packet batch of the payload budget"
                );
            }
        }
    }
}

#[test]
fn short_aac_samples_are_estimated_only_when_enrichment_finishes() {
    for (codec, frames, exhausted) in [
        ("aac", 1, false),
        ("aac", 16, false),
        ("aac", 16, true),
        ("unknown", 1, false),
        ("unknown", 16, false),
        ("unknown", 16, true),
    ] {
        let mut track = RawTrack {
            codec_name: Some(codec.into()),
            ..Default::default()
        };
        let mut state = TsStreamProbeState::new(0, TrackKind::Audio, Some(codec.into()));
        state.push_payload(
            &[0xFF, 0xF1, 0x50, 0x80, 0x00, 0xFF, 0xFC].repeat(frames),
            &mut track,
        );
        assert!(
            !state.complete,
            "continue collecting toward the regular 128-frame target"
        );
        assert!(track.metadata.estimated_bitrate_bps.is_none());
        assert_eq!(
            state.codec_name.as_ref().map(crate::Codec::as_str),
            Some("aac")
        );
        state.finish(&mut track, exhausted);
        assert_eq!(
            track.metadata.estimated_bitrate_bps,
            (frames > 1).then_some(2411)
        );
        assert!(track.bit_rate_bps.is_none());
        assert_eq!(state.metadata_report.budget_exhausted, exhausted);
        if exhausted {
            assert_eq!(state.metadata_report.status, crate::ProbeStatus::Incomplete);
        }
    }
}

#[test]
fn completed_transport_enrichment_does_not_report_an_unused_budget() {
    let mut track = RawTrack {
        codec_name: Some(Codec::Aac),
        ..Default::default()
    };
    let mut state = TsStreamProbeState::new(0, TrackKind::Audio, Some("aac".into()));
    state.push_payload(
        &[0xFF, 0xF1, 0x50, 0x80, 0x00, 0xFF, 0xFC].repeat(128),
        &mut track,
    );
    assert!(state.complete);
    state.finish(&mut track, true);
    assert!(!state.metadata_report.budget_exhausted);
    assert!(state.metadata_report.warnings.is_empty());
    assert_eq!(track.channels, Some(2));
}

#[test]
fn detects_bluray_dvhs_packet_layout() {
    let mut data = vec![0u8; TS_DVHS_PACKET_SIZE * 3];
    for packet_idx in 0..3 {
        data[packet_idx * TS_DVHS_PACKET_SIZE + 4] = SYNC_BYTE;
    }

    let file_size = data.len() as u64;
    let layout = detect_ts_packet_layout(&mut std::io::Cursor::new(data), file_size).unwrap();
    assert_eq!(layout.raw_packet_size, TS_DVHS_PACKET_SIZE);
    assert_eq!(layout.sync_offset, 4);
}

#[test]
fn parses_adts_header_channels_and_bitrate() {
    let mut data = Vec::new();
    for _ in 0..128 {
        data.extend_from_slice(&[0xFF, 0xF1, 0x50, 0x80, 0x00, 0xFF, 0xFC]);
    }
    let header = find_adts_header(&data).unwrap();
    assert_eq!(header.channels, 2);
    assert_eq!(header.sample_rate, 44100);
    assert_eq!(header.channel_layout, "stereo");
    assert!(header.bit_rate_bps.is_some());
    let mut track = RawTrack {
        codec_name: Some(Codec::Aac),
        ..Default::default()
    };
    probe_aac_track(&data, &mut track);
    assert!(
        track.bit_rate_bps.is_none(),
        "bounded sample accounting is not a measured stream bitrate"
    );
    assert!(track.metadata.estimated_bitrate_bps.is_some());
    assert_eq!(track.metadata.sample_rate, Some(44100));
    let state = TsStreamProbeState::new(0, TrackKind::Audio, Some("aac".into()));
    assert!(state.probe_complete(&track));
    assert!(
        !state.track_probe_needed(&track),
        "bounded estimates must not force payload scanning for a measured bitrate"
    );
    for header in data.as_chunks_mut::<7>().0 {
        header[2] |= 1;
        header[3] |= 0xc0;
    }
    let header = find_adts_header(&data).unwrap();
    assert_eq!(
        header.channels, 8,
        "AAC configuration 7 describes eight channels"
    );
    assert_eq!(header.channel_layout, "7.1(wide)");
}

#[test]
fn pts_frame_rate_estimate_prefers_dense_cadence_over_repeated_field_gaps() {
    let pts = [0, 3003, 12012, 15015, 24024, 27027, 36036, 39039];
    let fps = estimate_frame_rate_from_pts(&pts).unwrap();
    assert!((fps - (30000.0 / 1001.0)).abs() < 0.001);
}

#[test]
fn h264_ts_keeps_plausible_sps_frame_rate_over_sparse_pts_cadence() {
    assert!(!should_use_pts_frame_rate(
        Some(&crate::Codec::H264),
        Some(30000.0 / 1001.0),
        30000.0 / 3003.0
    ));
    assert!(should_use_pts_frame_rate(
        Some(&crate::Codec::Mpeg2Video),
        Some(24.0),
        12.0
    ));
}

#[test]
fn subtitle_language_prefers_subtitling_descriptor() {
    let descriptors = [
        ISO_639_LANGUAGE_DESCRIPTOR,
        4,
        b'e',
        b'n',
        b'g',
        0,
        SUBTITLING_DESCRIPTOR,
        8,
        b'j',
        b'p',
        b'n',
        0x10,
        0,
        1,
        0,
        2,
    ];
    assert_eq!(
        extract_language(TrackKind::Subtitle, &descriptors),
        Some("jpn".to_string())
    );
}

#[test]
fn audio_language_prefers_supplementary_audio_override() {
    let descriptors = [
        ISO_639_LANGUAGE_DESCRIPTOR,
        4,
        b'e',
        b'n',
        b'g',
        0,
        DVB_EXTENSION_DESCRIPTOR,
        5,
        SUPPLEMENTARY_AUDIO_DESCRIPTOR,
        0x01,
        b'j',
        b'p',
        b'n',
    ];
    assert_eq!(
        extract_language(TrackKind::Audio, &descriptors),
        Some("jpn".to_string())
    );
}

#[test]
fn truncated_supplementary_audio_descriptor_falls_back_without_panicking() {
    let descriptors = [
        ISO_639_LANGUAGE_DESCRIPTOR,
        4,
        b'e',
        b'n',
        b'g',
        0,
        DVB_EXTENSION_DESCRIPTOR,
        5,
        SUPPLEMENTARY_AUDIO_DESCRIPTOR,
        0x01,
        b'j',
    ];
    assert_eq!(
        extract_language(TrackKind::Audio, &descriptors),
        Some("eng".to_string())
    );
}

fn psi_packets(pid: u16, section: &[u8]) -> Vec<u8> {
    let mut payload = vec![0_u8];
    payload.extend_from_slice(section);
    let mut bytes = Vec::new();
    for (index, chunk) in payload.chunks(TS_PACKET_SIZE - 4).enumerate() {
        let start = if index == 0 { 0x40 } else { 0 };
        let mut packet = [0xff_u8; TS_PACKET_SIZE];
        packet[..4].copy_from_slice(&[SYNC_BYTE, start | (pid >> 8) as u8, pid as u8, 0x10]);
        packet[4..4 + chunk.len()].copy_from_slice(chunk);
        bytes.extend_from_slice(&packet);
    }
    bytes
}

fn section(table_id: u8, id: u16, body: &[u8]) -> Vec<u8> {
    // Header after the length field (5 bytes), the body, then a CRC slot.
    let length = 5 + body.len() + 4;
    let mut section = vec![
        table_id,
        0xb0 | (length >> 8) as u8,
        length as u8,
        (id >> 8) as u8,
        id as u8,
        0xc1,
        0,
        0,
    ];
    section.extend_from_slice(body);
    section.extend_from_slice(&[0; 4]);
    section
}

/// A transport stream whose programs each declare as many streams as one PMT
/// section holds.
fn crowded_program_map(programs: u16) -> (Vec<u8>, usize) {
    const PER_PROGRAM: u16 = 816;
    let pat: Vec<u8> = (0..programs)
        .flat_map(|program| {
            let pmt = 0x1000 + program;
            [0, (program + 1) as u8, 0xe0 | (pmt >> 8) as u8, pmt as u8]
        })
        .collect();
    let mut bytes = psi_packets(PAT_PID, &section(0, 1, &pat));
    let mut next_pid = 0x20_u16;
    for program in 0..programs {
        let mut body = vec![0xe1, 0x00, 0xf0, 0x00];
        for _ in 0..PER_PROGRAM {
            body.extend_from_slice(&[0x06, 0xe0 | (next_pid >> 8) as u8, next_pid as u8, 0xf0, 0]);
            next_pid += 1;
        }
        let pmt = section(0x02, program + 1, &body);
        assert_eq!(pmt.len(), 4096);
        bytes.extend(psi_packets(0x1000 + program, &pmt));
    }
    (bytes, usize::from(programs * PER_PROGRAM))
}

#[test]
fn a_crowded_program_map_reports_a_bounded_inventory() {
    let (bytes, declared) = crowded_program_map(6);
    assert!(declared > ELEMENTARY_STREAM_LIMIT);
    let raw = parse_ts_source(&mut std::io::Cursor::new(bytes), ProbeDepth::Identify).unwrap();
    assert_eq!(raw.tracks.len(), ELEMENTARY_STREAM_LIMIT);
    let report = &raw.details.report;
    assert_eq!(report.status, crate::ProbeStatus::Incomplete);
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.code == warning::TS_ELEMENTARY_STREAM_LIMIT)
    );
    let reported: usize = raw
        .details
        .programs
        .iter()
        .map(|program| program.stream_ids.len())
        .sum();
    assert_eq!(reported, ELEMENTARY_STREAM_LIMIT);

    // A map within the limit keeps every stream and adds no warning.
    let (bytes, declared) = crowded_program_map(5);
    assert!(declared <= ELEMENTARY_STREAM_LIMIT);
    let raw = parse_ts_source(&mut std::io::Cursor::new(bytes), ProbeDepth::Identify).unwrap();
    assert_eq!(raw.tracks.len(), declared);
    assert!(
        raw.details
            .report
            .warnings
            .iter()
            .all(|warning| warning.code != warning::TS_ELEMENTARY_STREAM_LIMIT)
    );
}

#[test]
fn short_streams_scan_for_timestamps_with_buffers_sized_to_the_file() {
    // Reads and seeks the source sees, so a smaller buffer can be shown to
    // leave them unchanged.
    struct Metered {
        input: std::io::Cursor<Vec<u8>>,
        calls: Vec<(char, usize, usize)>,
    }
    impl Read for Metered {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            let read = self.input.read(bytes)?;
            self.calls.push(('r', bytes.len().min(1 << 20), read));
            Ok(read)
        }
    }
    impl Seek for Metered {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            let at = self.input.seek(position)?;
            self.calls.push(('s', at as usize, 0));
            Ok(at)
        }
    }
    impl crate::source::MediaSource for Metered {
        fn len(&self) -> u64 {
            self.input.get_ref().len() as u64
        }
    }
    let (bytes, _) = crowded_program_map(1);
    let file_size = bytes.len() as u64;
    let lookup = {
        let mut lookup = [false; TS_PID_COUNT];
        lookup[0x20] = true;
        lookup
    };
    let layout = TsPacketLayout {
        raw_packet_size: TS_PACKET_SIZE,
        sync_offset: 0,
    };
    let run = |file_size: u64| {
        let mut source = Metered {
            input: std::io::Cursor::new(bytes.clone()),
            calls: Vec::new(),
        };
        let mut file = std::io::BufReader::with_capacity(
            READ_BUFFER_BYTES.min(file_size.saturating_add(1)) as usize,
            &mut source,
        );
        let layout_found = detect_ts_packet_layout(&mut file, file_size).unwrap();
        let pts = find_pts_near(
            &mut file,
            file_size,
            0,
            true,
            &lookup,
            FALLBACK_DURATION_PROBE_PACKETS,
            layout,
        );
        drop(file);
        (layout_found.raw_packet_size, pts, source.calls)
    };
    let (size, pts, sized) = run(file_size);
    assert_eq!(size, TS_PACKET_SIZE);
    assert_eq!(pts, None);
    // An unknown, effectively unbounded size reproduces full-size buffers.
    let (_, _, unbounded) = run(u64::MAX - 1);
    let returned = |calls: &[(char, usize, usize)]| -> Vec<(char, usize)> {
        calls
            .iter()
            .map(|(kind, at, read)| (*kind, if *kind == 's' { *at } else { *read }))
            .collect()
    };
    assert_eq!(returned(&sized), returned(&unbounded));
    // Every read asked for more than the file had left.
    assert!(
        sized
            .iter()
            .filter(|(kind, _, _)| *kind == 'r')
            .all(|(_, asked, read)| asked > read)
    );
}
