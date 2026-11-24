//! The container x codec matrix: 235 small fixtures that sweep video and
//! audio codecs, channel counts, languages, rates and dimensions across every
//! container family, plus the dense fixtures that keep the SIMD scanners and
//! the AVI index walk busy.

use super::builders::Builder;
use super::post::Post;
use super::{AudioExpect, Expect, Fixture, Generator, Step};

const WIDTHS: [i32; 4] = [64, 80, 96, 112];
const HEIGHTS: [i32; 4] = [36, 44, 54, 64];
const RATES: [i32; 5] = [12, 15, 24, 25, 30];
const CHANNELS: [i32; 3] = [1, 2, 6];
const LANGS: [&str; 4] = ["eng", "jpn", "spa", "und"];

#[derive(Clone)]
struct Case {
    name: String,
    container: &'static str,
    video: &'static str,
    width: i32,
    height: i32,
    fps: i32,
    audio: Vec<&'static str>,
    channels: Vec<i32>,
    languages: Vec<&'static str>,
    subtitle: bool,
    duration: f64,
    min_duration: i32,
    wave_extensible: bool,
}

struct Flags {
    subtitle: bool,
    dual: bool,
}

fn supported_channels(codec: &str, requested: i32) -> i32 {
    if codec == "vorbis" {
        return 2;
    }
    if matches!(codec, "mp3" | "mp2" | "wmav1" | "wmav2") {
        return requested.min(2);
    }
    requested
}

