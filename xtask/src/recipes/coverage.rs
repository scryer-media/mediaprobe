//! The coverage matrix: one fixture per signal the parser classifies, in
//! every container that can carry it.
//!
//! The axes come from the parser's own tables and branches: the codec-id and
//! fourcc maps, the H.264/HEVC/AV1/VP9 profile names, the HDR priority order
//! (Dolby Vision > HDR10+ > HDR10 > HLG), every place a Dolby Vision
//! configuration is read (MP4 dvcC/dvvC/dvwC, Matroska BlockAdditionMapping,
//! the TS DOVI descriptor), SEI payloads 4/137/144, AV1 metadata OBUs, A/53
//! caption user data, the Matroska and MP4 disposition flags, BCP-47 and
//! legacy language fields, rotation, pixel aspect, field order, stereo 3D,
//! variable frame rate, programs, chapters, attachments, scrambling, and the
//! warning codes those branches raise.
//!
//! Everything is small: 64x64 or 128x72 pictures, one second, silent audio.
//! FFmpeg writes every container; where no encoder can be asked for a
//! signal, `inject.rs` authors it into the elementary stream before FFmpeg
//! muxes it, and `post.rs` inserts configuration boxes afterwards.

use super::inject::{Inject, NalCodec, NalInsert};
use super::matrix::args;
use super::post::{DoviRecord, Post};
use super::{Expect, Fixture, Generator, Step};

const MASTER_X265: &str = "G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,50)";
const MASTER_X264: &str = "G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,50)";
const MASTER_SVT: &str = "G(0.265,0.690)B(0.150,0.060)R(0.680,0.320)WP(0.3127,0.3290)L(1000,0.005)";

/// Small HDR10+ dynamic metadata for x265's `dhdr10-info` (one scene).
pub(crate) const HDR10PLUS_JSON: &str = r#"{"JSONInfo":{"HDR10plusProfile":"B","Version":"1.0"},"SceneInfo":[{"BezierCurveData":{"Anchors":[256,512,768,1024,1280,1536,1792,2048,2304],"KneePointX":0,"KneePointY":0},"LuminanceParameters":{"AverageRGB":1000,"LuminanceDistributions":{"DistributionIndex":[1,5,10,25,50,75,90,95,99],"DistributionValues":[0,1,2,3,4,5,6,7,8]},"MaxScl":[1000,1000,1000]},"NumberOfWindows":1,"TargetedSystemDisplayMaximumLuminance":400,"SceneFrameNumbers":0,"SceneId":0,"SequenceFrameNumber":0}],"SceneInfoSummary":{"SceneFirstFrameIndex":[0],"SceneFrameNumbers":[24]},"ToolInfo":{"Tool":"mediaprobe-fixture","Version":"1"}}"#;

const SIZE: &str = "64x64";
const W: i32 = 64;
const H: i32 = 64;

fn s(list: &[&str]) -> Vec<String> {
    args(list)
}

/// testsrc2 at `size`, 24 fps, `seconds` long.
fn pattern(size: &str, rate: u32, seconds: &str) -> Vec<String> {
    vec![
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
        format!("testsrc2=size={size}:rate={rate}:duration={seconds}"),
    ]
}

fn picture() -> Vec<String> {
    pattern(SIZE, 24, "1")
}

fn silence(layout: &str, rate: u32) -> Vec<String> {
    vec![
        "-f".into(),
        "lavfi".into(),
        "-t".into(),
        "1".into(),
        "-i".into(),
        format!("anullsrc=r={rate}:cl={layout}"),
    ]
}

fn ffmpeg(parts: Vec<Vec<String>>) -> Step {
    Step::Ffmpeg(parts.concat())
}

fn to(muxer: &str, path: &str) -> Vec<String> {
    s(&["-f", muxer, path])
}

fn out(muxer: &str) -> Vec<String> {
    to(muxer, "{out}")
}

fn remux(from: &str, muxer: &str, extra: &[&str]) -> Step {
    let mut a = s(&["-i", from, "-map", "0", "-c", "copy"]);
    a.extend(s(extra));
    a.extend(out(muxer));
    Step::Ffmpeg(a)
}

fn write(name: &str, contents: &str) -> Step {
    Step::WriteFile {
        name: name.into(),
        contents: contents.into(),
    }
}

fn post(pass: Post) -> Step {
    Step::Post(pass)
}

fn inject(from: &str, to: &str, inject: Inject) -> Step {
    post(Post::Inject {
        from: from.into(),
        to: to.into(),
        inject,
    })
}

/// Container name the parser reports, per muxer.
fn container(muxer: &str) -> &'static str {
    match muxer {
        "matroska" => "matroska",
        "webm" => "webm",
        "mp4" | "ismv" => "mp4",
        "mov" => "mov",
        "mpegts" => "mpegts",
        "flv" => "flv",
        "avi" => "avi",
        "asf" => "asf",
        "ogg" => "ogg",
        "mpeg" | "vob" | "dvd" => "mpeg",
        "rm" => "rm",
        "dv" => "dv",
        "3gp" => "mp4",
        _ => "unknown",
    }
}

fn ext(muxer: &str) -> &'static str {
    match muxer {
        "matroska" => "mkv",
        "webm" => "webm",
        "mp4" => "mp4",
        "mov" => "mov",
        "mpegts" => "ts",
        "flv" => "flv",
        "avi" => "avi",
        "asf" => "wmv",
        "ogg" => "ogv",
        "vob" => "vob",
        "mpeg" => "mpg",
        "rm" => "rm",
        "dv" => "dv",
        "3gp" => "3gp",
        _ => "bin",
    }
}

struct F {
    name: String,
    steps: Vec<Step>,
    expect: Expect,
    encoders: Vec<&'static str>,
}

