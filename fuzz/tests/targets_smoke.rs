//! Every target body runs on stable Rust over a fixed set of inputs: each
//! selector byte of the wrapping targets with a few payload shapes, and the
//! built containers through the container targets. This keeps the targets
//! honest in ordinary CI, where libFuzzer does not run.

use mediaprobe_fuzz::{build, inputs, params, targets};

const PAYLOADS: [&[u8]; 4] = [
    &[],
    &[0x00, 0x00, 0x03, 0x01, 0xFF],
    &[0xFF; 64],
    &[
        0x01, 0x10, 0x10, 0x00, 0x00, 0x13, 0x88, 0x02, 0x00, 0x00, 0x03, 0xE8, 0x00, 0x00,
    ],
];

#[test]
fn wrapping_targets_accept_every_selector() {
    for pick in 0..=255u8 {
        for payload in PAYLOADS {
            let data = [&[pick][..], payload].concat();
            targets::codec_config(&data);
            targets::bitstream(&data);
            targets::hdr_sei(&data);
            targets::dolby_vision(&data);
            targets::dispatch(&data);
            targets::probe_depths(&data);
        }
    }
}

fn built() -> Vec<(Vec<u8>, &'static str)> {
    vec![
        (
            build::mp4_with_config(b"avcC", &params::H264_AVCC, 64, 36),
            "mp4",
        ),
        (
            build::mp4_with_config(b"hvcC", &params::hevc_hvcc(), 64, 36),
            "mp4",
        ),
        (
            build::mkv_with_codec(
                b"V_MPEG4/ISO/AVC",
                &params::H264_AVCC,
                &build::length_prefixed(&[&params::H264_SPS[..]]),
                64,
                36,
            ),
            "mkv",
        ),
        (
            build::ts_with_es(
                0x1B,
                &[],
                &build::annexb(&[&params::H264_SPS[..], &params::H264_PPS[..]]),
            ),
            "ts",
        ),
    ]
}

#[test]
fn built_containers_analyse_as_what_they_carry() {
    // The builders must produce files the parser accepts; otherwise the
    // wrapping targets would only ever exercise container rejection.
    for (bytes, ext) in built() {
        let analysis = targets::analyze_counted(std::io::Cursor::new(&bytes[..]), ext)
            .unwrap_or_else(|| panic!("{ext}: the parser refused a built file"));
        let video_codec = analysis
            .get("streams")
            .and_then(|s| s.as_array())
            .into_iter()
            .flatten()
            .any(|s| {
                s.get("kind").and_then(|k| k.as_str()) == Some("video")
                    && s.get("codec").is_some_and(|c| !c.is_null())
            });
        assert!(video_codec, "{ext}: no video codec in {analysis}");
    }
}

#[test]
fn simd_differential_and_budget_hold_on_built_inputs() {
    for (bytes, ext) in built() {
        let pick = inputs::EXTENSIONS.iter().position(|e| *e == ext).unwrap() as u8;
        let data = [&[pick][..], &bytes].concat();
        targets::simd_differential(&data);
        targets::simd_differential(&bytes);
        targets::budget(&data);
    }
}

#[test]
fn tag_and_model_targets_accept_their_shapes() {
    for shape in [
        [0u8, 0, 0, 0, 0, 0],
        [3, 2, 0, 4, 0, 0],
        [63, 16, 0, 16, 0, 0x81],
        [255, 255, 0, 255, 0, 1],
    ] {
        for payload in PAYLOADS {
            targets::mkv_tags(&[&shape[..], payload].concat());
        }
    }
    targets::mkv_tags(&inputs::mkv_tags_flood_seed());
    let details = mediaprobe::AnalysisDetails {
        duration_seconds: Some(12.5),
        container_format: Some("matroska".into()),
        ..Default::default()
    };
    targets::model_serde(&serde_json::to_vec(&details).unwrap());
    targets::model_serde(b"{}");
    targets::model_serde(b"not json");
}

/// The tag flood the parser bounds: tens of thousands of targets times
/// hundreds of 4 KiB simple tags keep at most 256 KiB of tags, with one
/// truncation warning, and the stream is still reported.
#[test]
fn tag_flood_seed_is_bounded() {
    let input = inputs::mkv_tags_flood_seed();
    let built = inputs::mkv_tags(&input);
    let details = targets::analyze_counted(std::io::Cursor::new(&built.bytes[..]), "mkv")
        .expect("the flood file probes");
    let (bytes, warnings) = targets::tag_totals(&details);
    assert!(
        bytes > 0 && bytes <= targets::TAG_BYTES_LIMIT,
        "kept {bytes} bytes"
    );
    assert_eq!(warnings, 1);
    assert_eq!(
        details.pointer("/streams/0/kind").and_then(|v| v.as_str()),
        Some("video")
    );
}