fn build_case(
    prefix: &str,
    index: usize,
    ext: &'static str,
    container: &'static str,
    video: &'static str,
    audio: &[&'static str],
    flags: Flags,
) -> Case {
    let mut audios = audio.to_vec();
    if flags.dual && audios.len() == 1 {
        let extra = if ext == "webm" {
            if audios[0] != "vorbis" {
                "vorbis"
            } else {
                "opus"
            }
        } else if ext == "mov" {
            "aac"
        } else if matches!(ext, "mp4" | "m4v") {
            if audios[0] == "aac" { "ac3" } else { "aac" }
        } else if audios[0] != "aac" {
            "aac"
        } else {
            "mp3"
        };
        audios.push(extra);
    }
    let mut channels: Vec<i32> = audios
        .iter()
        .enumerate()
        .map(|(stream, codec)| {
            supported_channels(codec, CHANNELS[(index + stream) % CHANNELS.len()])
        })
        .collect();
    if ext == "webm" {
        channels.fill(2);
    }
    let languages = (0..audios.len())
        .map(|stream| LANGS[(index + stream) % LANGS.len()])
        .collect();
    Case {
        name: format!("matrix_{prefix}_{index:03}.{ext}"),
        container,
        video,
        width: WIDTHS[index % WIDTHS.len()],
        height: HEIGHTS[index % HEIGHTS.len()],
        fps: RATES[index % RATES.len()],
        audio: audios,
        channels,
        languages,
        subtitle: flags.subtitle,
        duration: 1.4,
        min_duration: 1,
        wave_extensible: false,
    }
}

fn product(left: &[&'static str], right: &[&'static str]) -> Vec<(&'static str, &'static str)> {
    left.iter()
        .flat_map(|l| right.iter().map(move |r| (*l, *r)))
        .collect()
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();

    let mkv = product(
        &["h264", "mpeg4", "mjpeg", "mpeg2video", "vp9"],
        &["aac", "mp3", "flac", "ac3", "eac3", "opus", "vorbis"],
    );
    for i in 0..43 {
        let (v, a) = mkv[i % mkv.len()];
        cases.push(build_case(
            "mkv",
            i + 1,
            "mkv",
            "matroska",
            v,
            &[a],
            Flags {
                subtitle: i % 9 == 0,
                dual: i % 7 == 0,
            },
        ));
    }

    let webm = product(&["vp8", "vp9"], &["opus", "vorbis"]);
    for i in 0..12 {
        let (v, a) = webm[i % webm.len()];
        cases.push(build_case(
            "webm",
            i + 1,
            "webm",
            "webm",
            v,
            &[a],
            Flags {
                subtitle: false,
                dual: i % 5 == 0,
            },
        ));
    }

    let mp4_exts = ["mp4", "m4v", "mov"];
    let mp4 = product(&["h264", "mpeg4", "vp9"], &["aac", "ac3", "eac3", "opus"]);
    for i in 0..60 {
        let ext = mp4_exts[i % mp4_exts.len()];
        let (mut v, mut a) = mp4[i % mp4.len()];
        if ext == "mov" {
            v = "h264";
            a = "aac";
        }
        let container = if ext == "mov" { "mov" } else { "mp4" };
        cases.push(build_case(
            "mp4",
            i + 1,
            ext,
            container,
            v,
            &[a],
            Flags {
                subtitle: i % 10 == 0,
                dual: i % 8 == 0,
            },
        ));
    }

    let avi = product(
        &["mpeg4", "mjpeg", "mpeg2video", "h264"],
        &["mp3", "pcm_s16le", "ac3", "aac"],
    );
    for i in 0..40 {
        let (v, a) = avi[i % avi.len()];
        cases.push(build_case(
            "avi",
            i + 1,
            "avi",
            "avi",
            v,
            &[a],
            Flags {
                subtitle: false,
                dual: i % 6 == 0,
            },
        ));
    }

    let ts_exts = ["ts", "m2ts"];
    let ts = product(
        &["h264", "mpeg2video"],
        &["aac", "mp2", "ac3", "eac3", "dts"],
    );
    for i in 0..45 {
        let ext = ts_exts[i % ts_exts.len()];
        let (v, a) = ts[i % ts.len()];
        cases.push(build_case(
            "ts",
            i + 1,
            ext,
            "mpegts",
            v,
            &[a],
            Flags {
                subtitle: false,
                dual: i % 9 == 0,
            },
        ));
    }

    type Spec = (
        &'static str,
        &'static str,
        &'static [&'static str],
        &'static [i32],
    );
    const WMV: &[Spec] = &[
        ("wmv_wmv1_wmav1.wmv", "wmv1", &["wmav1"], &[1]),
        ("wmv_wmv2_wmav2.wmv", "wmv2", &["wmav2"], &[2]),
        ("wmv_wmv2_pcm_s24le.wmv", "wmv2", &["pcm_s24le"], &[2]),
        ("wmv_wmv1_mp3_mono.wmv", "wmv1", &["mp3"], &[1]),
        ("wmv_wmv2_pcm_u8_mono.wmv", "wmv2", &["pcm_u8"], &[1]),
        (
            "wmv_wmv1_pcm_s16le_stereo.wmv",
            "wmv1",
            &["pcm_s16le"],
            &[2],
        ),
        (
            "wmv_wmv2_pcm_s32le_stereo.wmv",
            "wmv2",
            &["pcm_s32le"],
            &[2],
        ),
        (
            "wmv_wmv1_pcm_f32le_stereo.wmv",
            "wmv1",
            &["pcm_f32le"],
            &[2],
        ),
        (
            "wmv_wmv2_pcm_s24le_surround.wmv",
            "wmv2",
            &["pcm_s24le"],
            &[6],
        ),
        ("wmv_wmv1_aac_surround.wmv", "wmv1", &["aac"], &[6]),
        ("wmv_wmv2_ac3_surround.wmv", "wmv2", &["ac3"], &[6]),
        (
            "wmv_wmv1_dual_wma.wmv",
            "wmv1",
            &["wmav1", "wmav2"],
            &[1, 2],
        ),
        ("wmv_wmv2_video_only.wmv", "wmv2", &[], &[]),
    ];
    for (index, &(name, video, audio, channels)) in WMV.iter().enumerate() {
        let mut case = build_case(
            "wmv",
            index + 1,
            "wmv",
            "asf",
            video,
            audio,
            Flags {
                subtitle: false,
                dual: false,
            },
        );
        case.name = name.into();
        case.channels = channels.to_vec();
        case.wave_extensible = name == "wmv_wmv2_pcm_s24le_surround.wmv";
        cases.push(case);
    }

    type OgvSpec = (&'static str, &'static [&'static str], &'static [i32]);
    const OGV: &[OgvSpec] = &[
        ("ogv_theora_vorbis.ogv", &["vorbis"], &[2]),
        ("ogv_theora_opus.ogv", &["opus"], &[6]),
        ("ogv_theora_opus_mono.ogv", &["opus"], &[1]),
        ("ogv_theora_opus_stereo.ogv", &["opus"], &[2]),
        ("ogv_theora_opus_surround.ogv", &["opus"], &[6]),
        (
            "ogv_theora_dual_vorbis_opus.ogv",
            &["vorbis", "opus"],
            &[2, 1],
        ),
        ("ogv_theora_dual_opus.ogv", &["opus", "opus"], &[1, 6]),
        ("ogv_theora_dual_vorbis.ogv", &["vorbis", "vorbis"], &[2, 2]),
        ("ogv_theora_vorbis_alt.ogv", &["vorbis"], &[2]),
        ("ogv_theora_video_only.ogv", &[], &[]),
    ];
    for (index, &(name, audio, channels)) in OGV.iter().enumerate() {
        let mut case = build_case(
            "ogv",
            index + 1,
            "ogv",
            "ogg",
            "theora",
            audio,
            Flags {
                subtitle: false,
                dual: false,
            },
        );
        case.name = name.into();
        case.channels = channels.to_vec();
        cases.push(case);
    }

    const FLV: &[Spec] = &[
        ("flv_flv1_mp3.flv", "flv1", &["mp3"], &[2]),
        ("flv_h264_aac.flv", "h264", &["aac"], &[6]),
        ("flv_flv1_pcm_s16le.flv", "flv1", &["pcm_s16le"], &[2]),
        ("flv_h264_mp3.flv", "h264", &["mp3"], &[1]),
        ("flv_flv1_pcm_u8.flv", "flv1", &["pcm_u8"], &[1]),
        ("flv_flv1_adpcm_swf.flv", "flv1", &["adpcm_swf"], &[1]),
        ("flv_flv1_nellymoser.flv", "flv1", &["nellymoser"], &[1]),
        ("flv_flv1_aac_mono.flv", "flv1", &["aac"], &[1]),
        ("flv_h264_pcm_alaw.flv", "h264", &["pcm_alaw"], &[1]),
        ("flv_flv1_pcm_mulaw.flv", "flv1", &["pcm_mulaw"], &[1]),
        ("flv_flv1_speex.flv", "flv1", &["speex"], &[1]),
        ("flv_flv1_video_only.flv", "flv1", &[], &[]),
    ];
    for (index, &(name, video, audio, channels)) in FLV.iter().enumerate() {
        let mut case = build_case(
            "flv",
            index + 1,
            "flv",
            "flv",
            video,
            audio,
            Flags {
                subtitle: false,
                dual: false,
            },
        );
        case.name = name.into();
        case.channels = channels.to_vec();
        cases.push(case);
    }
    assert_eq!(cases.len(), 235);
    cases
}

fn dense(
    name: &str,
    container: &'static str,
    video: &'static str,
    audio: Option<&'static str>,
) -> Case {
    Case {
        name: name.to_owned(),
        container,
        video,
        width: 32,
        height: 18,
        fps: 120,
        audio: audio.into_iter().collect(),
        channels: audio.map(|_| 2).into_iter().collect(),
        languages: audio.map(|_| "eng").into_iter().collect(),
        subtitle: false,
        duration: 8.0,
        min_duration: if audio.is_some() { 7 } else { 8 },
        wave_extensible: false,
    }
}

fn dense_cases() -> Vec<Case> {
    vec![
        dense("avi_idx1_dense_mpeg4.avi", "avi", "mpeg4", None),
        dense("simd_dense_h264_aac.ts", "mpegts", "h264", Some("aac")),
        dense(
            "simd_dense_mpeg2_mp2.ts",
            "mpegts",
            "mpeg2video",
            Some("mp2"),
        ),
        dense("simd_dense_h264_ac3.ts", "mpegts", "h264", Some("ac3")),
        dense("simd_dense_h264_dts.ts", "mpegts", "h264", Some("dts")),
        dense("simd_dense_vp9_opus.webm", "webm", "vp9", Some("opus")),
        dense("simd_dense_h264_aac.mp4", "mp4", "h264", Some("aac")),
    ]
}

fn ext(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or_default()
}

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

fn channel_layout(channels: i32) -> &'static str {
    match channels {
        1 => "mono",
        6 => "5.1",
        _ => "stereo",
    }
}