impl F {
    fn new(
        name: impl Into<String>,
        steps: Vec<Step>,
        expect: Expect,
        encoders: &[&'static str],
    ) -> Self {
        F {
            name: name.into(),
            steps,
            expect,
            encoders: encoders.to_vec(),
        }
    }
}

fn finish(list: Vec<F>) -> Vec<Fixture> {
    list.into_iter()
        .map(|f| Fixture {
            name: f.name,
            generator: Generator::Coverage,
            steps: f.steps,
            expect: f.expect,
            encoders: f.encoders,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Encoders
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Transfer {
    Sdr,
    Pq,
    Hlg,
}

impl Transfer {
    fn ffmpeg(self) -> &'static str {
        match self {
            Transfer::Sdr => "bt709",
            Transfer::Pq => "smpte2084",
            Transfer::Hlg => "arib-std-b67",
        }
    }

    fn code(self) -> i64 {
        match self {
            Transfer::Sdr => 1,
            Transfer::Pq => 16,
            Transfer::Hlg => 18,
        }
    }
}

/// libx265 options. `static_hdr` adds mastering display and content light
/// SEI; `dynamic` adds HDR10+ from `{work}/hdr10plus.json`; `container_trc`
/// overrides the transfer FFmpeg writes into the container (for conflicts).
fn x265(
    pix_fmt: &str,
    transfer: Transfer,
    static_hdr: bool,
    dynamic: bool,
    container_trc: Option<&str>,
) -> Vec<String> {
    let (prim, matrix) = if transfer == Transfer::Sdr {
        ("bt709", "bt709")
    } else {
        ("bt2020", "bt2020nc")
    };
    let mut params = format!(
        "log-level=error:colorprim={prim}:transfer={}:colormatrix={matrix}:repeat-headers=1",
        transfer.ffmpeg()
    );
    if static_hdr {
        params.push_str(&format!(
            ":hdr10=1:master-display={MASTER_X265}:max-cll=1000,400"
        ));
    }
    if dynamic {
        params.push_str(":dhdr10-info={work}/hdr10plus.json");
    }
    let mut a = s(&[
        "-c:v",
        "libx265",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        pix_fmt,
    ]);
    a.extend(container_colour(
        prim,
        matrix,
        container_trc.unwrap_or(transfer.ffmpeg()),
    ));
    a.extend(["-x265-params".into(), params]);
    a
}

/// The colour FFmpeg writes into the container (Matroska Colour, MP4 colr,
/// TS and FLV carry none). FFmpeg 8 takes it from the frames, and testsrc2
/// frames carry none, so the encoder options alone write nothing: the frames
/// are tagged here. The encoders' own parameters, applied after, set what the
/// bitstream says, so the two can be made to disagree.
fn container_colour(primaries: &str, matrix: &str, transfer: &str) -> Vec<String> {
    let tag =
        format!("setparams=color_primaries={primaries}:color_trc={transfer}:colorspace={matrix}");
    s(&[
        "-vf",
        &tag,
        "-color_primaries",
        primaries,
        "-colorspace",
        matrix,
        "-color_trc",
        transfer,
    ])
}

fn x265_hdr10() -> Vec<String> {
    x265("yuv420p10le", Transfer::Pq, true, false, None)
}

/// x265 arguments without B-frames, for streams that are written raw and
/// timestamped on remux: decode order is presentation order.
fn no_bframes(mut args: Vec<String>) -> Vec<String> {
    let at = args
        .iter()
        .position(|a| a == "-x265-params")
        .expect("x265 arguments")
        + 1;
    args[at].push_str(":bframes=0");
    args
}

/// Read a raw elementary stream and copy it with frame-count timestamps.
/// FFmpeg's raw demuxers leave packets untimed; the stream must have no
/// frame reordering (see `no_bframes`).
fn raw_copy(format: &str, rate: u32, path: &str) -> Vec<String> {
    let mut a = s(&[
        "-framerate",
        &rate.to_string(),
        "-f",
        format,
        "-i",
        path,
        "-c",
        "copy",
    ]);
    a.extend(s(&["-bsf:v", &format!("setts=ts=N/({rate}*TB)")]));
    a
}

/// libx264's profile is a ceiling: the stream declares the lowest profile its
/// tools need, and the ultrafast preset turns CABAC and 8x8 transforms off,
/// which leaves every stream Constrained Baseline. Main and the High family
/// turn the tools of their profile back on so the stream declares it.
fn x264(pix_fmt: &str, profile: &str) -> Vec<String> {
    let mut a = s(&[
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        pix_fmt,
        "-profile:v",
        profile,
    ]);
    if profile == "main" {
        a.extend(s(&["-coder", "1"]));
    } else if profile.starts_with("high") {
        a.extend(s(&["-coder", "1", "-8x8dct", "1"]));
    }
    a
}

fn x264_plain() -> Vec<String> {
    x264("yuv420p", "high")
}

fn x264_hdr(transfer: Transfer, static_hdr: bool, container_trc: Option<&str>) -> Vec<String> {
    let mut a = s(&[
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p10le",
        "-profile:v",
        "high10",
    ]);
    a.extend(container_colour(
        "bt2020",
        "bt2020nc",
        container_trc.unwrap_or(transfer.ffmpeg()),
    ));
    let mut params = format!(
        "colorprim=bt2020:transfer={}:colormatrix=bt2020nc",
        transfer.ffmpeg()
    );
    if static_hdr {
        params.push_str(&format!(":mastering-display={MASTER_X264}:cll=1000,400"));
    }
    a.extend(["-x264-params".into(), params]);
    a
}

fn svtav1(
    pix_fmt: &str,
    transfer: Transfer,
    static_hdr: bool,
    container_trc: Option<&str>,
) -> Vec<String> {
    let (prim, matrix, p, m) = if transfer == Transfer::Sdr {
        ("bt709", "bt709", 1, 1)
    } else {
        ("bt2020", "bt2020nc", 9, 9)
    };
    let mut a = s(&["-c:v", "libsvtav1", "-preset", "12", "-pix_fmt", pix_fmt]);
    a.extend(container_colour(
        prim,
        matrix,
        container_trc.unwrap_or(transfer.ffmpeg()),
    ));
    let mut params = format!(
        "color-primaries={p}:transfer-characteristics={}:matrix-coefficients={m}",
        transfer.code()
    );
    if static_hdr {
        params.push_str(&format!(
            ":mastering-display={MASTER_SVT}:content-light=1000,400"
        ));
    }
    a.extend(["-svtav1-params".into(), params]);
    a
}

fn vp9(pix_fmt: &str, transfer: Option<Transfer>) -> Vec<String> {
    let mut a = s(&[
        "-c:v",
        "libvpx-vp9",
        "-deadline",
        "realtime",
        "-cpu-used",
        "8",
        "-b:v",
        "200k",
        "-pix_fmt",
        pix_fmt,
    ]);
    if let Some(t) = transfer {
        let (prim, matrix) = if t == Transfer::Sdr {
            ("bt709", "bt709")
        } else {
            ("bt2020", "bt2020nc")
        };
        a.extend(container_colour(prim, matrix, t.ffmpeg()));
    }
    a
}

fn audio(codec: &str) -> Vec<String> {
    let mut a = s(&["-c:a", codec]);
    match codec {
        "aac" | "libmp3lame" | "mp2" | "ac3" | "eac3" | "libopus" | "opus" | "vorbis" | "wmav1"
        | "wmav2" => {
            a.extend(s(&["-b:a", "96k"]));
        }
        // The DTS core encoder refuses rates below its table for the layout.
        "dca" => a.extend(s(&["-b:a", "768k"])),
        _ => {}
    }
    if matches!(
        codec,
        "truehd" | "mlp" | "dca" | "vorbis" | "opus" | "s302m"
    ) {
        a.extend(s(&["-strict", "-2"]));
    }
    a
}

// ---------------------------------------------------------------------------
// Expectations
// ---------------------------------------------------------------------------

fn colour(e: Expect, transfer: Transfer) -> Expect {
    let e = e.check("video.metadata.color.transfer", transfer.code());
    if transfer == Transfer::Sdr {
        e
    } else {
        e.check("video.metadata.color.primaries", 9i64)
            .check("video.metadata.color.matrix", 9i64)
    }
}

fn hdr10(e: Expect) -> Expect {
    // The mastering minimum is 0.005 cd/m2. HEVC and H.264 SEI carry it in
    // 0.0001 units (exactly 50); AV1 metadata_hdr_mdcv carries 0.14 fixed
    // point, so 0.005 is stored as 82/16384.
    let min_luminance = if e.video_codec == Some("av1") {
        82.0 / 16384.0
    } else {
        0.005
    };
    let mut e = colour(e, Transfer::Pq)
        .check("video.metadata.hdr.hdr10", true)
        .check("video.metadata.hdr.pq", true)
        .check(
            "video.metadata.color.mastering_display.max_luminance",
            1000.0,
        )
        .check(
            "video.metadata.color.mastering_display.min_luminance",
            min_luminance,
        )
        .check("video.metadata.color.content_light.max_cll", 1000i64)
        .check("video.metadata.color.content_light.max_fall", 400i64);
    e.video_bit_depth = Some(10);
    e.hdr_format = Some("HDR10");
    e
}

fn pq_bare(e: Expect) -> Expect {
    let mut e = colour(e, Transfer::Pq)
        .check("video.metadata.hdr.pq", true)
        .absent("video.metadata.color.mastering_display")
        .absent("video.metadata.color.content_light");
    e.video_bit_depth = Some(10);
    e
}

fn hlg(e: Expect) -> Expect {
    let mut e = colour(e, Transfer::Hlg).check("video.metadata.hdr.hlg", true);
    e.video_bit_depth = Some(10);
    e.hdr_format = Some("HLG");
    e
}

fn hdr10plus(e: Expect) -> Expect {
    let mut e = colour(e, Transfer::Pq).check("video.metadata.hdr.hdr10plus", true);
    e.video_bit_depth = Some(10);
    e.hdr_format = Some("HDR10+");
    e
}

fn dovi(e: Expect, record: DoviRecord) -> Expect {
    let mut e = e
        .check("video.metadata.hdr.dolby_vision", true)
        .check("video.metadata.hdr.dovi.profile", i64::from(record.profile))
        .check("video.metadata.hdr.dovi.level", i64::from(record.level))
        .check("video.metadata.hdr.dovi.rpu_present", record.rpu)
        .check(
            "video.metadata.hdr.dovi.enhancement_layer_present",
            record.el,
        )
        .check("video.metadata.hdr.dovi.base_layer_present", record.bl)
        .check(
            "video.metadata.hdr.dovi.base_layer_compatibility_id",
            i64::from(record.bl_compat_id),
        );
    e.hdr_format = Some("Dolby Vision");
    e.dovi_profile = Some(record.profile);
    e.dovi_bl_compat_id = Some(record.bl_compat_id);
    e
}

// ---------------------------------------------------------------------------
// HDR
// ---------------------------------------------------------------------------

/// Muxers that carry each video codec, with the extra flags they need.
const HEVC_MUXERS: &[(&str, &[&str])] = &[
    ("mp4", &["-tag:v", "hvc1"]),
    ("matroska", &[]),
    ("mov", &["-tag:v", "hvc1"]),
    ("mpegts", &[]),
    ("flv", &[]),
];
const AV1_MUXERS: &[(&str, &[&str])] =
    &[("mp4", &[]), ("matroska", &[]), ("webm", &[]), ("flv", &[])];
const VP9_MUXERS: &[(&str, &[&str])] = &[("webm", &[]), ("matroska", &[]), ("mp4", &[])];
const H264_MUXERS: &[(&str, &[&str])] = &[("matroska", &[]), ("mp4", &[]), ("mpegts", &[])];

fn hdr_fixtures(v: &mut Vec<F>) {
    let hdr10plus_json = || write("hdr10plus.json", HDR10PLUS_JSON);

    for (muxer, flags) in HEVC_MUXERS {
        let base = || Expect::video(container(muxer), "hevc", W, H, 24);
        let x = ext(muxer);
        v.push(F::new(
            format!("hdr/hevc_hdr10.{x}"),
            vec![ffmpeg(vec![picture(), x265_hdr10(), s(flags), out(muxer)])],
            hdr10(base()),
            &["libx265"],
        ));
        v.push(F::new(
            format!("hdr/hevc_pq_no_static.{x}"),
            vec![ffmpeg(vec![
                picture(),
                x265("yuv420p10le", Transfer::Pq, false, false, None),
                s(flags),
                out(muxer),
            ])],
            pq_bare(base()),
            &["libx265"],
        ));
        v.push(F::new(
            format!("hdr/hevc_hlg.{x}"),
            vec![ffmpeg(vec![
                picture(),
                x265("yuv420p10le", Transfer::Hlg, false, false, None),
                s(flags),
                out(muxer),
            ])],
            hlg(base()),
            &["libx265"],
        ));
        v.push(F::new(
            format!("hdr/hevc_hdr10plus.{x}"),
            vec![
                hdr10plus_json(),
                ffmpeg(vec![
                    picture(),
                    x265("yuv420p10le", Transfer::Pq, true, true, None),
                    s(flags),
                    out(muxer),
                ]),
            ],
            hdr10plus(base()).check("video.metadata.hdr.hdr10", true),
            &["libx265"],
        ));
    }
    // M2TS (192-byte packets) for each HEVC HDR form.
    for (stem, encode, expect) in [
        (
            "hevc_hdr10",
            x265_hdr10(),
            hdr10(Expect::video("mpegts", "hevc", W, H, 24)),
        ),
        (
            "hevc_hlg",
            x265("yuv420p10le", Transfer::Hlg, false, false, None),
            hlg(Expect::video("mpegts", "hevc", W, H, 24)),
        ),
    ] {
        v.push(F::new(
            format!("hdr/{stem}.m2ts"),
            vec![ffmpeg(vec![
                picture(),
                encode,
                s(&["-mpegts_m2ts_mode", "1"]),
                out("mpegts"),
            ])],
            expect,
            &["libx265"],
        ));
    }
    v.push(F::new(
        "hdr/hevc_hdr10plus.m2ts",
        vec![
            hdr10plus_json(),
            ffmpeg(vec![
                picture(),
                x265("yuv420p10le", Transfer::Pq, true, true, None),
                s(&["-mpegts_m2ts_mode", "1"]),
                out("mpegts"),
            ]),
        ],
        hdr10plus(Expect::video("mpegts", "hevc", W, H, 24)),
        &["libx265"],
    ));
    // HEVC HDR10+ authored as SEI by Rust (no x265 JSON), with and without static metadata.
    for (stem, inserts, static_hdr) in [
        ("hevc_hdr10plus_sei_only", vec![NalInsert::Hdr10Plus], false),
        (
            "hevc_hdr10plus_sei_static",
            vec![NalInsert::Hdr10Plus, NalInsert::StaticHdr],
            false,
        ),
    ] {
        for (muxer, flags) in [
            ("matroska", &[][..]),
            ("mp4", &["-tag:v", "hvc1"][..]),
            ("mpegts", &[][..]),
        ] {
            let mut e = hdr10plus(Expect::video(container(muxer), "hevc", W, H, 24));
            if inserts.contains(&NalInsert::StaticHdr) {
                e = e
                    .present("video.metadata.color.mastering_display")
                    .check("video.metadata.color.content_light.max_cll", 1000i64);
            }
            v.push(F::new(
                format!("hdr/{stem}.{}", ext(muxer)),
                vec![
                    ffmpeg(vec![
                        picture(),
                        no_bframes(x265("yuv420p10le", Transfer::Pq, static_hdr, false, None)),
                        to("hevc", "{work}/v.hevc"),
                    ]),
                    inject(
                        "{work}/v.hevc",
                        "{work}/i.hevc",
                        Inject::AnnexB {
                            codec: NalCodec::Hevc,
                            inserts: inserts.clone(),
                        },
                    ),
                    ffmpeg(vec![
                        raw_copy("hevc", 24, "{work}/i.hevc"),
                        s(flags),
                        out(muxer),
                    ]),
                ],
                e,
                &["libx265"],
            ));
        }
    }

    // AV1.
    for (muxer, flags) in AV1_MUXERS {
        let base = || Expect::video(container(muxer), "av1", W, H, 24);
        let x = ext(muxer);
        v.push(F::new(
            format!("hdr/av1_hdr10.{x}"),
            vec![ffmpeg(vec![
                picture(),
                svtav1("yuv420p10le", Transfer::Pq, true, None),
                s(flags),
                out(muxer),
            ])],
            hdr10(base()),
            &["libsvtav1"],
        ));
        v.push(F::new(
            format!("hdr/av1_pq_no_static.{x}"),
            vec![ffmpeg(vec![
                picture(),
                svtav1("yuv420p10le", Transfer::Pq, false, None),
                s(flags),
                out(muxer),
            ])],
            pq_bare(base()),
            &["libsvtav1"],
        ));
        v.push(F::new(
            format!("hdr/av1_hlg.{x}"),
            vec![ffmpeg(vec![
                picture(),
                svtav1("yuv420p10le", Transfer::Hlg, false, None),
                s(flags),
                out(muxer),
            ])],
            hlg(base()),
            &["libsvtav1"],
        ));
        if *muxer != "flv" {
            v.push(F::new(
                format!("hdr/av1_hdr10plus.{x}"),
                vec![
                    ffmpeg(vec![
                        picture(),
                        svtav1("yuv420p10le", Transfer::Pq, true, None),
                        to("obu", "{work}/v.obu"),
                    ]),
                    inject("{work}/v.obu", "{work}/i.obu", Inject::Av1Hdr10Plus),
                    ffmpeg(vec![
                        raw_copy("obu", 24, "{work}/i.obu"),
                        s(flags),
                        out(muxer),
                    ]),
                ],
                hdr10plus(base()).check("video.metadata.hdr.hdr10", true),
                &["libsvtav1"],
            ));
        }
    }

    // VP9: static metadata lives in the container (WebM/Matroska Colour,
    // MP4 SmDm/CoLL), so the HDR10 variant is transcoded from an HEVC HDR10
    // source whose side data FFmpeg carries across.
    for (muxer, flags) in VP9_MUXERS {
        let base = || {
            Expect::video(container(muxer), "vp9", W, H, 24)
                .check("video.metadata.profile", "Profile 2")
        };
        let x = ext(muxer);
        v.push(F::new(
            format!("hdr/vp9_hdr10.{x}"),
            vec![
                ffmpeg(vec![
                    picture(),
                    x265_hdr10(),
                    to("matroska", "{work}/src.mkv"),
                ]),
                ffmpeg(vec![
                    s(&["-i", "{work}/src.mkv"]),
                    vp9("yuv420p10le", Some(Transfer::Pq)),
                    s(flags),
                    out(muxer),
                ]),
            ],
            hdr10(base()),
            &["libx265", "libvpx-vp9"],
        ));
        v.push(F::new(
            format!("hdr/vp9_pq_no_static.{x}"),
            vec![ffmpeg(vec![
                picture(),
                vp9("yuv420p10le", Some(Transfer::Pq)),
                s(flags),
                out(muxer),
            ])],
            pq_bare(base()),
            &["libvpx-vp9"],
        ));
        v.push(F::new(
            format!("hdr/vp9_hlg.{x}"),
            vec![ffmpeg(vec![
                picture(),
                vp9("yuv420p10le", Some(Transfer::Hlg)),
                s(flags),
                out(muxer),
            ])],
            hlg(base()),
            &["libvpx-vp9"],
        ));
    }

    // H.264 High 10 with HDR signalling.
    for (muxer, flags) in H264_MUXERS {
        let base = || {
            Expect::video(container(muxer), "h264", W, H, 24)
                .check("video.metadata.profile", "High 10")
        };
        let x = ext(muxer);
        v.push(F::new(
            format!("hdr/h264_hdr10.{x}"),
            vec![ffmpeg(vec![
                picture(),
                x264_hdr(Transfer::Pq, true, None),
                s(flags),
                out(muxer),
            ])],
            hdr10(base()),
            &["libx264"],
        ));
        v.push(F::new(
            format!("hdr/h264_hlg.{x}"),
            vec![ffmpeg(vec![
                picture(),
                x264_hdr(Transfer::Hlg, false, None),
                s(flags),
                out(muxer),
            ])],
            hlg(base()),
            &["libx264"],
        ));
    }

    // Conflicting signals: the bitstream says PQ, the container says BT.709.
    for (muxer, flags) in [
        ("matroska", &[][..]),
        ("mp4", &["-tag:v", "hvc1", "-movflags", "+write_colr"][..]),
        ("mov", &["-tag:v", "hvc1"][..]),
    ] {
        v.push(F::new(
            format!("hdr/hevc_pq_container_bt709.{}", ext(muxer)),
            vec![ffmpeg(vec![
                picture(),
                x265("yuv420p10le", Transfer::Pq, true, false, Some("bt709")),
                s(flags),
                out(muxer),
            ])],
            hdr10(Expect::video(container(muxer), "hevc", W, H, 24))
                .warning("color_signaling_conflict"),
            &["libx265"],
        ));
    }
    for (muxer, codec, encode, encoder) in [
        (
            "matroska",
            "av1",
            svtav1("yuv420p10le", Transfer::Pq, true, Some("bt709")),
            "libsvtav1",
        ),
        (
            "mp4",
            "av1",
            svtav1("yuv420p10le", Transfer::Pq, true, Some("bt709")),
            "libsvtav1",
        ),
        (
            "matroska",
            "h264",
            x264_hdr(Transfer::Pq, true, Some("bt709")),
            "libx264",
        ),
        (
            "mp4",
            "h264",
            x264_hdr(Transfer::Pq, true, Some("bt709")),
            "libx264",
        ),
    ] {
        v.push(F::new(
            format!("hdr/{codec}_pq_container_bt709.{}", ext(muxer)),
            vec![ffmpeg(vec![
                picture(),
                encode,
                s(&["-movflags", "+write_colr"]),
                out(muxer),
            ])],
            hdr10(Expect::video(container(muxer), codec, W, H, 24))
                .warning("color_signaling_conflict"),
            &[encoder],
        ));
    }
    // Container says PQ over an SDR bitstream: the bitstream wins and the
    // stream is not HDR.
    v.push(F::new(
        "hdr/hevc_sdr_container_pq.mkv",
        vec![ffmpeg(vec![
            picture(),
            x265(
                "yuv420p10le",
                Transfer::Sdr,
                false,
                false,
                Some("smpte2084"),
            ),
            out("matroska"),
        ])],
        colour(Expect::video("matroska", "hevc", W, H, 24), Transfer::Sdr)
            .absent("summary.video_hdr_format")
            .warning("color_signaling_conflict"),
        &["libx265"],
    ));
}

// ---------------------------------------------------------------------------
// Dolby Vision
// ---------------------------------------------------------------------------

fn record(profile: u8, compat: u8, el: bool) -> DoviRecord {
    DoviRecord {
        profile,
        level: 6,
        rpu: true,
        el,
        bl: true,
        bl_compat_id: compat,
    }
}

fn box_type(profile: u8) -> [u8; 4] {
    if profile <= 7 {
        *b"dvcC"
    } else if profile <= 10 {
        *b"dvvC"
    } else {
        *b"dvwC"
    }
}

/// A base-layer encode written to `{work}/bl.mp4`, plus its codec and encoder.
fn base_layer(kind: &str) -> (Vec<String>, &'static str, &'static str, i32) {
    match kind {
        "hevc_pq" => (
            [picture(), x265_hdr10(), s(&["-tag:v", "hvc1"])].concat(),
            "hevc",
            "libx265",
            10,
        ),
        "hevc_pq_hev1" => (
            [picture(), x265_hdr10(), s(&["-tag:v", "hev1"])].concat(),
            "hevc",
            "libx265",
            10,
        ),
        "hevc_sdr" => (
            [
                picture(),
                x265("yuv420p10le", Transfer::Sdr, false, false, None),
                s(&["-tag:v", "hvc1"]),
            ]
            .concat(),
            "hevc",
            "libx265",
            10,
        ),
        "hevc_hlg" => (
            [
                picture(),
                x265("yuv420p10le", Transfer::Hlg, false, false, None),
                s(&["-tag:v", "hvc1"]),
            ]
            .concat(),
            "hevc",
            "libx265",
            10,
        ),
        "avc_sdr" => (
            [picture(), x264("yuv420p", "high")].concat(),
            "h264",
            "libx264",
            8,
        ),
        "av1_pq" => (
            [picture(), svtav1("yuv420p10le", Transfer::Pq, true, None)].concat(),
            "av1",
            "libsvtav1",
            10,
        ),
        "av1_hlg" => (
            [picture(), svtav1("yuv420p10le", Transfer::Hlg, false, None)].concat(),
            "av1",
            "libsvtav1",
            10,
        ),
        _ => unreachable!("unknown base layer {kind}"),
    }
}

struct DoviCase {
    stem: &'static str,
    base: &'static str,
    record: DoviRecord,
    box_type: [u8; 4],
    rename: Option<[u8; 4]>,
    /// Remux the MP4 into these muxers as well.
    remux: &'static [&'static str],
}

/// Copy a Dolby Vision MP4 into another container. FFmpeg 8.1.1 carries the
/// configuration into Matroska itself; its TS and MOV muxers drop it, so the
/// record is written back: a DOVI descriptor in the PMT, or the box in the
/// MOV sample entry.
fn dovi_remux(from: &str, muxer: &str, record: DoviRecord) -> Vec<Step> {
    match muxer {
        "mpegts" => vec![
            Step::Ffmpeg(s(&[
                "-i",
                from,
                "-map",
                "0",
                "-c",
                "copy",
                "-f",
                "mpegts",
                "{work}/plain.ts",
            ])),
            inject(
                "{work}/plain.ts",
                "{out}",
                Inject::TsDoviDescriptor { record },
            ),
        ],
        "mov" => vec![
            remux(from, "mov", &[]),
            post(Post::DoviBox {
                path: "{out}".into(),
                record,
                box_type: box_type(record.profile),
                rename: None,
            }),
        ],
        _ => vec![remux(from, muxer, &[])],
    }
}

fn dovi_fixtures(v: &mut Vec<F>) {
    let cases = [
        DoviCase {
            stem: "p5_hvc1_dvcC",
            base: "hevc_pq",
            record: record(5, 0, false),
            box_type: *b"dvcC",
            rename: None,
            remux: &["matroska", "mpegts"],
        },
        DoviCase {
            stem: "p5_dvh1_dvcC",
            base: "hevc_pq",
            record: record(5, 0, false),
            box_type: *b"dvcC",
            rename: Some(*b"dvh1"),
            remux: &[],
        },
        DoviCase {
            stem: "p5_dvhe_dvcC",
            base: "hevc_pq_hev1",
            record: record(5, 0, false),
            box_type: *b"dvcC",
            rename: Some(*b"dvhe"),
            remux: &[],
        },
        DoviCase {
            stem: "p7_dual_layer_dvcC",
            base: "hevc_pq",
            record: record(7, 6, true),
            box_type: *b"dvcC",
            rename: None,
            remux: &["matroska", "mpegts"],
        },
        DoviCase {
            stem: "p7_single_layer_dvcC",
            base: "hevc_pq",
            record: record(7, 6, false),
            box_type: *b"dvcC",
            rename: None,
            remux: &[],
        },
        DoviCase {
            stem: "p81_hvc1_dvvC",
            base: "hevc_pq",
            record: record(8, 1, false),
            box_type: *b"dvvC",
            rename: None,
            remux: &["matroska", "mpegts", "mov"],
        },
        DoviCase {
            stem: "p81_dvh1_dvvC",
            base: "hevc_pq",
            record: record(8, 1, false),
            box_type: *b"dvvC",
            rename: Some(*b"dvh1"),
            remux: &[],
        },
        DoviCase {
            stem: "p81_hvc1_dvcC",
            base: "hevc_pq",
            record: record(8, 1, false),
            box_type: *b"dvcC",
            rename: None,
            remux: &[],
        },
        DoviCase {
            stem: "p82_sdr_dvvC",
            base: "hevc_sdr",
            record: record(8, 2, false),
            box_type: *b"dvvC",
            rename: None,
            remux: &["matroska"],
        },
        DoviCase {
            stem: "p84_hlg_dvvC",
            base: "hevc_hlg",
            record: record(8, 4, false),
            box_type: *b"dvvC",
            rename: None,
            remux: &["matroska", "mpegts"],
        },
        DoviCase {
            stem: "p9_avc1_dvvC",
            base: "avc_sdr",
            record: record(9, 2, false),
            box_type: *b"dvvC",
            rename: None,
            remux: &["matroska"],
        },
        DoviCase {
            stem: "p9_dvav_dvvC",
            base: "avc_sdr",
            record: record(9, 2, false),
            box_type: *b"dvvC",
            rename: Some(*b"dvav"),
            remux: &[],
        },
        DoviCase {
            stem: "p9_dva1_dvvC",
            base: "avc_sdr",
            record: record(9, 2, false),
            box_type: *b"dvvC",
            rename: Some(*b"dva1"),
            remux: &[],
        },
        DoviCase {
            stem: "p100_av01_dvvC",
            base: "av1_pq",
            record: record(10, 0, false),
            box_type: *b"dvvC",
            rename: None,
            remux: &[],
        },
        DoviCase {
            stem: "p101_av01_dvvC",
            base: "av1_pq",
            record: record(10, 1, false),
            box_type: *b"dvvC",
            rename: None,
            remux: &["matroska"],
        },
        DoviCase {
            stem: "p101_dav1_dvvC",
            base: "av1_pq",
            record: record(10, 1, false),
            box_type: *b"dvvC",
            rename: Some(*b"dav1"),
            remux: &[],
        },
        DoviCase {
            stem: "p104_av01_dvvC",
            base: "av1_hlg",
            record: record(10, 4, false),
            box_type: *b"dvvC",
            rename: None,
            remux: &["matroska"],
        },
        DoviCase {
            stem: "p20_hvc1_dvwC",
            base: "hevc_pq",
            record: record(20, 0, false),
            box_type: *b"dvwC",
            rename: None,
            remux: &[],
        },
    ];
    for case in &cases {
        debug_assert!(
            case.box_type == box_type(case.record.profile) || case.stem.ends_with("dvcC")
        );
        let (encode, codec, encoder, depth) = base_layer(case.base);
        let mut e = dovi(Expect::video("mp4", codec, W, H, 24), case.record);
        e.video_bit_depth = Some(depth);
        let mp4 = format!("dovi/{}.mp4", case.stem);
        v.push(F::new(
            &mp4,
            vec![
                ffmpeg(vec![encode, out("mp4")]),
                post(Post::DoviBox {
                    path: "{out}".into(),
                    record: case.record,
                    box_type: case.box_type,
                    rename: case.rename,
                }),
            ],
            e.clone(),
            &[encoder],
        ));
        for muxer in case.remux {
            let mut e = e.clone();
            e.container = container(muxer);
            v.push(F::new(
                format!(
                    "dovi/{}.{}",
                    case.stem,
                    if *muxer == "mpegts" { "ts" } else { ext(muxer) }
                ),
                dovi_remux(&format!("{{media}}/{mp4}"), muxer, case.record),
                e,
                &[],
            ));
        }
    }
    // Fragmented MP4 and Matroska written by FFmpeg from the DV MP4's side data.
    let p81 = record(8, 1, false);
    let mut e = dovi(Expect::video("mp4", "hevc", W, H, 24), p81);
    e.video_bit_depth = Some(10);
    v.push(F::new(
        "dovi/p81_fragmented.mp4",
        vec![remux(
            "{media}/dovi/p81_hvc1_dvvC.mp4",
            "mp4",
            &[
                "-movflags",
                "+frag_keyframe+empty_moov+default_base_moof",
                "-strict",
                "unofficial",
            ],
        )],
        e,
        &[],
    ));

    // RPU NAL units in the bitstream.
    let hevc_rpu = |inserts: Vec<NalInsert>| {
        vec![
            ffmpeg(vec![
                picture(),
                no_bframes(x265_hdr10()),
                to("hevc", "{work}/v.hevc"),
            ]),
            inject(
                "{work}/v.hevc",
                "{work}/i.hevc",
                Inject::AnnexB {
                    codec: NalCodec::Hevc,
                    inserts,
                },
            ),
        ]
    };
    let raw_in = || raw_copy("hevc", 24, "{work}/i.hevc");
    // RPU without any configuration record: the parser reads Dolby Vision
    // from configuration records only, so these classify by their base layer.
    for muxer in ["matroska", "mp4", "mpegts"] {
        let mut steps = hevc_rpu(vec![NalInsert::DoviRpu]);
        steps.push(ffmpeg(vec![
            raw_in(),
            if muxer == "mp4" {
                s(&["-tag:v", "hvc1"])
            } else {
                vec![]
            },
            out(muxer),
        ]));
        v.push(F::new(
            format!("dovi/rpu_only_no_config.{}", ext(muxer)),
            steps,
            hdr10(Expect::video(container(muxer), "hevc", W, H, 24))
                .absent("video.metadata.hdr.dovi"),
            &["libx265"],
        ));
    }
    // RPU plus configuration record (profile 8.1), then remuxes.
    {
        let mut steps = hevc_rpu(vec![NalInsert::DoviRpu]);
        steps.push(ffmpeg(vec![raw_in(), s(&["-tag:v", "hvc1"]), out("mp4")]));
        steps.push(post(Post::DoviBox {
            path: "{out}".into(),
            record: p81,
            box_type: *b"dvvC",
            rename: None,
        }));
        let mut e = dovi(Expect::video("mp4", "hevc", W, H, 24), p81);
        e.video_bit_depth = Some(10);
        v.push(F::new("dovi/p81_rpu.mp4", steps, e.clone(), &["libx265"]));
        for muxer in ["matroska", "mpegts"] {
            let mut e = e.clone();
            e.container = container(muxer);
            v.push(F::new(
                format!("dovi/p81_rpu.{}", ext(muxer)),
                dovi_remux("{media}/dovi/p81_rpu.mp4", muxer, p81),
                e,
                &[],
            ));
        }
    }
    // Everything at once: Dolby Vision 8.1 + HDR10+ + HDR10 fallback.
    {
        let mut steps = vec![
            write("hdr10plus.json", HDR10PLUS_JSON),
            ffmpeg(vec![
                picture(),
                no_bframes(x265("yuv420p10le", Transfer::Pq, true, true, None)),
                to("hevc", "{work}/v.hevc"),
            ]),
            inject(
                "{work}/v.hevc",
                "{work}/i.hevc",
                Inject::AnnexB {
                    codec: NalCodec::Hevc,
                    inserts: vec![NalInsert::DoviRpu],
                },
            ),
        ];
        steps.push(ffmpeg(vec![raw_in(), s(&["-tag:v", "hvc1"]), out("mp4")]));
        steps.push(post(Post::DoviBox {
            path: "{out}".into(),
            record: p81,
            box_type: *b"dvvC",
            rename: None,
        }));
        let mut e = dovi(hdr10(Expect::video("mp4", "hevc", W, H, 24)), p81)
            .check("video.metadata.hdr.hdr10plus", true);
        e.hdr_format = Some("Dolby Vision");
        v.push(F::new(
            "dovi/p81_hdr10plus_hdr10.mp4",
            steps,
            e.clone(),
            &["libx265"],
        ));
        for muxer in ["matroska", "mpegts"] {
            let mut e = e.clone();
            e.container = container(muxer);
            v.push(F::new(
                format!("dovi/p81_hdr10plus_hdr10.{}", ext(muxer)),
                dovi_remux("{media}/dovi/p81_hdr10plus_hdr10.mp4", muxer, p81),
                e,
                &[],
            ));
        }
    }
    // AV1 profile 10.1 with an HDR10+ metadata OBU.
    {
        let p101 = record(10, 1, false);
        let steps = vec![
            ffmpeg(vec![
                picture(),
                svtav1("yuv420p10le", Transfer::Pq, true, None),
                to("obu", "{work}/v.obu"),
            ]),
            inject("{work}/v.obu", "{work}/i.obu", Inject::Av1Hdr10Plus),
            ffmpeg(vec![raw_copy("obu", 24, "{work}/i.obu"), out("mp4")]),
            post(Post::DoviBox {
                path: "{out}".into(),
                record: p101,
                box_type: *b"dvvC",
                rename: None,
            }),
        ];
        let mut e = dovi(hdr10(Expect::video("mp4", "av1", W, H, 24)), p101)
            .check("video.metadata.hdr.hdr10plus", true);
        e.hdr_format = Some("Dolby Vision");
        v.push(F::new(
            "dovi/p101_av1_hdr10plus.mp4",
            steps,
            e,
            &["libsvtav1"],
        ));
    }
}

// ---------------------------------------------------------------------------
// Codecs: profiles, bit depths, chroma formats
// ---------------------------------------------------------------------------

fn codec_fixtures(v: &mut Vec<F>) {
    let profile = |e: Expect, p: &str, fmt: &str, depth: i64| {
        e.check("video.metadata.profile", p)
            .check("video.metadata.pixel_format", fmt)
            .check("video.metadata.bit_depth", depth)
    };
    // H.264.
    for (stem, pix, prof, name, depth) in [
        ("baseline", "yuv420p", "baseline", "Constrained Baseline", 8),
        ("main", "yuv420p", "main", "Main", 8),
        ("high", "yuv420p", "high", "High", 8),
        ("high10", "yuv420p10le", "high10", "High 10", 10),
        ("high422", "yuv422p10le", "high422", "High 4:2:2", 10),
        ("high444", "yuv444p", "high444", "High 4:4:4 Predictive", 8),
        (
            "high444_10",
            "yuv444p10le",
            "high444",
            "High 4:4:4 Predictive",
            10,
        ),
    ] {
        v.push(F::new(
            format!("codec/h264_{stem}.mkv"),
            vec![ffmpeg(vec![picture(), x264(pix, prof), out("matroska")])],
            profile(
                Expect::video("matroska", "h264", W, H, 24),
                name,
                pix,
                depth,
            ),
            &["libx264"],
        ));
    }
    v.push(F::new(
        "codec/h264_level41.mp4",
        vec![ffmpeg(vec![
            picture(),
            x264_plain(),
            s(&["-level", "4.1"]),
            out("mp4"),
        ])],
        Expect::video("mp4", "h264", W, H, 24).check("video.metadata.level", 41i64),
        &["libx264"],
    ));
    v.push(F::new(
        "codec/h264_intra_cavlc.mkv",
        vec![ffmpeg(vec![
            picture(),
            x264("yuv420p", "high"),
            s(&["-x264-params", "keyint=1:cabac=0"]),
            out("matroska"),
        ])],
        Expect::video("matroska", "h264", W, H, 24).check("video.metadata.profile", "High"),
        &["libx264"],
    ));
    v.push(F::new(
        "codec/h264_avc3.mp4",
        vec![ffmpeg(vec![
            picture(),
            x264_plain(),
            s(&["-tag:v", "avc3"]),
            out("mp4"),
        ])],
        Expect::video("mp4", "h264", W, H, 24),
        &["libx264"],
    ));
    // HEVC.
    for (stem, pix, name, depth) in [
        ("main", "yuv420p", "Main", 8),
        ("main10", "yuv420p10le", "Main 10", 10),
        ("main12", "yuv420p12le", "Format Range Extensions", 12),
        ("main422_10", "yuv422p10le", "Format Range Extensions", 10),
        ("main444", "yuv444p", "Format Range Extensions", 8),
        ("main444_10", "yuv444p10le", "Format Range Extensions", 10),
        ("gray", "gray", "Format Range Extensions", 8),
    ] {
        v.push(F::new(
            format!("codec/hevc_{stem}.mkv"),
            vec![ffmpeg(vec![
                picture(),
                s(&[
                    "-c:v",
                    "libx265",
                    "-preset",
                    "ultrafast",
                    "-pix_fmt",
                    pix,
                    "-x265-params",
                    "log-level=error",
                ]),
                out("matroska"),
            ])],
            profile(
                Expect::video("matroska", "hevc", W, H, 24),
                name,
                pix,
                depth,
            ),
            &["libx265"],
        ));
    }
    v.push(F::new(
        "codec/hevc_hev1.mp4",
        vec![ffmpeg(vec![
            picture(),
            s(&[
                "-c:v",
                "libx265",
                "-preset",
                "ultrafast",
                "-x265-params",
                "log-level=error",
                "-tag:v",
                "hev1",
            ]),
            out("mp4"),
        ])],
        Expect::video("mp4", "hevc", W, H, 24),
        &["libx265"],
    ));
    // AV1 Main 8-bit and 10-bit.
    for (stem, pix, depth) in [("main8", "yuv420p", 8), ("main10", "yuv420p10le", 10)] {
        v.push(F::new(
            format!("codec/av1_{stem}.mkv"),
            vec![ffmpeg(vec![
                picture(),
                s(&["-c:v", "libsvtav1", "-preset", "12", "-pix_fmt", pix]),
                out("matroska"),
            ])],
            profile(
                Expect::video("matroska", "av1", W, H, 24),
                "Main",
                pix,
                depth,
            ),
            &["libsvtav1"],
        ));
    }
    v.push(F::new(
        "codec/av1.ivf.webm",
        vec![
            ffmpeg(vec![
                picture(),
                s(&["-c:v", "libsvtav1", "-preset", "12"]),
                to("ivf", "{work}/v.ivf"),
            ]),
            Step::Ffmpeg(s(&[
                "-i",
                "{work}/v.ivf",
                "-c",
                "copy",
                "-f",
                "webm",
                "{out}",
            ])),
        ],
        Expect::video("webm", "av1", W, H, 24),
        &["libsvtav1"],
    ));
    // VP9 profiles 0-3 and VP8.
    for (stem, pix, name, depth) in [
        ("profile0", "yuv420p", "Profile 0", 8),
        ("profile1", "yuv444p", "Profile 1", 8),
        ("profile2", "yuv420p10le", "Profile 2", 10),
        ("profile3", "yuv444p10le", "Profile 3", 10),
    ] {
        v.push(F::new(
            format!("codec/vp9_{stem}.webm"),
            vec![ffmpeg(vec![picture(), vp9(pix, None), out("webm")])],
            profile(Expect::video("webm", "vp9", W, H, 24), name, pix, depth),
            &["libvpx-vp9"],
        ));
    }
    for muxer in ["webm", "matroska", "mp4"] {
        v.push(F::new(
            format!("codec/vp9.{}", ext(muxer)),
            vec![ffmpeg(vec![picture(), vp9("yuv420p", None), out(muxer)])],
            Expect::video(container(muxer), "vp9", W, H, 24),
            &["libvpx-vp9"],
        ));
    }
    for muxer in ["webm", "matroska", "avi"] {
        v.push(F::new(
            format!("codec/vp8.{}", ext(muxer)),
            vec![ffmpeg(vec![
                picture(),
                s(&["-c:v", "libvpx", "-deadline", "realtime", "-b:v", "200k"]),
                out(muxer),
            ])],
            Expect::video(container(muxer), "vp8", W, H, 24),
            &["libvpx"],
        ));
    }
    // MPEG-4 Part 2 and its Microsoft and DivX/Xvid variants.
    for (stem, muxer, extra, codec, encoder) in [
        (
            "mpeg4_simple",
            "avi",
            &["-c:v", "mpeg4", "-q:v", "8"][..],
            "mpeg4",
            "mpeg4",
        ),
        (
            "mpeg4_asp_bframes_qpel",
            "avi",
            &[
                "-c:v", "mpeg4", "-q:v", "8", "-bf", "2", "-flags", "+qpel", "-vtag", "XVID",
            ][..],
            "mpeg4",
            "mpeg4",
        ),
        (
            "mpeg4_divx_tag",
            "avi",
            &["-c:v", "mpeg4", "-q:v", "8", "-vtag", "DX50"][..],
            "mpeg4",
            "mpeg4",
        ),
        (
            "mpeg4_packed_bitstream",
            "avi",
            &[
                "-c:v",
                "mpeg4",
                "-q:v",
                "8",
                "-bf",
                "1",
                "-vtag",
                "DIVX",
                "-bsf:v",
                "mpeg4_unpack_bframes",
            ][..],
            "mpeg4",
            "mpeg4",
        ),
        (
            "mpeg4_in_mp4",
            "mp4",
            &["-c:v", "mpeg4", "-q:v", "8"][..],
            "mpeg4",
            "mpeg4",
        ),
        (
            "mpeg4_in_mkv",
            "matroska",
            &["-c:v", "mpeg4", "-q:v", "8"][..],
            "mpeg4",
            "mpeg4",
        ),
        (
            "mpeg4_in_3gp",
            "3gp",
            &["-c:v", "mpeg4", "-q:v", "8"][..],
            "mpeg4",
            "mpeg4",
        ),
        (
            "msmpeg4v2",
            "avi",
            &["-c:v", "msmpeg4v2", "-q:v", "8"][..],
            "msmpeg4v2",
            "msmpeg4v2",
        ),
        (
            "msmpeg4v3",
            "avi",
            &["-c:v", "msmpeg4", "-q:v", "8"][..],
            "msmpeg4v3",
            "msmpeg4",
        ),
        (
            "wmv1",
            "asf",
            &["-c:v", "wmv1", "-q:v", "8"][..],
            "wmv1",
            "wmv1",
        ),
        (
            "wmv2",
            "asf",
            &["-c:v", "wmv2", "-q:v", "8"][..],
            "wmv2",
            "wmv2",
        ),
        (
            "msmpeg4v3_in_asf",
            "asf",
            &["-c:v", "msmpeg4", "-q:v", "8"][..],
            "msmpeg4v3",
            "msmpeg4",
        ),
        (
            "h264_in_asf",
            "asf",
            &[
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ][..],
            "h264",
            "libx264",
        ),
        (
            "h264_in_avi",
            "avi",
            &[
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ][..],
            "h264",
            "libx264",
        ),
        (
            "hevc_in_avi",
            "avi",
            &[
                "-c:v",
                "libx265",
                "-preset",
                "ultrafast",
                "-x265-params",
                "log-level=error",
            ][..],
            "hevc",
            "libx265",
        ),
        (
            "flv1",
            "flv",
            &["-c:v", "flv", "-q:v", "8"][..],
            "flv1",
            "flv",
        ),
        (
            "h263_3gp",
            "3gp",
            &["-c:v", "h263", "-q:v", "8"][..],
            "h263",
            "h263",
        ),
        (
            "h263_mov",
            "mov",
            &["-c:v", "h263", "-q:v", "8"][..],
            "h263",
            "h263",
        ),
        (
            "mjpeg_avi",
            "avi",
            &["-c:v", "mjpeg", "-q:v", "8", "-pix_fmt", "yuvj420p"][..],
            "mjpeg",
            "mjpeg",
        ),
        (
            "mjpeg_mkv",
            "matroska",
            &["-c:v", "mjpeg", "-q:v", "8", "-pix_fmt", "yuvj420p"][..],
            "mjpeg",
            "mjpeg",
        ),
        (
            "mjpeg_mov",
            "mov",
            &["-c:v", "mjpeg", "-q:v", "8", "-pix_fmt", "yuvj420p"][..],
            "mjpeg",
            "mjpeg",
        ),
        ("png_mov", "mov", &["-c:v", "png"][..], "png", "png"),
        ("png_mkv", "matroska", &["-c:v", "png"][..], "png", "png"),
        (
            "prores_proxy",
            "mov",
            &["-c:v", "prores_ks", "-profile:v", "0"][..],
            "prores",
            "prores_ks",
        ),
        (
            "prores_422",
            "mov",
            &["-c:v", "prores_ks", "-profile:v", "2"][..],
            "prores",
            "prores_ks",
        ),
        (
            "prores_4444",
            "mov",
            &[
                "-c:v",
                "prores_ks",
                "-profile:v",
                "4",
                "-pix_fmt",
                "yuva444p10le",
            ][..],
            "prores",
            "prores_ks",
        ),
        (
            "prores_mkv",
            "matroska",
            &["-c:v", "prores_ks", "-profile:v", "0"][..],
            "prores",
            "prores_ks",
        ),
        (
            "ffv1_mkv",
            "matroska",
            &["-c:v", "ffv1", "-level", "3"][..],
            "ffv1",
            "ffv1",
        ),
        ("ffv1_avi", "avi", &["-c:v", "ffv1"][..], "ffv1", "ffv1"),
        (
            "huffyuv_avi",
            "avi",
            &["-c:v", "huffyuv", "-pix_fmt", "yuv422p"][..],
            "huffyuv",
            "huffyuv",
        ),
        (
            "utvideo_avi",
            "avi",
            &["-c:v", "utvideo"][..],
            "utvideo",
            "utvideo",
        ),
        (
            "rawvideo_avi",
            "avi",
            &["-c:v", "rawvideo", "-pix_fmt", "yuv420p"][..],
            "rawvideo",
            "rawvideo",
        ),
        ("v210_mov", "mov", &["-c:v", "v210"][..], "v210", "v210"),
        ("qtrle_mov", "mov", &["-c:v", "qtrle"][..], "qtrle", "qtrle"),
        ("svq1_mov", "mov", &["-c:v", "svq1"][..], "svq1", "svq1"),
        (
            "cinepak_avi",
            "avi",
            &["-c:v", "cinepak"][..],
            "cinepak",
            "cinepak",
        ),
        (
            "cinepak_mov",
            "mov",
            &["-c:v", "cinepak"][..],
            "cinepak",
            "cinepak",
        ),
        (
            "msvideo1_avi",
            "avi",
            &["-c:v", "msvideo1"][..],
            "msvideo1",
            "msvideo1",
        ),
        (
            "jpeg2000_mov",
            "mov",
            &["-c:v", "jpeg2000"][..],
            "jpeg2000",
            "jpeg2000",
        ),
        (
            "jpeg2000_mkv",
            "matroska",
            &["-c:v", "jpeg2000"][..],
            "jpeg2000",
            "jpeg2000",
        ),
        (
            "dnxhr_lb_mov",
            "mov",
            &[
                "-c:v",
                "dnxhd",
                "-profile:v",
                "dnxhr_lb",
                "-pix_fmt",
                "yuv422p",
            ][..],
            "dnxhd",
            "dnxhd",
        ),
        (
            "dirac_vc2_mkv",
            "matroska",
            &["-c:v", "vc2", "-strict", "-2"][..],
            "dirac",
            "vc2",
        ),
        (
            "theora_ogg",
            "ogg",
            &["-c:v", "libtheora", "-q:v", "5"][..],
            "theora",
            "libtheora",
        ),
        (
            "theora_mkv",
            "matroska",
            &["-c:v", "libtheora", "-q:v", "5"][..],
            "theora",
            "libtheora",
        ),
    ] {
        let (size, w, h) = if stem.starts_with("h263") {
            ("128x96", 128, 96)
        } else if stem.starts_with("dnxhr") {
            ("256x144", 256, 144)
        } else {
            (SIZE, W, H)
        };
        v.push(F::new(
            format!("codec/{stem}.{}", ext(muxer)),
            vec![ffmpeg(vec![pattern(size, 24, "1"), s(extra), out(muxer)])],
            Expect::video(container(muxer), codec, w, h, 24),
            &[encoder],
        ));
    }
    // RealMedia.
    for (stem, video, audio_args, acodec, channels, aenc, extension) in [
        (
            "rv10_ac3",
            "rv10",
            &["-c:a", "ac3", "-b:a", "96k"][..],
            "ac3",
            2,
            "ac3",
            "rm",
        ),
        (
            "rv20_real144",
            "rv20",
            &["-c:a", "real_144", "-ar", "8000", "-ac", "1"][..],
            "ra_144",
            1,
            "real_144",
            "rm",
        ),
        (
            "rv20_ac3",
            "rv20",
            &["-c:a", "ac3", "-b:a", "96k"][..],
            "ac3",
            2,
            "ac3",
            "rmvb",
        ),
    ] {
        v.push(F::new(
            format!("rm/{stem}.{extension}"),
            vec![ffmpeg(vec![
                pattern("128x96", 25, "1"),
                silence("stereo", 44100),
                s(&["-c:v", video, "-q:v", "8"]),
                s(audio_args),
                out("rm"),
            ])],
            Expect::video("rm", video, 128, 96, 25).audio(acodec, channels, None),
            &[video, aenc],
        ));
    }
    // DV: one or two frames each, since every DV frame is 120-576 KB.
    for (stem, size, rate, pix, frames, extra) in [
        ("ntsc_411", "720x480", "30000/1001", "yuv411p", "2", &[][..]),
        ("pal_420", "720x576", "25", "yuv420p", "2", &[][..]),
        (
            "dvcpro50_ntsc_422",
            "720x480",
            "30000/1001",
            "yuv422p",
            "2",
            &[][..],
        ),
        (
            "dvcprohd_1080i",
            "1280x1080",
            "30000/1001",
            "yuv422p",
            "1",
            &[][..],
        ),
        (
            "dvcprohd_1080i_50",
            "1440x1080",
            "25",
            "yuv422p",
            "1",
            &[][..],
        ),
        (
            "dvcprohd_720p",
            "960x720",
            "60000/1001",
            "yuv422p",
            "1",
            &[][..],
        ),
    ] {
        let (w, h) = size
            .split_once('x')
            .map(|(w, h)| (w.parse().unwrap_or(0), h.parse().unwrap_or(0)))
            .unwrap_or((0, 0));
        let fps = if rate.starts_with("30000") {
            30
        } else if rate.starts_with("60000") {
            60
        } else {
            25
        };
        let mut e =
            Expect::video("dv", "dvvideo", w, h, fps).check("video.metadata.pixel_format", pix);
        e.min_duration_seconds = 0;
        let input = vec![
            "-f".into(),
            "lavfi".into(),
            "-i".into(),
            format!("testsrc2=size={size}:rate={rate}"),
        ];
        v.push(F::new(
            format!("dv/{stem}.dv"),
            vec![ffmpeg(vec![
                input,
                s(&["-frames:v", frames, "-c:v", "dvvideo", "-pix_fmt", pix]),
                s(extra),
                out("dv"),
            ])],
            e,
            &["dvvideo"],
        ));
    }
    for (stem, muxer) in [
        ("dv_in_avi", "avi"),
        ("dv_in_mov", "mov"),
        ("dv_in_mkv", "matroska"),
    ] {
        let mut e = Expect::video(container(muxer), "dvvideo", 720, 576, 25);
        e.min_duration_seconds = 0;
        v.push(F::new(
            format!("dv/{stem}.{}", ext(muxer)),
            vec![ffmpeg(vec![
                s(&[
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=720x576:rate=25",
                    "-frames:v",
                    "2",
                    "-c:v",
                    "dvvideo",
                    "-pix_fmt",
                    "yuv420p",
                ]),
                out(muxer),
            ])],
            e,
            &["dvvideo"],
        ));
    }
    // MPEG-1/2 video in the containers that carry it.
    for (stem, muxer, extra, codec, encoder) in [
        (
            "mpeg2_main.mkv",
            "matroska",
            &["-c:v", "mpeg2video", "-b:v", "500k"][..],
            "mpeg2video",
            "mpeg2video",
        ),
        (
            "mpeg2_main.ts",
            "mpegts",
            &["-c:v", "mpeg2video", "-b:v", "500k"][..],
            "mpeg2video",
            "mpeg2video",
        ),
        (
            "mpeg2_main.mp4",
            "mp4",
            &["-c:v", "mpeg2video", "-b:v", "500k"][..],
            "mpeg2video",
            "mpeg2video",
        ),
        (
            "mpeg2_main.mov",
            "mov",
            &["-c:v", "mpeg2video", "-b:v", "500k"][..],
            "mpeg2video",
            "mpeg2video",
        ),
        (
            "mpeg2_main.avi",
            "avi",
            &["-c:v", "mpeg2video", "-b:v", "500k"][..],
            "mpeg2video",
            "mpeg2video",
        ),
        (
            "mpeg2_422.mkv",
            "matroska",
            &["-c:v", "mpeg2video", "-b:v", "500k", "-pix_fmt", "yuv422p"][..],
            "mpeg2video",
            "mpeg2video",
        ),
        (
            "mpeg2_main.mpg",
            "mpeg",
            &["-c:v", "mpeg2video", "-b:v", "500k"][..],
            "mpeg2video",
            "mpeg2video",
        ),
        (
            "mpeg1.mkv",
            "matroska",
            &["-c:v", "mpeg1video", "-b:v", "500k"][..],
            "mpeg1video",
            "mpeg1video",
        ),
        (
            "mpeg1.ts",
            "mpegts",
            &["-c:v", "mpeg1video", "-b:v", "500k"][..],
            "mpeg1video",
            "mpeg1video",
        ),
    ] {
        let muxer_ext = ext(muxer);
        let name = format!("codec/{stem}");
        debug_assert!(name.ends_with(muxer_ext));
        v.push(F::new(
            name,
            vec![ffmpeg(vec![
                pattern("128x96", 25, "1"),
                s(extra),
                out(muxer),
            ])],
            Expect::video(container(muxer), codec, 128, 96, 25),
            &[encoder],
        ));
    }
}

// ---------------------------------------------------------------------------
// Elementary streams and program streams
// ---------------------------------------------------------------------------

fn elementary_fixtures(v: &mut Vec<F>) {
    for (name, muxer, codec_args, codec, encoder) in [
        (
            "elementary/h264.h264",
            "h264",
            &[
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ][..],
            "h264",
            "libx264",
        ),
        (
            "elementary/h264.avc",
            "h264",
            &[
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ][..],
            "h264",
            "libx264",
        ),
        (
            "elementary/h264_high10.h264",
            "h264",
            &[
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p10le",
                "-profile:v",
                "high10",
            ][..],
            "h264",
            "libx264",
        ),
        (
            "elementary/mpeg2.m2v",
            "mpeg2video",
            &["-c:v", "mpeg2video", "-b:v", "500k"][..],
            "mpeg2video",
            "mpeg2video",
        ),
        (
            "elementary/mpeg1.m2v",
            "mpeg1video",
            &["-c:v", "mpeg1video", "-b:v", "500k"][..],
            "mpeg1video",
            "mpeg1video",
        ),
    ] {
        let mut e = Expect::video("unknown", codec, 128, 96, 25);
        e.container = if muxer == "h264" { "h264" } else { "mpegvideo" };
        e.min_duration_seconds = 0;
        v.push(F::new(
            name,
            vec![ffmpeg(vec![
                pattern("128x96", 25, "1"),
                s(codec_args),
                out(muxer),
            ])],
            e,
            &[encoder],
        ));
    }
    // Raw HEVC: SDR Main, and HDR10 whose static metadata travels only in SEI,
    // with no container to repeat it.
    for (name, args, hdr) in [
        (
            "elementary/hevc.h265",
            x265("yuv420p", Transfer::Sdr, false, false, None),
            false,
        ),
        (
            "elementary/hevc_hdr10.hevc",
            x265("yuv420p10le", Transfer::Pq, true, false, None),
            true,
        ),
    ] {
        let mut e = Expect::video("unknown", "hevc", 128, 96, 25);
        e.container = "hevc";
        e.min_duration_seconds = 0;
        let e = if hdr { hdr10(e) } else { e };
        v.push(F::new(
            name,
            vec![ffmpeg(vec![pattern("128x96", 25, "1"), args, out("hevc")])],
            e,
            &["libx265"],
        ));
    }
    // Program streams: MPEG-2 with AC-3, LPCM and MP2 audio; DVD and SVCD muxers.
    for (name, muxer, audio_args, acodec, channels, aenc) in [
        (
            "ps/mpeg2_ac3.vob",
            "vob",
            &["-c:a", "ac3", "-b:a", "192k"][..],
            "ac3",
            6,
            "ac3",
        ),
        (
            "ps/mpeg2_lpcm.vob",
            "vob",
            &["-c:a", "pcm_dvd"][..],
            "pcm_dvd",
            2,
            "pcm_dvd",
        ),
        (
            "ps/mpeg2_dvd_mux.vob",
            "dvd",
            &["-c:a", "ac3", "-b:a", "192k"][..],
            "ac3",
            6,
            "ac3",
        ),
        (
            "ps/mpeg2_mp2.mpg",
            "mpeg",
            &["-c:a", "mp2", "-b:a", "128k"][..],
            "mp2",
            2,
            "mp2",
        ),
        (
            "ps/mpeg1_mp2.mpeg",
            "mpeg",
            &["-c:a", "mp2", "-b:a", "128k"][..],
            "mp2",
            2,
            "mp2",
        ),
        (
            "ps/svcd.mpg",
            "svcd",
            &["-c:a", "mp2", "-b:a", "128k"][..],
            "mp2",
            2,
            "mp2",
        ),
        (
            "ps/mpeg2_dts.vob",
            "vob",
            &["-c:a", "dca", "-b:a", "768k", "-strict", "-2"][..],
            "dts",
            6,
            "dca",
        ),
    ] {
        let layout = if channels == 6 { "5.1" } else { "stereo" };
        let video: &str = if name.contains("mpeg1") {
            "mpeg1video"
        } else {
            "mpeg2video"
        };
        v.push(F::new(
            name,
            vec![ffmpeg(vec![
                pattern("128x96", 25, "1"),
                silence(layout, 48000),
                s(&["-c:v", video, "-b:v", "500k"]),
                s(audio_args),
                s(&["-shortest"]),
                out(muxer),
            ])],
            Expect::video("mpeg", video, 128, 96, 25).audio(acodec, channels, None),
            &[video, aenc],
        ));
    }
}

// ---------------------------------------------------------------------------
// Audio: codecs per container, layouts, profiles, sample formats
// ---------------------------------------------------------------------------

fn audio_fixtures(v: &mut Vec<F>) {
    // (muxer, encoder, reported codec, layout, channels, sample rate)
    let cases: &[(&str, &str, &str, &str, i32, u32)] = &[
        ("matroska", "aac", "aac", "stereo", 2, 48000),
        ("matroska", "libmp3lame", "mp3", "stereo", 2, 44100),
        ("matroska", "mp2", "mp2", "stereo", 2, 48000),
        ("matroska", "ac3", "ac3", "5.1", 6, 48000),
        ("matroska", "eac3", "eac3", "5.1", 6, 48000),
        ("matroska", "truehd", "truehd", "5.1", 6, 48000),
        ("matroska", "mlp", "mlp", "stereo", 2, 48000),
        ("matroska", "dca", "dts", "5.1", 6, 48000),
        ("matroska", "flac", "flac", "7.1", 8, 48000),
        ("matroska", "libopus", "opus", "5.1", 6, 48000),
        ("matroska", "vorbis", "vorbis", "stereo", 2, 44100),
        ("matroska", "alac", "alac", "stereo", 2, 44100),
        ("matroska", "pcm_s16le", "pcm_s16le", "stereo", 2, 48000),
        ("matroska", "pcm_s24le", "pcm_s24le", "5.1", 6, 48000),
        ("matroska", "pcm_s32le", "pcm_s32le", "stereo", 2, 48000),
        ("matroska", "pcm_f32le", "pcm_f32le", "stereo", 2, 48000),
        ("matroska", "pcm_f64le", "pcm_f64le", "mono", 1, 48000),
        ("matroska", "pcm_s16be", "pcm_s16be", "stereo", 2, 48000),
        ("matroska", "wavpack", "wavpack", "stereo", 2, 44100),
        ("matroska", "tta", "tta", "stereo", 2, 44100),
        ("mp4", "aac", "aac", "7.1", 8, 48000),
        ("mp4", "libmp3lame", "mp3", "stereo", 2, 44100),
        ("mp4", "mp2", "mp2", "stereo", 2, 48000),
        ("mp4", "ac3", "ac3", "5.1", 6, 48000),
        ("mp4", "eac3", "eac3", "5.1", 6, 48000),
        ("mp4", "alac", "alac", "stereo", 2, 44100),
        ("mp4", "flac", "flac", "5.1", 6, 48000),
        ("mp4", "libopus", "opus", "stereo", 2, 48000),
        ("mp4", "dca", "dts", "5.1", 6, 48000),
        ("mp4", "truehd", "truehd", "5.1", 6, 48000),
        ("mov", "pcm_s16be", "pcm_s16be", "stereo", 2, 48000),
        ("mov", "pcm_s16le", "pcm_s16le", "stereo", 2, 48000),
        ("mov", "pcm_s24be", "pcm_s24be", "stereo", 2, 48000),
        ("mov", "pcm_s32be", "pcm_s32be", "stereo", 2, 48000),
        ("mov", "pcm_f32be", "pcm_f32be", "stereo", 2, 48000),
        ("mov", "pcm_f64be", "pcm_f64be", "mono", 1, 48000),
        ("mov", "pcm_s8", "pcm_s8", "mono", 1, 22050),
        ("mov", "pcm_alaw", "pcm_alaw", "mono", 1, 8000),
        ("mov", "pcm_mulaw", "pcm_mulaw", "mono", 1, 8000),
        ("mov", "adpcm_ima_qt", "adpcm_ima_qt", "stereo", 2, 44100),
        ("mov", "alac", "alac", "stereo", 2, 44100),
        ("mov", "aac", "aac", "mono", 1, 48000),
        ("avi", "pcm_s16le", "pcm_s16le", "stereo", 2, 48000),
        ("avi", "pcm_u8", "pcm_u8", "mono", 1, 22050),
        ("avi", "pcm_s24le", "pcm_s24le", "5.1", 6, 48000),
        ("avi", "adpcm_ms", "adpcm_ms", "stereo", 2, 44100),
        ("avi", "pcm_alaw", "pcm_alaw", "mono", 1, 8000),
        ("avi", "ac3", "ac3", "5.1", 6, 48000),
        ("avi", "dca", "dts", "5.1", 6, 48000),
        ("avi", "mp2", "mp2", "stereo", 2, 48000),
        ("avi", "libmp3lame", "mp3", "mono", 1, 44100),
        ("avi", "aac", "aac", "stereo", 2, 48000),
        ("avi", "wmav2", "wmav2", "stereo", 2, 44100),
        ("mpegts", "aac", "aac", "5.1", 6, 48000),
        ("mpegts", "mp2", "mp2", "stereo", 2, 48000),
        ("mpegts", "libmp3lame", "mp3", "stereo", 2, 44100),
        ("mpegts", "ac3", "ac3", "5.1", 6, 48000),
        ("mpegts", "eac3", "eac3", "5.1", 6, 48000),
        ("mpegts", "dca", "dts", "5.1", 6, 48000),
        ("mpegts", "truehd", "truehd", "5.1", 6, 48000),
        ("mpegts", "libopus", "opus", "stereo", 2, 48000),
        ("mpegts", "s302m", "s302m", "stereo", 2, 48000),
        ("asf", "wmav1", "wmav1", "stereo", 2, 44100),
        ("asf", "wmav2", "wmav2", "stereo", 2, 44100),
        ("asf", "libmp3lame", "mp3", "stereo", 2, 44100),
        ("asf", "pcm_s16le", "pcm_s16le", "stereo", 2, 48000),
        ("asf", "ac3", "ac3", "5.1", 6, 48000),
        ("flv", "aac", "aac", "stereo", 2, 44100),
        ("flv", "libmp3lame", "mp3", "stereo", 2, 44100),
        ("flv", "pcm_s16le", "pcm_s16le", "stereo", 2, 44100),
        ("flv", "adpcm_swf", "adpcm_swf", "mono", 1, 22050),
        ("flv", "nellymoser", "nellymoser", "mono", 1, 22050),
        ("webm", "libopus", "opus", "stereo", 2, 48000),
        ("webm", "vorbis", "vorbis", "stereo", 2, 48000),
    ];
    for (muxer, encoder, codec, layout, channels, rate) in cases {
        let (video, vcodec, venc) = match *muxer {
            "webm" => (vp9("yuv420p", None), "vp9", "libvpx-vp9"),
            "asf" => (s(&["-c:v", "wmv2", "-q:v", "8"]), "wmv2", "wmv2"),
            "flv" => (s(&["-c:v", "flv", "-q:v", "8"]), "flv1", "flv"),
            _ => (x264_plain(), "h264", "libx264"),
        };
        let mut ac = audio(encoder);
        if *encoder == "libmp3lame" && *channels == 1 {
            ac.extend(s(&["-ac", "1"]));
        }
        let name = format!("audio/{codec}_{}ch.{}", channels, ext(muxer));
        let mut e = Expect::video(container(muxer), vcodec, W, H, 24).audio(codec, *channels, None);
        e = e.check("audio.0.metadata.sample_rate", i64::from(*rate));
        if codec.starts_with("pcm_s24") {
            e = e.check("audio.0.metadata.sample_bit_depth", 24i64);
        }
        if *codec == "aac" {
            e = e.check("summary.audio_profile", "LC");
        }
        if *codec == "dts" {
            e = e.check("summary.audio_profile", "DTS");
        }
        let mut steps = vec![ffmpeg(vec![
            picture(),
            silence(layout, *rate),
            video,
            ac,
            s(&["-shortest"]),
            out(muxer),
        ])];
        if *muxer == "mov" && codec.starts_with("pcm") {
            // nothing extra: the mov muxer picks the fourcc from the codec.
            steps.shrink_to_fit();
        }
        v.push(F::new(name, steps, e, &[venc, encoder]));
    }
    // Layouts: FLAC carries every WAVE channel mask up to eight channels.
    for (layout, channels) in [
        ("mono", 1),
        ("stereo", 2),
        ("2.1", 3),
        ("quad", 4),
        ("5.0", 5),
        ("5.1", 6),
        ("6.1", 7),
        ("7.1", 8),
    ] {
        v.push(F::new(
            format!("audio/layout_flac_{}.mkv", layout.replace('.', "_")),
            vec![ffmpeg(vec![
                picture(),
                silence(layout, 48000),
                x264_plain(),
                audio("flac"),
                s(&["-shortest"]),
                out("matroska"),
            ])],
            Expect::video("matroska", "h264", W, H, 24).audio("flac", channels, None),
            &["libx264", "flac"],
        ));
    }
    // M2TS LPCM (Blu-ray) and AAC LATM.
    v.push(F::new(
        "audio/pcm_bluray.m2ts",
        vec![ffmpeg(vec![
            picture(),
            silence("5.1", 48000),
            x264_plain(),
            s(&["-c:a", "pcm_bluray", "-mpegts_m2ts_mode", "1", "-shortest"]),
            out("mpegts"),
        ])],
        Expect::video("mpegts", "h264", W, H, 24).audio("pcm_bluray", 6, None),
        &["libx264", "pcm_bluray"],
    ));
    v.push(F::new(
        "audio/aac_latm.ts",
        vec![ffmpeg(vec![
            picture(),
            silence("stereo", 48000),
            x264_plain(),
            s(&[
                "-c:a",
                "aac",
                "-b:a",
                "96k",
                "-mpegts_flags",
                "latm",
                "-shortest",
            ]),
            out("mpegts"),
        ])],
        Expect::video("mpegts", "h264", W, H, 24).audio("aac_latm", 2, None),
        &["libx264", "aac"],
    ));
    // Ogg audio streams. Ogg video is Theora only (FFmpeg's Ogg muxer takes no
    // Dirac), so these are audio-only Ogg files.
    for (encoder, codec, layout, channels) in [
        ("vorbis", "vorbis", "stereo", 2),
        ("libopus", "opus", "5.1", 6),
        ("flac", "flac", "stereo", 2),
    ] {
        v.push(F::new(
            format!("audio/{codec}_{channels}ch.ogg"),
            vec![ffmpeg(vec![
                silence(layout, 48000),
                audio(encoder),
                out("ogg"),
            ])],
            Expect::audio_only("ogg").audio(codec, channels, None),
            &[encoder],
        ));
    }
}

// ---------------------------------------------------------------------------
// Subtitles, dispositions, languages, multi-audio
// ---------------------------------------------------------------------------

const SRT: &str = "1\n00:00:00,000 --> 00:00:00,500\nfixture one\n\n2\n00:00:00,500 --> 00:00:01,000\nfixture two\n";

fn subtitle_fixtures(v: &mut Vec<F>) {
    let srt = || write("s.srt", SRT);
    for (muxer, encoder, codec) in [
        ("matroska", "srt", "subrip"),
        ("matroska", "ass", "ass"),
        ("matroska", "ssa", "ass"),
        ("matroska", "webvtt", "webvtt"),
        ("matroska", "text", "subrip"),
        ("webm", "webvtt", "webvtt"),
        ("mp4", "mov_text", "mov_text"),
        ("mov", "mov_text", "mov_text"),
        ("mp4", "ttml", "ttml"),
        ("3gp", "mov_text", "mov_text"),
    ] {
        let (video, vcodec, venc) = if muxer == "webm" {
            (vp9("yuv420p", None), "vp9", "libvpx-vp9")
        } else {
            (x264_plain(), "h264", "libx264")
        };
        let mut e = Expect::video(container(muxer), vcodec, W, H, 24)
            .check("summary.subtitle_codecs", &[codec][..])
            .check("subtitle.0.codec", codec);
        e.subtitle_stream_count = 1;
        let mut extra = s(&[
            "-map",
            "0:v",
            "-map",
            "1:s",
            "-c:s",
            encoder,
            "-metadata:s:s:0",
            "language=eng",
        ]);
        if encoder == "ttml" {
            extra.extend(s(&["-strict", "unofficial"]));
        }
        v.push(F::new(
            format!("subs/{encoder}.{}", ext(muxer)),
            vec![
                srt(),
                ffmpeg(vec![
                    picture(),
                    s(&["-i", "{work}/s.srt"]),
                    extra,
                    video,
                    out(muxer),
                ]),
            ],
            e.check("summary.subtitle_languages", &["eng"][..]),
            &[venc, encoder],
        ));
    }
    // Several subtitle tracks with distinct languages and dispositions.
    let mut e = Expect::video("matroska", "h264", W, H, 24)
        .check("summary.subtitle_languages", &["eng", "fra", "deu"][..])
        .check("subtitle.0.metadata.disposition.default", true)
        .check("subtitle.1.metadata.disposition.forced", true)
        .check("subtitle.2.metadata.disposition.hearing_impaired", true);
    e.subtitle_stream_count = 3;
    v.push(F::new(
        "subs/three_tracks_dispositions.mkv",
        vec![
            srt(),
            ffmpeg(vec![
                picture(),
                s(&[
                    "-i",
                    "{work}/s.srt",
                    "-i",
                    "{work}/s.srt",
                    "-i",
                    "{work}/s.srt",
                    "-map",
                    "0:v",
                    "-map",
                    "1:s",
                    "-map",
                    "2:s",
                    "-map",
                    "3:s",
                    "-c:s",
                    "srt",
                    "-metadata:s:s:0",
                    "language=eng",
                    "-metadata:s:s:1",
                    "language=fra",
                    "-metadata:s:s:2",
                    "language=deu",
                    "-disposition:s:0",
                    "default",
                    "-disposition:s:1",
                    "forced",
                    "-disposition:s:2",
                    "hearing_impaired",
                ]),
                x264_plain(),
                out("matroska"),
            ]),
        ],
        e,
        &["libx264", "srt"],
    ));
    let mut e = Expect::video("mp4", "h264", W, H, 24)
        .check("subtitle.0.metadata.disposition.forced", true)
        .check("subtitle.1.metadata.disposition.hearing_impaired", true);
    e.subtitle_stream_count = 2;
    v.push(F::new(
        "subs/mov_text_forced_sdh.mp4",
        vec![
            srt(),
            ffmpeg(vec![
                picture(),
                s(&[
                    "-i",
                    "{work}/s.srt",
                    "-i",
                    "{work}/s.srt",
                    "-map",
                    "0:v",
                    "-map",
                    "1:s",
                    "-map",
                    "2:s",
                    "-c:s",
                    "mov_text",
                    "-metadata:s:s:0",
                    "language=eng",
                    "-metadata:s:s:1",
                    "language=eng",
                    "-disposition:s:0",
                    "forced",
                    "-disposition:s:1",
                    "hearing_impaired",
                ]),
                x264_plain(),
                out("mp4"),
            ]),
        ],
        e,
        &["libx264", "mov_text"],
    ));
}

fn disposition_fixtures(v: &mut Vec<F>) {
    // Matroska: FlagDefault, FlagForced, FlagHearingImpaired, FlagVisualImpaired,
    // FlagOriginal and FlagCommentary, one per audio track.
    let flags = [
        "default",
        "forced",
        "hearing_impaired",
        "visual_impaired",
        "original",
        "comment",
    ];
    let fields = [
        "default",
        "forced",
        "hearing_impaired",
        "visual_impaired",
        "original",
        "commentary",
    ];
    let mut a = vec![picture()];
    for _ in &flags {
        a.push(silence("stereo", 48000));
    }
    let mut maps = s(&["-map", "0:v"]);
    for (i, flag) in flags.iter().enumerate() {
        maps.extend(s(&["-map", &format!("{}:a", i + 1)]));
        maps.extend([format!("-disposition:a:{i}"), (*flag).to_owned()]);
        maps.extend([format!("-metadata:s:a:{i}"), "language=eng".to_owned()]);
    }
    a.push(maps);
    a.push(x264_plain());
    a.push(s(&["-c:a", "aac", "-b:a", "32k", "-shortest"]));
    a.push(out("matroska"));
    let mut e = Expect::video("matroska", "h264", W, H, 24);
    for (i, field) in fields.iter().enumerate() {
        e = e
            .audio("aac", 2, Some("eng"))
            .check(&format!("audio.{i}.metadata.disposition.{field}"), true);
    }
    v.push(F::new(
        "disposition/audio_flags.mkv",
        vec![ffmpeg(a)],
        e,
        &["libx264", "aac"],
    ));
    // MP4: kind boxes (DASH role scheme) for commentary and hearing-impaired.
    v.push(F::new(
        "disposition/audio_kind.mp4",
        vec![ffmpeg(vec![
            picture(),
            silence("stereo", 48000),
            silence("stereo", 48000),
            s(&[
                "-map",
                "0:v",
                "-map",
                "1:a",
                "-map",
                "2:a",
                "-disposition:a:0",
                "default",
                "-disposition:a:1",
                "comment+hearing_impaired",
                "-metadata:s:a:0",
                "language=eng",
                "-metadata:s:a:1",
                "language=eng",
            ]),
            x264_plain(),
            s(&["-c:a", "aac", "-b:a", "32k", "-shortest"]),
            out("mp4"),
        ])],
        Expect::video("mp4", "h264", W, H, 24)
            .audio("aac", 2, Some("eng"))
            .audio("aac", 2, Some("eng"))
            .check("audio.0.metadata.disposition.default", true)
            .check("audio.1.metadata.disposition.commentary", true)
            .check("audio.1.metadata.disposition.hearing_impaired", true),
        &["libx264", "aac"],
    ));
    // Cover art: an attached picture in MP4 and MKV.
    v.push(F::new(
        "disposition/cover_art.mp4",
        vec![
            ffmpeg(vec![
                pattern("32x32", 1, "1"),
                s(&[
                    "-frames:v",
                    "1",
                    "-c:v",
                    "png",
                    "-f",
                    "image2",
                    "{work}/cover.png",
                ]),
            ]),
            // Per-stream options: an unqualified -profile:v or -pix_fmt would reach the PNG encoder too.
            ffmpeg(vec![
                picture(),
                s(&[
                    "-i",
                    "{work}/cover.png",
                    "-map",
                    "0:v",
                    "-map",
                    "1:v",
                    "-c:v:0",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-pix_fmt:v:0",
                    "yuv420p",
                    "-profile:v:0",
                    "high",
                    "-c:v:1",
                    "png",
                    "-disposition:v:1",
                    "attached_pic",
                ]),
                out("mp4"),
            ]),
        ],
        Expect::video("mp4", "h264", W, H, 24),
        &["libx264", "png"],
    ));
    v.push(F::new(
        "disposition/cover_art.mkv",
        vec![
            ffmpeg(vec![
                pattern("32x32", 1, "1"),
                s(&[
                    "-frames:v",
                    "1",
                    "-c:v",
                    "mjpeg",
                    "-pix_fmt",
                    "yuvj420p",
                    "-f",
                    "image2",
                    "{work}/cover.jpg",
                ]),
            ]),
            ffmpeg(vec![
                picture(),
                x264_plain(),
                s(&[
                    "-attach",
                    "{work}/cover.jpg",
                    "-metadata:s:t:0",
                    "mimetype=image/jpeg",
                    "-metadata:s:t:0",
                    "filename=cover.jpg",
                ]),
                out("matroska"),
            ]),
        ],
        Expect::video("matroska", "h264", W, H, 24).check("attachment.0.media_type", "image/jpeg"),
        &["libx264", "mjpeg"],
    ));
    // Video track flagged non-default beside a default one.
    v.push(F::new(
        "disposition/two_video_second_default.mkv",
        vec![ffmpeg(vec![
            picture(),
            picture(),
            s(&[
                "-map",
                "0:v",
                "-map",
                "1:v",
                "-disposition:v:0",
                "0",
                "-disposition:v:1",
                "default",
            ]),
            x264_plain(),
            out("matroska"),
        ])],
        Expect::video("matroska", "h264", W, H, 24),
        &["libx264"],
    ));
}

fn language_fixtures(v: &mut Vec<F>) {
    // (name, muxer, language metadata, expected language)
    for (stem, muxer, tag, expected) in [
        ("iso639_2_jpn", "matroska", Some("jpn"), Some("jpn")),
        ("iso639_2b_fre", "matroska", Some("fre"), Some("fra")),
        ("bcp47_pt_br", "matroska", Some("pt-BR"), Some("pt-BR")),
        (
            "bcp47_zh_hant",
            "matroska",
            Some("zh-Hant"),
            Some("zh-Hant"),
        ),
        ("und", "matroska", Some("und"), None),
        ("missing", "matroska", None, None),
        ("iso639_2_deu", "mp4", Some("deu"), Some("deu")),
        ("bcp47_pt_br", "mp4", Some("pt-BR"), Some("pt-BR")),
        ("und", "mp4", Some("und"), None),
        ("iso639_2_spa", "mpegts", Some("spa"), Some("spa")),
        ("iso639_2_swe", "asf", Some("swe"), Some("swe")),
        ("iso639_2_kor", "mov", Some("kor"), Some("kor")),
        ("iso639_2_nld", "webm", Some("nld"), Some("nld")),
    ] {
        let (video, vcodec, venc, acodec, aenc) = match muxer {
            "webm" => (vp9("yuv420p", None), "vp9", "libvpx-vp9", "opus", "libopus"),
            "asf" => (
                s(&["-c:v", "wmv2", "-q:v", "8"]),
                "wmv2",
                "wmv2",
                "wmav2",
                "wmav2",
            ),
            _ => (x264_plain(), "h264", "libx264", "aac", "aac"),
        };
        let mut a = vec![
            picture(),
            silence("stereo", 48000),
            video,
            audio(aenc),
            s(&["-shortest"]),
        ];
        if let Some(tag) = tag {
            a.push(vec!["-metadata:s:a:0".into(), format!("language={tag}")]);
        }
        a.push(out(muxer));
        let mut e = Expect::video(container(muxer), vcodec, W, H, 24).audio(acodec, 2, expected);
        if let Some(l) = expected {
            e = e.check("audio.0.language", l);
        }
        v.push(F::new(
            format!("lang/{stem}.{}", ext(muxer)),
            vec![ffmpeg(a)],
            e,
            &[venc, aenc],
        ));
    }
    // Ogg carries the language as a Vorbis comment; Ogg video is Theora only,
    // so this one is audio-only.
    v.push(F::new(
        "lang/iso639_2_ita.ogg",
        vec![ffmpeg(vec![
            silence("stereo", 48000),
            audio("vorbis"),
            s(&["-metadata:s:a:0", "language=ita"]),
            out("ogg"),
        ])],
        Expect::audio_only("ogg")
            .audio("vorbis", 2, Some("ita"))
            .check("audio.0.language", "ita"),
        &["vorbis"],
    ));
}

fn multiaudio_fixtures(v: &mut Vec<F>) {
    for (muxer, codecs) in [
        (
            "matroska",
            &[
                ("ac3", "ac3", "5.1", 6, "eng"),
                ("aac", "aac", "stereo", 2, "jpn"),
                ("flac", "flac", "stereo", 2, "fra"),
                ("libopus", "opus", "stereo", 2, "spa"),
            ][..],
        ),
        (
            "mp4",
            &[
                ("aac", "aac", "stereo", 2, "eng"),
                ("ac3", "ac3", "5.1", 6, "deu"),
                ("eac3", "eac3", "5.1", 6, "ita"),
            ][..],
        ),
        (
            "mpegts",
            &[
                ("aac", "aac", "stereo", 2, "eng"),
                ("ac3", "ac3", "5.1", 6, "fra"),
                ("mp2", "mp2", "stereo", 2, "deu"),
            ][..],
        ),
        (
            "avi",
            &[
                ("libmp3lame", "mp3", "stereo", 2, "eng"),
                ("ac3", "ac3", "5.1", 6, "eng"),
            ][..],
        ),
        (
            "asf",
            &[
                ("wmav2", "wmav2", "stereo", 2, "eng"),
                ("wmav2", "wmav2", "stereo", 2, "fra"),
            ][..],
        ),
    ] {
        let (video, vcodec, venc) = if muxer == "asf" {
            (s(&["-c:v", "wmv2", "-q:v", "8"]), "wmv2", "wmv2")
        } else {
            (x264_plain(), "h264", "libx264")
        };
        let mut a = vec![picture()];
        for (_, _, layout, _, _) in codecs {
            a.push(silence(layout, 48000));
        }
        let mut maps = s(&["-map", "0:v"]);
        let mut encoders = vec![venc];
        let mut e =
            Expect::video(container(muxer), vcodec, W, H, 24).check("summary.has_multiaudio", true);
        for (i, (encoder, codec, _, channels, language)) in codecs.iter().enumerate() {
            maps.extend(s(&["-map", &format!("{}:a", i + 1)]));
            maps.extend([
                format!("-c:a:{i}"),
                (*encoder).to_owned(),
                format!("-b:a:{i}"),
                "96k".to_owned(),
            ]);
            maps.extend([format!("-metadata:s:a:{i}"), format!("language={language}")]);
            encoders.push(*encoder);
            let reported = (muxer != "avi").then_some(*language);
            e = e.audio(codec, *channels, reported);
        }
        a.push(maps);
        a.push(video);
        a.push(s(&["-shortest"]));
        a.push(out(muxer));
        encoders.dedup();
        v.push(F::new(
            format!("multiaudio/{}_tracks.{}", codecs.len(), ext(muxer)),
            vec![ffmpeg(a)],
            e,
            &encoders,
        ));
    }
}

// ---------------------------------------------------------------------------
// Chapters, attachments, captions
// ---------------------------------------------------------------------------

const CHAPTERS: &str = ";FFMETADATA1\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=300\ntitle=Opening\n\
                        [CHAPTER]\nTIMEBASE=1/1000\nSTART=300\nEND=700\ntitle=幕間\n\
                        [CHAPTER]\nTIMEBASE=1/1000\nSTART=700\nEND=1000\ntitle=Fin\n";

fn chapter_fixtures(v: &mut Vec<F>) {
    for (muxer, flags) in [
        ("matroska", &[][..]),
        ("webm", &[][..]),
        ("mov", &[][..]),
        ("mp4", &["-movflags", "+faststart"][..]),
        ("asf", &[][..]),
    ] {
        let (video, vcodec, venc) = match muxer {
            "webm" => (vp9("yuv420p", None), "vp9", "libvpx-vp9"),
            "asf" => (s(&["-c:v", "wmv2", "-q:v", "8"]), "wmv2", "wmv2"),
            _ => (x264_plain(), "h264", "libx264"),
        };
        let mut e =
            Expect::video(container(muxer), vcodec, W, H, 24).check("summary.num_chapters", 3i64);
        e.chapter_titles = vec!["Opening", "幕間", "Fin"];
        v.push(F::new(
            format!("chapters/three.{}", ext(muxer)),
            vec![
                write("chapters.txt", CHAPTERS),
                ffmpeg(vec![
                    picture(),
                    s(&[
                        "-i",
                        "{work}/chapters.txt",
                        "-map",
                        "0:v",
                        "-map_metadata",
                        "1",
                        "-map_chapters",
                        "1",
                    ]),
                    video,
                    s(flags),
                    out(muxer),
                ]),
            ],
            e,
            &[venc],
        ));
    }
    // Ogg chapters are Vorbis comments (CHAPTERnn); audio-only, as above.
    let mut e = Expect::audio_only("ogg")
        .audio("vorbis", 2, None)
        .check("summary.num_chapters", 3i64);
    e.chapter_titles = vec!["Opening", "幕間", "Fin"];
    v.push(F::new(
        "chapters/three.ogg",
        vec![
            write("chapters.txt", CHAPTERS),
            ffmpeg(vec![
                silence("stereo", 48000),
                s(&[
                    "-i",
                    "{work}/chapters.txt",
                    "-map",
                    "0:a",
                    "-map_metadata",
                    "1",
                    "-map_chapters",
                    "1",
                ]),
                audio("vorbis"),
                out("ogg"),
            ]),
        ],
        e,
        &["vorbis"],
    ));
    // Many chapters in Matroska (one per 10 ms).
    let mut many = String::from(";FFMETADATA1\n");
    for i in 0..100 {
        many.push_str(&format!(
            "[CHAPTER]\nTIMEBASE=1/1000\nSTART={}\nEND={}\ntitle=Chapter {:03}\n",
            i * 10,
            i * 10 + 10,
            i + 1
        ));
    }
    v.push(F::new(
        "chapters/hundred.mkv",
        vec![
            write("chapters.txt", &many),
            ffmpeg(vec![
                picture(),
                s(&[
                    "-i",
                    "{work}/chapters.txt",
                    "-map",
                    "0:v",
                    "-map_chapters",
                    "1",
                ]),
                x264_plain(),
                out("matroska"),
            ]),
        ],
        Expect::video("matroska", "h264", W, H, 24).check("summary.num_chapters", 100i64),
        &["libx264"],
    ));
}

fn attachment_fixtures(v: &mut Vec<F>) {
    // A stand-in font file: attachment parsing reads names and MIME types,
    // not font tables.
    v.push(F::new("attach/font_and_text.mkv",
        vec![
            write("Fixture-Regular.ttf", "\u{0}\u{1}\u{0}\u{0}fixture-font-placeholder"),
            write("notes.txt", "fixture attachment\n"),
            write("s.ass", "[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize\nStyle: Default,Fixture,20\n\n[Events]\nFormat: Layer, Start, End, Style, Text\nDialogue: 0,0:00:00.00,0:00:01.00,Default,fixture\n"),
            ffmpeg(vec![picture(), s(&["-i", "{work}/s.ass", "-map", "0:v", "-map", "1:s", "-c:s", "ass",
                "-attach", "{work}/Fixture-Regular.ttf", "-metadata:s:t:0", "mimetype=font/ttf",
                "-attach", "{work}/notes.txt", "-metadata:s:t:1", "mimetype=text/plain"]), x264_plain(), out("matroska")]),
        ],
        {
            let mut e = Expect::video("matroska", "h264", W, H, 24)
                .check("attachment.0.name", "Fixture-Regular.ttf")
                .check("attachment.0.media_type", "font/ttf")
                .check("attachment.1.name", "notes.txt")
                .check("attachment.1.media_type", "text/plain");
            e.subtitle_stream_count = 1;
            e
        },
        &["libx264", "ass"]));
}

fn caption_fixtures(v: &mut Vec<F>) {
    let captions = |e: Expect| e.present("caption.0");
    // H.264 and HEVC with A/53 caption SEI, muxed by FFmpeg.
    for (codec, nal, encode, encoder, raw) in [
        ("h264", NalCodec::H264, x264_plain(), "libx264", "h264"),
        (
            "hevc",
            NalCodec::Hevc,
            no_bframes(s(&[
                "-c:v",
                "libx265",
                "-preset",
                "ultrafast",
                "-x265-params",
                "log-level=error",
            ])),
            "libx265",
            "hevc",
        ),
    ] {
        for (muxer, flags) in [("mpegts", &[][..]), ("matroska", &[][..]), ("mp4", &[][..])] {
            v.push(F::new(
                format!("captions/{codec}_a53.{}", ext(muxer)),
                vec![
                    ffmpeg(vec![picture(), encode.clone(), to(raw, "{work}/v.es")]),
                    inject(
                        "{work}/v.es",
                        "{work}/i.es",
                        Inject::AnnexB {
                            codec: nal,
                            inserts: vec![NalInsert::Captions],
                        },
                    ),
                    ffmpeg(vec![raw_copy(raw, 24, "{work}/i.es"), s(flags), out(muxer)]),
                ],
                captions(Expect::video(container(muxer), codec, W, H, 24)),
                &[encoder],
            ));
        }
    }
    // MPEG-2 with A/53 user data, in TS, PS and Matroska.
    for (muxer, name) in [
        ("mpegts", "captions/mpeg2_a53.ts"),
        ("vob", "captions/mpeg2_a53.vob"),
        ("matroska", "captions/mpeg2_a53.mkv"),
    ] {
        v.push(F::new(
            name,
            vec![
                ffmpeg(vec![
                    pattern("128x96", 25, "1"),
                    s(&["-c:v", "mpeg2video", "-b:v", "500k"]),
                    to("mpeg2video", "{work}/v.m2v"),
                ]),
                inject("{work}/v.m2v", "{work}/i.m2v", Inject::Mpeg2Captions),
                ffmpeg(vec![raw_copy("mpegvideo", 25, "{work}/i.m2v"), out(muxer)]),
            ],
            captions(Expect::video(container(muxer), "mpeg2video", 128, 96, 25)),
            &["mpeg2video"],
        ));
    }
    let mut e = captions(Expect::video("mpegvideo", "mpeg2video", 128, 96, 25));
    e.min_duration_seconds = 0;
    v.push(F::new(
        "captions/mpeg2_a53.m2v",
        vec![
            ffmpeg(vec![
                pattern("128x96", 25, "1"),
                s(&["-c:v", "mpeg2video", "-b:v", "500k"]),
                to("mpeg2video", "{work}/v.m2v"),
            ]),
            inject("{work}/v.m2v", "{out}", Inject::Mpeg2Captions),
        ],
        e,
        &["mpeg2video"],
    ));
}

// ---------------------------------------------------------------------------
// Geometry: aspect, rotation, interlacing, stereo 3D, VFR, odd sizes
// ---------------------------------------------------------------------------

fn geometry_fixtures(v: &mut Vec<F>) {
    // Display aspect from the container (pasp, DisplayWidth) and from the bitstream (VUI SAR).
    for (stem, muxer, flags, sar) in [
        ("dar_16x9_pasp", "mp4", &["-aspect", "16:9"][..], (16, 9)),
        (
            "dar_16x9_display",
            "matroska",
            &["-aspect", "16:9"][..],
            (16, 9),
        ),
        ("dar_16x9_mov", "mov", &["-aspect", "16:9"][..], (16, 9)),
        (
            "sar_4x3_vui",
            "matroska",
            &["-vf", "setsar=4/3"][..],
            (4, 3),
        ),
        ("sar_4x3_vui", "mpegts", &["-vf", "setsar=4/3"][..], (4, 3)),
        ("sar_4x3_vui", "avi", &["-vf", "setsar=4/3"][..], (4, 3)),
    ] {
        v.push(F::new(
            format!("geometry/{stem}.{}", ext(muxer)),
            vec![ffmpeg(vec![picture(), x264_plain(), s(flags), out(muxer)])],
            Expect::video(container(muxer), "h264", W, H, 24)
                .check(
                    "video.metadata.display_aspect_ratio.numerator",
                    i64::from(sar.0),
                )
                .check(
                    "video.metadata.display_aspect_ratio.denominator",
                    i64::from(sar.1),
                ),
            &["libx264"],
        ));
    }
    v.push(F::new(
        "geometry/mpeg2_dar_16x9.mpg",
        vec![ffmpeg(vec![
            pattern("128x96", 25, "1"),
            s(&["-c:v", "mpeg2video", "-b:v", "500k", "-aspect", "16:9"]),
            out("mpeg"),
        ])],
        Expect::video("mpeg", "mpeg2video", 128, 96, 25)
            .check("video.metadata.display_aspect_ratio.numerator", 16i64)
            .check("video.metadata.display_aspect_ratio.denominator", 9i64),
        &["mpeg2video"],
    ));
    // Rotation through the display matrix.
    // The matrix is set on a stream copy: an encode applies it to the pixels
    // and drops it. `-display_rotation -D` asks for a clockwise turn of D
    // degrees, which the display matrix states as a counter-clockwise angle
    // of -D normalised to (-180, 180]; an exact half turn reads as -180.
    for (degrees, reported, muxer) in [
        (90, -90.0, "mp4"),
        (180, -180.0, "mp4"),
        (270, 90.0, "mp4"),
        (90, -90.0, "mov"),
        (90, -90.0, "matroska"),
    ] {
        v.push(F::new(
            format!("geometry/rotate_{degrees}.{}", ext(muxer)),
            vec![
                ffmpeg(vec![
                    picture(),
                    x264_plain(),
                    to("matroska", "{work}/upright.mkv"),
                ]),
                Step::Ffmpeg(s(&[
                    "-display_rotation",
                    &format!("-{degrees}"),
                    "-i",
                    "{work}/upright.mkv",
                    "-c",
                    "copy",
                    "-f",
                    muxer,
                    "{out}",
                ])),
            ],
            Expect::video(container(muxer), "h264", W, H, 24)
                .check("video.metadata.rotation_degrees", reported),
            &["libx264"],
        ));
    }
    // Interlacing and field order.
    for (stem, muxer, encode, codec, encoder, order) in [
        (
            "h264_tff",
            "matroska",
            [
                x264_plain(),
                s(&[
                    "-flags",
                    "+ildct+ilme",
                    "-x264-params",
                    "interlaced=1:tff=1",
                    "-field_order",
                    "tt",
                ]),
            ]
            .concat(),
            "h264",
            "libx264",
            "tt",
        ),
        (
            "h264_bff",
            "mpegts",
            [
                x264_plain(),
                s(&[
                    "-flags",
                    "+ildct+ilme",
                    "-x264-params",
                    "interlaced=1:bff=1",
                    "-field_order",
                    "bb",
                ]),
            ]
            .concat(),
            "h264",
            "libx264",
            "bb",
        ),
        (
            "h264_tff",
            "mp4",
            [
                x264_plain(),
                s(&[
                    "-flags",
                    "+ildct+ilme",
                    "-x264-params",
                    "interlaced=1:tff=1",
                    "-field_order",
                    "tt",
                ]),
            ]
            .concat(),
            "h264",
            "libx264",
            "tt",
        ),
        (
            "mpeg2_tff",
            "matroska",
            s(&[
                "-c:v",
                "mpeg2video",
                "-b:v",
                "500k",
                "-flags",
                "+ildct+ilme",
                "-top",
                "1",
                "-field_order",
                "tt",
            ]),
            "mpeg2video",
            "mpeg2video",
            "tt",
        ),
        (
            "mpeg2_bff",
            "mov",
            s(&[
                "-c:v",
                "mpeg2video",
                "-b:v",
                "500k",
                "-flags",
                "+ildct+ilme",
                "-top",
                "0",
                "-field_order",
                "bb",
            ]),
            "mpeg2video",
            "mpeg2video",
            "bb",
        ),
        (
            "mpeg2_bff",
            "mpeg",
            s(&[
                "-c:v",
                "mpeg2video",
                "-b:v",
                "500k",
                "-flags",
                "+ildct+ilme",
                "-top",
                "0",
            ]),
            "mpeg2video",
            "mpeg2video",
            "bb",
        ),
        (
            "prores_tff",
            "mov",
            s(&[
                "-c:v",
                "prores_ks",
                "-profile:v",
                "0",
                "-flags",
                "+ildct",
                "-field_order",
                "tt",
            ]),
            "prores",
            "prores_ks",
            "tt",
        ),
        (
            "hevc_progressive",
            "matroska",
            s(&[
                "-c:v",
                "libx265",
                "-preset",
                "ultrafast",
                "-x265-params",
                "log-level=error",
            ]),
            "hevc",
            "libx265",
            "progressive",
        ),
    ] {
        let (size, w, h, rate) = if codec == "h264" || codec == "hevc" {
            (SIZE, W, H, 24)
        } else {
            ("128x96", 128, 96, 25)
        };
        v.push(F::new(
            format!("geometry/{stem}.{}", ext(muxer)),
            vec![ffmpeg(vec![pattern(size, rate, "1"), encode, out(muxer)])],
            Expect::video(container(muxer), codec, w, h, rate as i32)
                .check("video.metadata.field_order", order),
            &[encoder],
        ));
    }
    // Stereo 3D: Matroska StereoMode, MP4 st3d, H.264 frame-packing SEI.
    for (mode, stem) in [
        ("left_right", "sbs"),
        ("top_bottom", "tab"),
        ("block_lr", "checkerboard_lr"),
        ("row_interleaved_lr", "row_lr"),
        ("anaglyph_cyan_red", "anaglyph"),
    ] {
        v.push(F::new(
            format!("geometry/stereo_{stem}.mkv"),
            vec![ffmpeg(vec![
                picture(),
                x264_plain(),
                vec!["-metadata:s:v".into(), format!("stereo_mode={mode}")],
                out("matroska"),
            ])],
            Expect::video("matroska", "h264", W, H, 24).check("video.metadata.is_3d", true),
            &["libx264"],
        ));
    }
    for (stem, mode) in [("stereo_sbs", "left_right"), ("stereo_tab", "top_bottom")] {
        v.push(F::new(
            format!("geometry/{stem}.mp4"),
            vec![
                ffmpeg(vec![
                    picture(),
                    x264_plain(),
                    vec!["-metadata:s:v".into(), format!("stereo_mode={mode}")],
                    to("matroska", "{work}/s.mkv"),
                ]),
                remux_work("{work}/s.mkv", "mp4", &["-strict", "unofficial"]),
            ],
            Expect::video("mp4", "h264", W, H, 24).check("video.metadata.is_3d", true),
            &["libx264"],
        ));
    }
    v.push(F::new(
        "geometry/stereo_frame_packing_sei.mkv",
        vec![ffmpeg(vec![
            picture(),
            x264_plain(),
            s(&["-x264-params", "frame-packing=3"]),
            out("matroska"),
        ])],
        Expect::video("matroska", "h264", W, H, 24).check("video.metadata.is_3d", true),
        &["libx264"],
    ));
    v.push(F::new(
        "geometry/mono_2d.mkv",
        vec![ffmpeg(vec![
            picture(),
            x264_plain(),
            s(&["-metadata:s:v", "stereo_mode=mono"]),
            out("matroska"),
        ])],
        Expect::video("matroska", "h264", W, H, 24).check("video.metadata.is_3d", false),
        &["libx264"],
    ));
    // Variable frame rate: half the frames at 24 fps, half at 12 fps.
    let vfr = s(&[
        "-vf",
        "setpts='if(lt(N,12),N/24/TB,(0.5+(N-12)/12)/TB)'",
        "-fps_mode",
        "passthrough",
        "-enc_time_base",
        "1/1000",
    ]);
    for (muxer, vcodec, encode, encoder) in [
        ("matroska", "h264", x264_plain(), "libx264"),
        ("mp4", "h264", x264_plain(), "libx264"),
        ("asf", "wmv2", s(&["-c:v", "wmv2", "-q:v", "8"]), "wmv2"),
        ("webm", "vp9", vp9("yuv420p", None), "libvpx-vp9"),
    ] {
        v.push(F::new(
            format!("geometry/vfr.{}", ext(muxer)),
            vec![ffmpeg(vec![picture(), vfr.clone(), encode, out(muxer)])],
            // No single rate describes the stream, so the rate is not asserted.
            Expect::video(container(muxer), vcodec, W, H, 0)
                .check("video.metadata.variable_frame_rate", true),
            &[encoder],
        ));
    }
    // Fractional and high frame rates.
    for (stem, rate, fps, num, den) in [
        ("ntsc_23976", "24000/1001", 24, 24000, 1001),
        ("ntsc_2997", "30000/1001", 30, 30000, 1001),
        ("hfr_120", "120", 120, 120, 1),
    ] {
        let input = vec![
            "-f".into(),
            "lavfi".into(),
            "-i".into(),
            format!("testsrc2=size={SIZE}:rate={rate}:duration=1"),
        ];
        v.push(F::new(
            format!("geometry/{stem}.mkv"),
            vec![ffmpeg(vec![input, x264_plain(), out("matroska")])],
            Expect::video("matroska", "h264", W, H, fps)
                .check("video.metadata.declared_frame_rate.numerator", num)
                .check("video.metadata.declared_frame_rate.denominator", den),
            &["libx264"],
        ));
    }
    // Odd and extreme dimensions.
    for (stem, size, w, h, pix) in [
        ("odd_65x33", "65x33", 65, 33, "yuv444p"),
        ("tiny_16x16", "16x16", 16, 16, "yuv420p"),
        ("wide_1024x32", "1024x32", 1024, 32, "yuv420p"),
        ("tall_32x1024", "32x1024", 32, 1024, "yuv420p"),
    ] {
        v.push(F::new(
            format!("geometry/{stem}.mkv"),
            // testsrc, not testsrc2: testsrc2 rounds odd sizes down to even.
            vec![ffmpeg(vec![
                s(&[
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("testsrc=size={size}:rate=24:duration=1"),
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-pix_fmt",
                    pix,
                ]),
                out("matroska"),
            ])],
            Expect::video("matroska", "h264", w, h, 24),
            &["libx264"],
        ));
    }
    v.push(F::new(
        "geometry/cropped_sps_1080.mkv",
        vec![ffmpeg(vec![
            pattern("1920x1080", 24, "0.25"),
            s(&[
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
                "-crf",
                "51",
            ]),
            out("matroska"),
        ])],
        {
            let mut e = Expect::video("matroska", "h264", 1920, 1080, 24);
            e.min_duration_seconds = 0;
            e
        },
        &["libx264"],
    ));
}

fn remux_work(from: &str, muxer: &str, extra: &[&str]) -> Step {
    remux(from, muxer, extra)
}

// ---------------------------------------------------------------------------
// Programs, fragmented MP4, FLV, encryption and scrambling
// ---------------------------------------------------------------------------

fn program_fixtures(v: &mut Vec<F>) {
    // Only the 188-byte form: FFmpeg's m2ts mode allows one program, as Blu-ray does.
    {
        let (name, m2ts) = ("program/two_programs.ts", false);
        let mut a = vec![
            picture(),
            silence("stereo", 48000),
            pattern("128x72", 24, "1"),
            silence("stereo", 48000),
            s(&["-map", "0:v", "-map", "1:a", "-map", "2:v", "-map", "3:a"]),
            x264_plain(),
            s(&[
                "-c:a",
                "aac",
                "-b:a",
                "64k",
                "-program",
                "title=Fixture One:program_num=1:st=0:st=1",
                "-program",
                "title=Fixture Two:program_num=2:st=2:st=3",
                "-metadata:s:a:0",
                "language=eng",
                "-metadata:s:a:1",
                "language=fra",
                "-shortest",
            ]),
        ];
        if m2ts {
            a.push(s(&["-mpegts_m2ts_mode", "1"]));
        }
        a.push(out("mpegts"));
        v.push(F::new(
            name,
            vec![ffmpeg(a)],
            Expect::video("mpegts", "h264", W, H, 24)
                .audio("aac", 2, Some("eng"))
                .present("program.0")
                .present("program.1"),
            &["libx264", "aac"],
        ));
    }
    v.push(F::new(
        "program/service_metadata.ts",
        vec![ffmpeg(vec![
            picture(),
            silence("stereo", 48000),
            x264_plain(),
            s(&[
                "-c:a",
                "aac",
                "-b:a",
                "64k",
                "-metadata",
                "service_provider=Fixture Provider",
                "-metadata",
                "service_name=Fixture Service",
                "-mpegts_service_id",
                "4660",
                "-mpegts_pmt_start_pid",
                "0x0200",
                "-mpegts_start_pid",
                "0x0300",
                "-shortest",
            ]),
            out("mpegts"),
        ])],
        Expect::video("mpegts", "h264", W, H, 24)
            .audio("aac", 2, None)
            .present("program.0"),
        &["libx264", "aac"],
    ));
    v.push(F::new(
        "program/audio_only_second_program.ts",
        vec![ffmpeg(vec![
            picture(),
            silence("stereo", 48000),
            silence("stereo", 48000),
            s(&["-map", "0:v", "-map", "1:a", "-map", "2:a"]),
            x264_plain(),
            s(&[
                "-c:a",
                "mp2",
                "-b:a",
                "64k",
                "-program",
                "program_num=1:st=0:st=1",
                "-program",
                "program_num=2:st=2",
                "-shortest",
            ]),
            out("mpegts"),
        ])],
        Expect::video("mpegts", "h264", W, H, 24)
            .audio("mp2", 2, None)
            .present("program.1"),
        &["libx264", "mp2"],
    ));
}

fn fragmented_fixtures(v: &mut Vec<F>) {
    for (stem, flags) in [
        (
            "frag_keyframe",
            &["-movflags", "+frag_keyframe+empty_moov+default_base_moof"][..],
        ),
        ("cmaf", &["-movflags", "cmaf"][..]),
        (
            "global_sidx",
            &[
                "-movflags",
                "+frag_keyframe+empty_moov+default_base_moof+global_sidx",
            ][..],
        ),
        (
            "frag_every_frame",
            &[
                "-movflags",
                "+frag_every_frame+empty_moov+default_base_moof",
            ][..],
        ),
        (
            "dash_style",
            &[
                "-movflags",
                "+dash+frag_keyframe+empty_moov+delay_moov+default_base_moof",
            ][..],
        ),
    ] {
        v.push(F::new(
            format!("fmp4/{stem}.mp4"),
            vec![ffmpeg(vec![
                picture(),
                silence("stereo", 48000),
                x264_plain(),
                s(&["-c:a", "aac", "-b:a", "64k", "-g", "6", "-shortest"]),
                s(flags),
                out("mp4"),
            ])],
            Expect::video("mp4", "h264", W, H, 24).audio("aac", 2, None),
            &["libx264", "aac"],
        ));
    }
    v.push(F::new(
        "fmp4/hevc_hdr10_frag.mp4",
        vec![ffmpeg(vec![
            picture(),
            x265_hdr10(),
            s(&[
                "-tag:v",
                "hvc1",
                "-movflags",
                "+frag_keyframe+empty_moov+default_base_moof",
            ]),
            out("mp4"),
        ])],
        hdr10(Expect::video("mp4", "hevc", W, H, 24)),
        &["libx265"],
    ));
    v.push(F::new(
        "fmp4/smooth.ismv",
        vec![ffmpeg(vec![
            picture(),
            silence("stereo", 48000),
            x264_plain(),
            s(&["-c:a", "aac", "-b:a", "64k", "-shortest"]),
            to("ismv", "{out}"),
        ])],
        Expect::video("mp4", "h264", W, H, 24).audio("aac", 2, None),
        &["libx264", "aac"],
    ));
    // Enhanced FLV codecs.
    for (stem, encode, codec, encoder) in [
        (
            "hevc",
            s(&[
                "-c:v",
                "libx265",
                "-preset",
                "ultrafast",
                "-x265-params",
                "log-level=error",
            ]),
            "hevc",
            "libx265",
        ),
        (
            "av1",
            s(&["-c:v", "libsvtav1", "-preset", "12"]),
            "av1",
            "libsvtav1",
        ),
        ("vp9", vp9("yuv420p", None), "vp9", "libvpx-vp9"),
    ] {
        v.push(F::new(
            format!("flv/enhanced_{stem}.flv"),
            vec![ffmpeg(vec![
                picture(),
                silence("stereo", 44100),
                encode,
                s(&["-c:a", "aac", "-b:a", "64k", "-shortest"]),
                out("flv"),
            ])],
            Expect::video("flv", codec, W, H, 24).audio("aac", 2, None),
            &[encoder, "aac"],
        ));
    }
}

fn encrypted_fixtures(v: &mut Vec<F>) {
    let key = &[
        "-encryption_scheme",
        "cenc-aes-ctr",
        "-encryption_key",
        "00112233445566778899aabbccddeeff",
        "-encryption_kid",
        "0f0e0d0c0b0a09080706050403020100",
    ][..];
    v.push(F::new(
        "encrypted/cenc_h264_aac.mp4",
        vec![ffmpeg(vec![
            picture(),
            silence("stereo", 48000),
            x264_plain(),
            s(&["-c:a", "aac", "-b:a", "64k", "-shortest"]),
            s(key),
            out("mp4"),
        ])],
        Expect::video("mp4", "h264", W, H, 24).audio("aac", 2, None),
        &["libx264", "aac"],
    ));
    v.push(F::new(
        "encrypted/cenc_hevc_frag.mp4",
        vec![ffmpeg(vec![
            picture(),
            s(&[
                "-c:v",
                "libx265",
                "-preset",
                "ultrafast",
                "-x265-params",
                "log-level=error",
                "-tag:v",
                "hvc1",
                "-movflags",
                "+frag_keyframe+empty_moov+default_base_moof",
            ]),
            s(key),
            out("mp4"),
        ])],
        Expect::video("mp4", "hevc", W, H, 24),
        &["libx265"],
    ));
    for (name, packet) in [
        ("encrypted/scrambled.ts", 188usize),
        ("encrypted/scrambled.m2ts", 192),
    ] {
        let mut encode = vec![
            picture(),
            silence("stereo", 48000),
            x264_plain(),
            s(&["-c:a", "aac", "-b:a", "64k", "-shortest"]),
        ];
        if packet == 192 {
            encode.push(s(&["-mpegts_m2ts_mode", "1"]));
        }
        encode.push(to("mpegts", "{work}/clear.ts"));
        // The PMT still names the codecs; everything read from the payload is
        // ciphertext, so only those and the diagnosis are asserted.
        let mut e = Expect::video("mpegts", "h264", 0, 0, 0).audio("aac", 0, None);
        e.payload_scrambled = true;
        e.diagnostic_status = Some("encrypted");
        v.push(F::new(
            name,
            vec![
                ffmpeg(encode),
                inject(
                    "{work}/clear.ts",
                    "{out}",
                    Inject::TsScramble {
                        packet_size: packet,
                    },
                ),
            ],
            e,
            &["libx264", "aac"],
        ));
    }
    // Only the video PES is scrambled: a stream id names MPEG video but not
    // which MPEG, so the video codec is not asserted; the AC-3 substream stays
    // clear.
    let mut e = Expect::audio_only("mpeg").audio("ac3", 2, None);
    e.payload_scrambled = true;
    e.diagnostic_status = Some("encrypted");
    v.push(F::new(
        "encrypted/scrambled_pes.vob",
        vec![
            ffmpeg(vec![
                pattern("128x96", 25, "1"),
                silence("stereo", 48000),
                s(&[
                    "-c:v",
                    "mpeg2video",
                    "-b:v",
                    "500k",
                    "-c:a",
                    "ac3",
                    "-b:a",
                    "96k",
                    "-shortest",
                ]),
                to("vob", "{work}/clear.vob"),
            ]),
            inject("{work}/clear.vob", "{out}", Inject::PsScramble),
        ],
        e,
        &["mpeg2video", "ac3"],
    ));
}

pub(crate) fn fixtures() -> Vec<Fixture> {
    let mut v = Vec::new();
    hdr_fixtures(&mut v);
    dovi_fixtures(&mut v);
    codec_fixtures(&mut v);
    elementary_fixtures(&mut v);
    audio_fixtures(&mut v);
    subtitle_fixtures(&mut v);
    disposition_fixtures(&mut v);
    language_fixtures(&mut v);
    multiaudio_fixtures(&mut v);
    chapter_fixtures(&mut v);
    attachment_fixtures(&mut v);
    caption_fixtures(&mut v);
    geometry_fixtures(&mut v);
    program_fixtures(&mut v);
    fragmented_fixtures(&mut v);
    encrypted_fixtures(&mut v);
    finish(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dolby_vision_profile_and_signalling_path_is_covered() {
        let all = fixtures();
        let profiles: std::collections::BTreeSet<(u8, u8)> = all
            .iter()
            .filter_map(|f| f.expect.dovi_profile.zip(f.expect.dovi_bl_compat_id))
            .collect();
        for wanted in [
            (5, 0),
            (7, 6),
            (8, 1),
            (8, 2),
            (8, 4),
            (9, 2),
            (10, 0),
            (10, 1),
            (10, 4),
        ] {
            assert!(profiles.contains(&wanted), "{wanted:?}");
        }
        for name in [
            "dovi/p81_hvc1_dvvC.mkv",
            "dovi/p81_hvc1_dvvC.ts",
            "dovi/p20_hvc1_dvwC.mp4",
            "dovi/p81_rpu.mp4",
            "dovi/rpu_only_no_config.mkv",
        ] {
            assert!(all.iter().any(|f| f.name == name), "{name}");
        }
    }

    #[test]
    fn every_hdr_format_appears_for_every_codec_that_carries_it() {
        let all = fixtures();
        for (codec, format) in [
            ("hevc", "HDR10"),
            ("hevc", "HDR10+"),
            ("hevc", "HLG"),
            ("hevc", "Dolby Vision"),
            ("av1", "HDR10"),
            ("av1", "HDR10+"),
            ("av1", "HLG"),
            ("av1", "Dolby Vision"),
            ("vp9", "HDR10"),
            ("vp9", "HLG"),
            ("h264", "HDR10"),
            ("h264", "HLG"),
            ("h264", "Dolby Vision"),
        ] {
            assert!(
                all.iter()
                    .any(|f| f.expect.video_codec == Some(codec)
                        && f.expect.hdr_format == Some(format)),
                "{codec} {format}"
            );
        }
    }

    #[test]
    fn remuxes_follow_their_sources() {
        let all = fixtures();
        for (i, fixture) in all.iter().enumerate() {
            for step in &fixture.steps {
                if let Step::Ffmpeg(a) = step {
                    for arg in a.iter().filter_map(|arg| arg.strip_prefix("{media}/")) {
                        let source = all
                            .iter()
                            .position(|f| f.name == arg)
                            .unwrap_or_else(|| panic!("{arg}"));
                        assert!(source < i, "{} before {arg}", fixture.name);
                    }
                }
            }
        }
    }
}
