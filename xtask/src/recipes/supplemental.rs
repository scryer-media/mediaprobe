//! Named fixtures for one feature each: AV1, HEVC HDR10 / HLG / HDR10+,
//! Dolby Vision configuration records, PQ sequence headers, chapters, MPEG
//! program streams, an 8K stream and a few plain container baselines.

use super::matrix::args;
use super::post::{DoviRecord, Post};
use super::{Expect, Fixture, Generator, Step};

const MASTER_DISPLAY: &str =
    "G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,50)";

/// Small, deterministic HDR10+ dynamic metadata for x265's `dhdr10-info`.
const HDR10PLUS_JSON: &str = r#"{"JSONInfo":{"HDR10plusProfile":"B","Version":"1.0"},"SceneInfo":[{"BezierCurveData":{"Anchors":[256,512,768,1024,1280,1536,1792,2048,2304],"KneePointX":0,"KneePointY":0},"LuminanceParameters":{"AverageRGB":1000,"LuminanceDistributions":{"DistributionIndex":[1,5,10,25,50,75,90,95,99],"DistributionValues":[0,1,2,3,4,5,6,7,8]},"MaxScl":[1000,1000,1000]},"NumberOfWindows":1,"TargetedSystemDisplayMaximumLuminance":400,"SceneFrameNumbers":0,"SceneId":0,"SequenceFrameNumber":0}],"SceneInfoSummary":{"SceneFirstFrameIndex":[0],"SceneFrameNumbers":[24]},"ToolInfo":{"Tool":"mediaprobe-fixture","Version":"1"}}"#;

fn black(size: &str, rate: u32, seconds: &str) -> Vec<String> {
    vec![
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
        format!("color=c=black:s={size}:r={rate}:d={seconds}"),
    ]
}

fn test_pattern(size: &str, rate: u32, seconds: &str) -> Vec<String> {
    vec![
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
        format!("testsrc2=size={size}:rate={rate}:duration={seconds}"),
    ]
}

fn silence(layout: &str, rate: u32, seconds: &str) -> Vec<String> {
    vec![
        "-f".into(),
        "lavfi".into(),
        "-t".into(),
        seconds.into(),
        "-i".into(),
        format!("anullsrc=r={rate}:cl={layout}"),
    ]
}

fn x264() -> Vec<String> {
    args(&[
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
    ])
}

fn svtav1() -> Vec<String> {
    args(&["-c:v", "libsvtav1", "-preset", "12", "-pix_fmt", "yuv420p"])
}

/// libx265 Main 10 with colour signalling and, optionally, HDR10 SEI and
/// HDR10+ dynamic metadata.
fn x265(transfer: &str, hdr10: bool, dynamic: bool) -> Vec<String> {
    let mut params = format!(
        "log-level=error:colorprim=bt2020:transfer={transfer}:colormatrix=bt2020nc:repeat-headers=1"
    );
    if hdr10 {
        params.push_str(&format!(
            ":hdr10=1:master-display={MASTER_DISPLAY}:max-cll=1000,400"
        ));
    }
    if dynamic {
        params.push_str(":dhdr10-info={work}/hdr10plus.json");
    }
    let mut a = args(&[
        "-c:v",
        "libx265",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p10le",
        "-tag:v",
        "hvc1",
    ]);
    a.extend(args(&[
        "-color_primaries",
        "bt2020",
        "-colorspace",
        "bt2020nc",
        "-color_trc",
        transfer,
    ]));
    a.extend(["-x265-params".into(), params]);
    a
}

fn audio(codec: &str, bitrate: Option<&str>) -> Vec<String> {
    let mut a = vec!["-c:a".to_owned(), codec.to_owned()];
    if let Some(bitrate) = bitrate {
        a.extend(["-b:a".to_owned(), bitrate.to_owned()]);
    }
    a
}

fn out(muxer: &str) -> Vec<String> {
    vec![
        "-f".into(),
        muxer.into(),
        "-shortest".into(),
        "{out}".into(),
    ]
}