pub(crate) fn video_encoder(codec: &str) -> (Vec<String>, &'static str) {
    let (args, encoder): (&[&str], &str) = match codec {
        "h264" => (
            &[
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-tune",
                "zerolatency",
                "-pix_fmt",
                "yuv420p",
            ],
            "libx264",
        ),
        "mpeg4" => (
            &["-c:v", "mpeg4", "-q:v", "8", "-pix_fmt", "yuv420p"],
            "mpeg4",
        ),
        "mjpeg" => (
            &["-c:v", "mjpeg", "-q:v", "8", "-pix_fmt", "yuvj420p"],
            "mjpeg",
        ),
        "mpeg2video" => (
            &["-c:v", "mpeg2video", "-q:v", "8", "-pix_fmt", "yuv420p"],
            "mpeg2video",
        ),
        "vp8" => (
            &[
                "-c:v",
                "libvpx",
                "-deadline",
                "realtime",
                "-cpu-used",
                "8",
                "-b:v",
                "80k",
                "-pix_fmt",
                "yuv420p",
            ],
            "libvpx",
        ),
        "vp9" => (
            &[
                "-c:v",
                "libvpx-vp9",
                "-deadline",
                "realtime",
                "-cpu-used",
                "8",
                "-b:v",
                "80k",
                "-pix_fmt",
                "yuv420p",
            ],
            "libvpx-vp9",
        ),
        "wmv1" => (
            &["-c:v", "wmv1", "-q:v", "8", "-pix_fmt", "yuv420p"],
            "wmv1",
        ),
        "wmv2" => (
            &["-c:v", "wmv2", "-q:v", "8", "-pix_fmt", "yuv420p"],
            "wmv2",
        ),
        "theora" => (
            &["-c:v", "libtheora", "-q:v", "5", "-pix_fmt", "yuv420p"],
            "libtheora",
        ),
        "flv1" => (&["-c:v", "flv", "-q:v", "8", "-pix_fmt", "yuv420p"], "flv"),
        other => panic!("matrix video codec without an encoder: {other}"),
    };
    (args.iter().map(|s| (*s).to_owned()).collect(), encoder)
}