fn ffmpeg(parts: &[Vec<String>]) -> Step {
    Step::Ffmpeg(parts.concat())
}

fn fixture(
    name: &str,
    generator: Generator,
    steps: Vec<Step>,
    expect: Expect,
    encoders: &[&'static str],
) -> Fixture {
    Fixture {
        name: name.into(),
        generator,
        steps,
        expect,
        encoders: encoders.to_vec(),
    }
}

fn hdr(mut e: Expect, format: &'static str) -> Expect {
    e.video_bit_depth = Some(10);
    e.hdr_format = Some(format);
    e
}

pub(crate) fn fixtures() -> Vec<Fixture> {
    use Generator::{DolbyVision as Dv, Supplemental as Sup};
    // Plain baselines: one per major container with the commonest pair.
    let mut v = vec![
        fixture(
            "h264_aac.mkv",
            Sup,
            vec![ffmpeg(&[
                test_pattern("128x72", 24, "1"),
                silence("stereo", 44100, "1"),
                x264(),
                audio("aac", Some("96k")),
                out("matroska"),
            ])],
            Expect::video("matroska", "h264", 128, 72, 24).audio("aac", 2, None),
            &["libx264", "aac"],
        ),
        fixture(
            "h264_aac.mp4",
            Sup,
            vec![ffmpeg(&[
                test_pattern("128x72", 24, "1"),
                silence("stereo", 44100, "1"),
                x264(),
                audio("aac", Some("96k")),
                out("mp4"),
            ])],
            Expect::video("mp4", "h264", 128, 72, 24).audio("aac", 2, None),
            &["libx264", "aac"],
        ),
        fixture(
            "h264_aac.ts",
            Sup,
            vec![ffmpeg(&[
                test_pattern("128x72", 24, "2"),
                silence("stereo", 44100, "2"),
                x264(),
                audio("aac", Some("96k")),
                out("mpegts"),
            ])],
            Expect::video("mpegts", "h264", 128, 72, 24).audio("aac", 2, None),
            &["libx264", "aac"],
        ),
        fixture(
            "mpeg4_mp3.avi",
            Sup,
            vec![ffmpeg(&[
                test_pattern("128x72", 25, "1"),
                silence("stereo", 44100, "1"),
                args(&["-c:v", "mpeg4", "-q:v", "8", "-pix_fmt", "yuv420p"]),
                audio("libmp3lame", Some("96k")),
                out("avi"),
            ])],
            Expect::video("avi", "mpeg4", 128, 72, 25).audio("mp3", 2, None),
            &["mpeg4", "libmp3lame"],
        ),
    ];
    let mut dual = Expect::video("matroska", "h264", 128, 72, 24)
        .audio("flac", 6, Some("jpn"))
        .audio("aac", 2, Some("eng"));
    dual.subtitle_stream_count = 1;
    v.push(fixture(
        "dual_audio_subs.mkv",
        Sup,
        vec![
            Step::WriteFile {
                name: "subs.srt".into(),
                contents: "1\n00:00:00,000 --> 00:00:01,000\nfixture\n".into(),
            },
            ffmpeg(&[
                test_pattern("128x72", 24, "1"),
                silence("5.1", 44100, "1"),
                silence("stereo", 44100, "1"),
                args(&[
                    "-f",
                    "srt",
                    "-i",
                    "{work}/subs.srt",
                    "-map",
                    "0:v",
                    "-map",
                    "1:a",
                    "-map",
                    "2:a",
                    "-map",
                    "3:s",
                ]),
                x264(),
                args(&[
                    "-c:a:0",
                    "flac",
                    "-c:a:1",
                    "aac",
                    "-b:a:1",
                    "96k",
                    "-c:s",
                    "srt",
                    "-metadata:s:a:0",
                    "language=jpn",
                    "-metadata:s:a:1",
                    "language=eng",
                    "-metadata:s:s:0",
                    "language=eng",
                ]),
                out("matroska"),
            ]),
        ],
        dual,
        &["libx264", "flac", "aac", "srt"],
    ));
    let mut eight_k = Expect::video("matroska", "h264", 7680, 4320, 2).audio("aac", 2, None);
    eight_k.min_duration_seconds = 1;
    v.push(fixture(
        "h264_8k_aac.mkv",
        Sup,
        vec![ffmpeg(&[
            black("7680x4320", 2, "1"),
            silence("stereo", 48000, "1"),
            args(&[
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-profile:v",
                "baseline",
                "-level",
                "6.1",
                "-pix_fmt",
                "yuv420p",
            ]),
            audio("aac", Some("96k")),
            out("matroska"),
        ])],
        eight_k,
        &["libx264", "aac"],
    ));

    // AV1.
    v.push(fixture(
        "av1_aac.mp4",
        Sup,
        vec![ffmpeg(&[
            test_pattern("128x72", 24, "1"),
            silence("stereo", 44100, "1"),
            svtav1(),
            audio("aac", Some("96k")),
            out("mp4"),
        ])],
        Expect::video("mp4", "av1", 128, 72, 24).audio("aac", 2, None),
        &["libsvtav1", "aac"],
    ));
    v.push(fixture(
        "av1_flac.mkv",
        Sup,
        vec![ffmpeg(&[
            test_pattern("128x72", 24, "1"),
            silence("5.1", 44100, "1"),
            svtav1(),
            audio("flac", None),
            out("matroska"),
        ])],
        Expect::video("matroska", "av1", 128, 72, 24).audio("flac", 6, None),
        &["libsvtav1", "flac"],
    ));

    // PQ sequence headers with no audio, and their Matroska remuxes.
    let svt_pq = {
        let mut a = args(&[
            "-c:v",
            "libsvtav1",
            "-preset",
            "12",
            "-pix_fmt",
            "yuv420p10le",
            "-color_primaries",
            "bt2020",
            "-colorspace",
            "bt2020nc",
            "-color_trc",
            "smpte2084",
        ]);
        a.extend([
            "-svtav1-params".into(),
            "color-primaries=9:transfer-characteristics=16:matrix-coefficients=9".into(),
        ]);
        a
    };
    v.push(fixture(
        "av1_sequence_pq.mp4",
        Sup,
        vec![ffmpeg(&[
            black("64x64", 24, "1"),
            svt_pq,
            args(&["-f", "mp4", "{out}"]),
        ])],
        hdr(Expect::video("mp4", "av1", 64, 64, 24), "HDR10"),
        &["libsvtav1"],
    ));
    v.push(fixture(
        "av1_sequence_pq.mkv",
        Sup,
        vec![Step::Ffmpeg(args(&[
            "-i",
            "{media}/av1_sequence_pq.mp4",
            "-c",
            "copy",
            "-f",
            "matroska",
            "{out}",
        ]))],
        hdr(Expect::video("matroska", "av1", 64, 64, 24), "HDR10"),
        &[],
    ));
    v.push(fixture(
        "hevc_sequence_pq.mp4",
        Sup,
        vec![ffmpeg(&[
            black("64x64", 24, "1"),
            x265("smpte2084", true, false),
            args(&["-f", "mp4", "{out}"]),
        ])],
        hdr(Expect::video("mp4", "hevc", 64, 64, 24), "HDR10"),
        &["libx265"],
    ));
    v.push(fixture(
        "hevc_sequence_pq.mkv",
        Sup,
        vec![Step::Ffmpeg(args(&[
            "-i",
            "{media}/hevc_sequence_pq.mp4",
            "-c",
            "copy",
            "-f",
            "matroska",
            "{out}",
        ]))],
        hdr(Expect::video("matroska", "hevc", 64, 64, 24), "HDR10"),
        &[],
    ));

    // Chapters: one Latin and one CJK title, Nero chpl and QuickTime text track.
    let chapters = ";FFMETADATA1\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=500\ntitle=Opening\n\
                    [CHAPTER]\nTIMEBASE=1/1000\nSTART=500\nEND=1000\ntitle=幕間\n";
    for (name, flags) in [
        ("chapters_nero.mp4", "+faststart"),
        ("chapters_quicktime.mp4", "+faststart+disable_chpl"),
    ] {
        let mut e = hdr(Expect::video("mp4", "hevc", 64, 64, 24), "HDR10");
        e.chapter_titles = vec!["Opening", "幕間"];
        v.push(fixture(
            name,
            Sup,
            vec![
                Step::WriteFile {
                    name: "chapters.txt".into(),
                    contents: chapters.into(),
                },
                Step::Ffmpeg(args(&[
                    "-i",
                    "{media}/hevc_sequence_pq.mp4",
                    "-i",
                    "{work}/chapters.txt",
                    "-map",
                    "0",
                    "-map_metadata",
                    "1",
                    "-map_chapters",
                    "1",
                    "-c",
                    "copy",
                    "-movflags",
                    flags,
                    "-f",
                    "mp4",
                    "{out}",
                ])),
            ],
            e,
            &[],
        ));
    }

    // HEVC HDR10, HLG and HDR10+.
    v.push(fixture(
        "hevc_hdr10.mkv",
        Sup,
        vec![ffmpeg(&[
            test_pattern("128x72", 24, "1"),
            silence("5.1", 44100, "1"),
            x265("smpte2084", true, false),
            audio("flac", None),
            out("matroska"),
        ])],
        hdr(
            Expect::video("matroska", "hevc", 128, 72, 24).audio("flac", 6, None),
            "HDR10",
        ),
        &["libx265", "flac"],
    ));
    v.push(fixture(
        "hevc_hlg.mkv",
        Sup,
        vec![ffmpeg(&[
            test_pattern("128x72", 24, "1"),
            silence("stereo", 44100, "1"),
            x265("arib-std-b67", false, false),
            audio("aac", Some("96k")),
            out("matroska"),
        ])],
        hdr(
            Expect::video("matroska", "hevc", 128, 72, 24).audio("aac", 2, None),
            "HLG",
        ),
        &["libx265", "aac"],
    ));
    for (name, muxer, container) in [
        ("hevc_hdr10plus.mkv", "matroska", "matroska"),
        ("hevc_hdr10plus.mp4", "mp4", "mp4"),
    ] {
        v.push(fixture(
            name,
            Sup,
            vec![
                Step::WriteFile {
                    name: "hdr10plus.json".into(),
                    contents: HDR10PLUS_JSON.into(),
                },
                ffmpeg(&[
                    test_pattern("128x72", 24, "1"),
                    silence("stereo", 48000, "1"),
                    x265("smpte2084", true, true),
                    audio("aac", Some("96k")),
                    out(muxer),
                ]),
            ],
            hdr(
                Expect::video(container, "hevc", 128, 72, 24).audio("aac", 2, None),
                "HDR10+",
            ),
            &["libx265", "aac"],
        ));
    }

    // Dolby Vision configuration records: profile 5 (no compatible base
    // layer), 7 (dual layer, here its record only) and 8.1 (HDR10 compatible).
    for (profile, compat) in [(5u8, 0u8), (7, 6), (8, 1)] {
        let record = DoviRecord {
            profile,
            level: 6,
            rpu: true,
            el: false,
            bl: true,
            bl_compat_id: compat,
        };
        let dv = |container: &'static str| {
            let mut e = hdr(
                Expect::video(container, "hevc", 128, 72, 24).audio("aac", 2, None),
                "Dolby Vision",
            );
            e.dovi_profile = Some(profile);
            e.dovi_bl_compat_id = Some(compat);
            e
        };
        let encode = ffmpeg(&[
            test_pattern("128x72", 24, "1"),
            silence("stereo", 44100, "1"),
            x265("smpte2084", true, false),
            audio("aac", Some("96k")),
            out("mp4"),
        ]);
        let mp4_name = format!("dv_profile{profile}.mp4");
        if profile == 5 {
            // Profile 5 ships only as Matroska here; the MP4 is an intermediate.
            v.push(fixture(
                &format!("dv_profile{profile}.mkv"),
                Dv,
                vec![
                    Step::Ffmpeg(rewrite_out(&encode, "{work}/dv.mp4")),
                    Step::Post(Post::DolbyVisionConfigAt {
                        path: "{work}/dv.mp4".into(),
                        record,
                    }),
                    Step::Ffmpeg(args(&[
                        "-i",
                        "{work}/dv.mp4",
                        "-c",
                        "copy",
                        "-f",
                        "matroska",
                        "{out}",
                    ])),
                ],
                dv("matroska"),
                &["libx265", "aac"],
            ));
            continue;
        }
        v.push(fixture(
            &mp4_name,
            Dv,
            vec![encode, Step::Post(Post::DolbyVisionConfig(record))],
            dv("mp4"),
            &["libx265", "aac"],
        ));
        v.push(fixture(
            &format!("dv_profile{profile}.mkv"),
            Dv,
            vec![Step::Ffmpeg(vec![
                "-i".into(),
                format!("{{media}}/{mp4_name}"),
                "-c".into(),
                "copy".into(),
                "-f".into(),
                "matroska".into(),
                "{out}".into(),
            ])],
            dv("matroska"),
            &[],
        ));
    }

    // MPEG-1 and MPEG-2 program streams with MP2 audio.
    for (name, codec, muxer) in [
        ("ps_mpeg1_mp2.mpg", "mpeg1video", "mpeg"),
        ("ps_mpeg2_mp2.vob", "mpeg2video", "vob"),
    ] {
        let mut e = Expect::video("mpeg", codec, 160, 120, 25).audio("mp2", 1, None);
        e.min_duration_seconds = 1;
        v.push(fixture(
            name,
            Sup,
            vec![Step::Ffmpeg(
                [
                    test_pattern("160x120", 25, "2"),
                    args(&[
                        "-f",
                        "lavfi",
                        "-i",
                        "sine=frequency=440:sample_rate=48000:duration=2",
                        "-c:v",
                        codec,
                        "-b:v",
                        "200k",
                        "-c:a",
                        "mp2",
                        "-b:a",
                        "128k",
                        "-ac",
                        "1",
                        "-f",
                        muxer,
                        "{out}",
                    ]),
                ]
                .concat(),
            )],
            e,
            &[
                if codec == "mpeg1video" {
                    "mpeg1video"
                } else {
                    "mpeg2video"
                },
                "mp2",
            ],
        ));
    }
    v
}