pub(crate) fn audio_encoder(codec: &str, stream: usize) -> (Vec<String>, Option<&'static str>) {
    let c = format!("-c:a:{stream}");
    let b = format!("-b:a:{stream}");
    let ar = format!("-ar:a:{stream}");
    let strict = format!("-strict:a:{stream}");
    let v = |pairs: &[(&str, &str)]| -> Vec<String> {
        pairs
            .iter()
            .flat_map(|(k, v)| [(*k).to_owned(), (*v).to_owned()])
            .collect()
    };
    match codec {
        "aac" => (v(&[(&c, "aac"), (&b, "64k")]), Some("aac")),
        "mp3" => (v(&[(&c, "libmp3lame"), (&b, "64k")]), Some("libmp3lame")),
        "pcm_u8" => (v(&[(&c, "pcm_u8"), (&ar, "44100")]), Some("pcm_u8")),
        "pcm_s16le" => (v(&[(&c, "pcm_s16le"), (&ar, "44100")]), Some("pcm_s16le")),
        "pcm_alaw" => (v(&[(&c, "pcm_alaw"), (&ar, "44100")]), Some("pcm_alaw")),
        "pcm_mulaw" => (v(&[(&c, "pcm_mulaw"), (&ar, "44100")]), Some("pcm_mulaw")),
        "pcm_s24le" => (v(&[(&c, "pcm_s24le")]), Some("pcm_s24le")),
        "pcm_s32le" => (v(&[(&c, "pcm_s32le")]), Some("pcm_s32le")),
        "pcm_f32le" => (v(&[(&c, "pcm_f32le")]), Some("pcm_f32le")),
        "adpcm_swf" => (v(&[(&c, "adpcm_swf"), (&ar, "44100")]), Some("adpcm_swf")),
        "nellymoser" => (v(&[(&c, "nellymoser"), (&ar, "16000")]), Some("nellymoser")),
        "speex" => (v(&[(&c, "copy")]), None),
        "ac3" => (v(&[(&c, "ac3"), (&b, "96k")]), Some("ac3")),
        "eac3" => (v(&[(&c, "eac3"), (&b, "96k")]), Some("eac3")),
        "flac" => (v(&[(&c, "flac")]), Some("flac")),
        "opus" => (v(&[(&c, "libopus"), (&b, "64k")]), Some("libopus")),
        "vorbis" => (v(&[(&c, "vorbis"), (&strict, "-2")]), Some("vorbis")),
        "mp2" => (v(&[(&c, "mp2"), (&b, "96k")]), Some("mp2")),
        "dts" => (v(&[(&c, "dca"), (&strict, "-2")]), Some("dca")),
        "wmav1" => (v(&[(&c, "wmav1"), (&b, "64k")]), Some("wmav1")),
        "wmav2" => (v(&[(&c, "wmav2"), (&b, "64k")]), Some("wmav2")),
        other => panic!("matrix audio codec without an encoder: {other}"),
    }
}