fn rewrite_out(step: &Step, to: &str) -> Vec<String> {
    match step {
        Step::Ffmpeg(a) => a
            .iter()
            .map(|arg| {
                if arg == "{out}" {
                    to.to_owned()
                } else {
                    arg.clone()
                }
            })
            .collect(),
        _ => unreachable!("only FFmpeg steps are rewritten"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dolby_vision_fixtures_cover_profiles_5_7_8() {
        let all = fixtures();
        for (name, profile) in [
            ("dv_profile5.mkv", 5),
            ("dv_profile7.mkv", 7),
            ("dv_profile7.mp4", 7),
            ("dv_profile8.mkv", 8),
            ("dv_profile8.mp4", 8),
        ] {
            let f = all
                .iter()
                .find(|f| f.name == name)
                .unwrap_or_else(|| panic!("{name}"));
            assert_eq!(f.expect.dovi_profile, Some(profile));
        }
    }

    #[test]
    fn derived_fixtures_come_after_their_sources() {
        let all = fixtures();
        let position = |name: &str| all.iter().position(|f| f.name == name).unwrap();
        assert!(position("hevc_sequence_pq.mp4") < position("chapters_nero.mp4"));
        assert!(position("dv_profile8.mp4") < position("dv_profile8.mkv"));
        assert!(position("av1_sequence_pq.mp4") < position("av1_sequence_pq.mkv"));
    }
}