fn muxer(ext: &str) -> &'static str {
    match ext {
        "webm" => "webm",
        "mp4" | "m4v" => "mp4",
        "mov" => "mov",
        "ts" | "m2ts" => "mpegts",
        "avi" => "avi",
        "wmv" => "asf",
        "ogv" => "ogg",
        "flv" => "flv",
        "mkv" => "matroska",
        other => panic!("matrix extension without a muxer: {other}"),
    }
}

pub(crate) fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

fn steps(case: &Case) -> (Vec<Step>, Vec<&'static str>) {
    let mut steps = Vec::new();
    let mut encoders = Vec::new();
    let duration = case.duration.to_string();
    let speex = case.audio.contains(&"speex");
    let st = stem(&case.name);
    if speex {
        steps.push(Step::Ffmpeg(vec![
            "-f".into(),
            "lavfi".into(),
            "-i".into(),
            format!("sine=frequency=440:sample_rate=16000:duration={duration}"),
            "-c:a".into(),
            "pcm_s16le".into(),
            format!("{{work}}/{st}.wav"),
        ]));
        steps.push(Step::Speexenc(vec![
            format!("{{work}}/{st}.wav"),
            format!("{{work}}/{st}.spx"),
        ]));
        encoders.push("pcm_s16le");
    }
    let mut a = vec![
        "-f".to_owned(),
        "lavfi".into(),
        "-i".into(),
        format!(
            "testsrc2=size={}x{}:rate={}:duration={duration}",
            case.width, case.height, case.fps
        ),
    ];
    for (stream, channels) in case.channels.iter().enumerate() {
        if case.audio.get(stream) == Some(&"speex") {
            a.extend(["-i".into(), format!("{{work}}/{st}.spx")]);
        } else {
            a.extend([
                "-f".into(),
                "lavfi".into(),
                "-t".into(),
                duration.clone(),
                "-i".into(),
                format!("anullsrc=r=48000:cl={}", channel_layout(*channels)),
            ]);
        }
    }
    if case.subtitle {
        steps.push(Step::WriteFile {
            name: format!("{st}.srt"),
            contents: "1\n00:00:00,000 --> 00:00:01,000\nfixture\n".into(),
        });
        a.extend([
            "-f".into(),
            "srt".into(),
            "-i".into(),
            format!("{{work}}/{st}.srt"),
        ]);
    }
    a.extend(["-map".into(), "0:v:0".into()]);
    for stream in 0..case.channels.len() {
        a.extend(["-map".into(), format!("{}:a:0", stream + 1)]);
    }
    if case.subtitle {
        a.extend(["-map".into(), format!("{}:s:0", case.channels.len() + 1)]);
    }
    let (video_args, video_encoder) = video_encoder(case.video);
    a.extend(video_args);
    encoders.push(video_encoder);
    for (stream, codec) in case.audio.iter().enumerate() {
        let (audio_args, encoder) = audio_encoder(codec, stream);
        a.extend(audio_args);
        encoders.extend(encoder);
        a.extend([format!("-ac:a:{stream}"), case.channels[stream].to_string()]);
        a.extend([
            format!("-metadata:s:a:{stream}"),
            format!("language={}", case.languages[stream]),
        ]);
    }
    if case.subtitle {
        let codec = if matches!(ext(&case.name), "mp4" | "m4v" | "mov") {
            "mov_text"
        } else {
            "srt"
        };
        encoders.push(codec);
        a.extend(args(&["-c:s", codec, "-metadata:s:s:0", "language=eng"]));
    }
    a.extend([
        "-f".into(),
        muxer(ext(&case.name)).into(),
        "-shortest".into(),
        "{out}".into(),
    ]);
    steps.push(Step::Ffmpeg(a));
    if case.wave_extensible {
        steps.push(Step::Post(Post::AsfWaveExtensible));
    }
    (steps, encoders)
}

fn expect(case: &Case) -> Expect {
    let mut e = Expect::video(
        case.container,
        case.video,
        case.width,
        case.height,
        case.fps,
    );
    e.min_duration_seconds = case.min_duration;
    e.subtitle_stream_count = usize::from(case.subtitle);
    e.audio = case
        .audio
        .iter()
        .zip(&case.channels)
        .zip(&case.languages)
        .map(|((codec, channels), language)| AudioExpect {
            codec: (*codec).to_owned(),
            channels: *channels,
            language: (*language != "und" && !matches!(case.container, "avi" | "flv"))
                .then(|| (*language).to_owned()),
        })
        .collect();
    e
}

fn fixture(case: &Case) -> Fixture {
    let (steps, encoders) = steps(case);
    Fixture {
        name: case.name.clone(),
        generator: Generator::Matrix,
        steps,
        expect: expect(case),
        encoders,
    }
}

pub(crate) fn fixtures() -> Vec<Fixture> {
    let mut out: Vec<Fixture> = cases().iter().map(fixture).collect();
    let dense = dense_cases();
    out.extend(dense.iter().map(fixture));
    // Re-framed copies of simd_dense_h264_aac.ts: same packets, 192-byte
    // (M2TS timestamp prefix) and 204-byte (Reed-Solomon trailer) framing.
    let source = dense
        .iter()
        .find(|c| c.name == "simd_dense_h264_aac.ts")
        .cloned();
    if let Some(source) = source {
        for (name, packet, offset) in [
            ("simd_dense_h264_aac_192.m2ts", 192usize, 4usize),
            ("simd_dense_h264_aac_204.ts", 204, 0),
        ] {
            let mut case = source.clone();
            case.name = name.to_owned();
            out.push(Fixture {
                name: name.to_owned(),
                generator: Generator::Matrix,
                steps: vec![Step::Post(Post::TsReframe {
                    source: "simd_dense_h264_aac.ts".into(),
                    packet_size: packet,
                    sync_offset: offset,
                })],
                expect: expect(&case),
                encoders: Vec::new(),
            });
        }
    }
    let mut scan = Expect::video("none", "none", 0, 0, 0);
    scan.video_codec = None;
    scan.valid_video = false;
    scan.min_duration_seconds = 0;
    out.push(Fixture {
        name: "simd_scan_dense.bin".into(),
        generator: Generator::Matrix,
        steps: vec![Step::Build(Builder::SimdScanDense)],
        expect: scan,
        encoders: Vec::new(),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_keeps_the_original_shape() {
        let all = fixtures();
        // 235 matrix cases, 7 dense, 2 re-framed TS, 1 scanner input.
        assert_eq!(all.len(), 245);
        let speex = all.iter().find(|f| f.name == "flv_flv1_speex.flv").unwrap();
        assert!(matches!(speex.steps[1], Step::Speexenc(_)));
        let extensible = all
            .iter()
            .find(|f| f.name == "wmv_wmv2_pcm_s24le_surround.wmv")
            .unwrap();
        assert!(matches!(
            extensible.steps.last(),
            Some(Step::Post(Post::AsfWaveExtensible))
        ));
    }

    #[test]
    fn languages_follow_the_container_rules() {
        let all = fixtures();
        let avi = all.iter().find(|f| f.name == "matrix_avi_001.avi").unwrap();
        assert!(avi.expect.audio.iter().all(|a| a.language.is_none()));
        let mkv = all.iter().find(|f| f.name == "matrix_mkv_001.mkv").unwrap();
        assert_eq!(mkv.expect.audio[0].language.as_deref(), Some("jpn"));
    }
}
