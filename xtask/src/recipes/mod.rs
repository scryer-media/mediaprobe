//! Every fixture in the corpus, as a recipe and the expectations it must meet.
//!
//! A recipe is a list of steps: FFmpeg invocations, a speexenc invocation, a
//! deterministic Rust post-processing pass over FFmpeg's output, or a Rust
//! builder for the few shapes FFmpeg cannot write at all. The expectations
//! (`tests/media-fixtures.toml`) are rendered from the same table, so a recipe
//! and what the tests hold it to cannot drift apart.

pub(crate) mod builders;
pub(crate) mod coverage;
pub(crate) mod disc;
pub(crate) mod imported;
pub(crate) mod inject;
pub(crate) mod large;
pub(crate) mod matrix;
pub(crate) mod post;
pub(crate) mod run;
pub(crate) mod supplemental;
pub(crate) mod synth;

use std::fmt::Write as _;

/// Which ledger generator a fixture belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Generator {
    /// The container x codec matrix ported from the original generator, plus
    /// the dense-index and SIMD-scanner fixtures.
    Matrix,
    /// Named fixtures for specific features: AV1, HDR10, HLG, HDR10+, chapters,
    /// program streams, 8K.
    Supplemental,
    /// HEVC encoded by FFmpeg with a Dolby Vision configuration box inserted
    /// afterwards (FFmpeg has no way to attach one from the command line).
    DolbyVision,
    /// Written by Rust code in xtask, because no FFmpeg muxer emits the shape.
    Builders,
    /// Large fixtures that push the bounded-read and I/O budgets.
    Large,
    /// The coverage matrix: every signal the parser classifies, across the
    /// containers that can carry it, with injected bitstream units where no
    /// encoder writes the signal.
    Coverage,
    /// Disc images (DVD-Video on ISO 9660, Blu-ray and UHD Blu-ray on UDF)
    /// written by Rust around FFmpeg-encoded program and transport streams.
    Disc,
    /// Files taken byte for byte from the repository the parser came from,
    /// because its own tests assert facts of those exact bytes.
    Imported,
    /// Large fixtures written at hydrate time from an imported seed plus
    /// format padding, mostly holes, and never published.
    Synthesized,
}

impl Generator {
    pub(crate) const ALL: [Generator; 9] = [
        Generator::Matrix,
        Generator::Supplemental,
        Generator::DolbyVision,
        Generator::Builders,
        Generator::Large,
        Generator::Coverage,
        Generator::Disc,
        Generator::Imported,
        Generator::Synthesized,
    ];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Generator::Matrix => "matrix",
            Generator::Supplemental => "supplemental",
            Generator::DolbyVision => "dolby_vision",
            Generator::Builders => "builders",
            Generator::Large => "large",
            Generator::Coverage => "coverage",
            Generator::Disc => "disc",
            Generator::Imported => "imported",
            Generator::Synthesized => "synthesized",
        }
    }

    pub(crate) fn path(self) -> &'static str {
        match self {
            Generator::Matrix => "xtask/src/recipes/matrix.rs",
            Generator::Supplemental => "xtask/src/recipes/supplemental.rs",
            Generator::DolbyVision => "xtask/src/recipes/supplemental.rs",
            Generator::Builders => "xtask/src/recipes/builders.rs",
            Generator::Large => "xtask/src/recipes/large.rs",
            Generator::Coverage => "xtask/src/recipes/coverage.rs",
            Generator::Disc => "xtask/src/recipes/disc.rs",
            Generator::Imported => "xtask/src/recipes/imported.rs",
            Generator::Synthesized => "xtask/src/recipes/synth.rs",
        }
    }

    pub(crate) fn toolchains(self) -> &'static [&'static str] {
        match self {
            Generator::Matrix => &["ffmpeg", "speexenc"],
            Generator::Imported | Generator::Synthesized => &[],
            _ => &["ffmpeg"],
        }
    }

    pub(crate) fn notes(self) -> &'static str {
        match self {
            Generator::Matrix => {
                "testsrc2 video and silent anullsrc audio across MKV, WebM, MP4/M4V/MOV, AVI, \
                 TS/M2TS, ASF, Ogg and FLV. Two TS fixtures re-frame another fixture's packets \
                 into 192- and 204-byte packets, and one ASF fixture has its PCM format rewritten \
                 to WAVE_FORMAT_EXTENSIBLE, both in Rust after FFmpeg. simd_scan_dense.bin is \
                 written by Rust: it is scanner input, not media."
            }
            Generator::Supplemental => {
                "AV1, HEVC HDR10/HLG/HDR10+ (x265 SEI and dhdr10-info), PQ sequence headers, \
                 Nero and QuickTime chapters, MPEG-1/2 program streams and an 8K H.264 stream."
            }
            Generator::DolbyVision => {
                "HEVC Main 10 PQ from libx265 with a dvcC box inserted into the hvc1 sample entry \
                 by Rust after FFmpeg; the Matroska variants are FFmpeg remuxes of those MP4s, \
                 which carry the record into BlockAdditionMapping. There is no RPU in the \
                 bitstream: the fixtures test the configuration record, not RPU parsing."
            }
            Generator::Builders => {
                "OGM (DirectShow stream headers inside Ogg), which no FFmpeg muxer writes. The \
                 Vorbis headers inside come from an FFmpeg-encoded Ogg Vorbis stream."
            }
            Generator::Large => {
                "Budget fixtures: an MP4 whose moov is larger than the 64 MiB read budget, an \
                 MP4 with a four-million-entry sample table, a Matroska live stream with no \
                 cues and unknown-size clusters, and a long interleaved MPEG-TS."
            }
            Generator::Coverage => {
                "One fixture per classifying branch of the parser: HDR10, PQ without static \
                 metadata, HLG and HDR10+ in HEVC, AV1, VP9 and H.264; Dolby Vision profiles 5, \
                 7, 8.1, 8.2, 8.4, 9 and 10 signalled through dvcC/dvvC/dvwC, Matroska \
                 BlockAdditionMapping, the TS DOVI descriptor and RPU NAL units; codec profiles, \
                 bit depths and chroma formats; audio codecs, layouts and profiles; subtitle \
                 formats; dispositions; language tags; chapters, attachments, cover art and \
                 captions; geometry (aspect ratios, rotation, interlacing, stereo 3D, variable \
                 frame rate); multi-program TS; and encrypted or scrambled payloads. Signals no \
                 encoder writes are authored in Rust after the encode: SEI messages, Dolby \
                 Vision RPU NAL units and configuration boxes, AV1 ITU-T T.35 metadata OBUs, \
                 A/53 caption user data and scrambling bits (xtask/src/recipes/inject.rs)."
            }
            Generator::Disc => {
                "DVD-Video ISO 9660 images (VIDEO_TS IFO navigation around an FFmpeg MPEG-2 \
                 program stream) and Blu-ray / UHD Blu-ray UDF 2.50 / 2.60 images (BDMV index, \
                 MovieObject, MPLS playlist and CLPI clip information around FFmpeg M2TS \
                 clips), written by xtask/src/recipes/disc.rs."
            }
            Generator::Imported => {
                "Files read byte for byte from the public repository the parser was extracted \
                 from, at a pinned commit, and checked against a pinned size and BLAKE3. The \
                 parser's own tests assert facts of these exact bytes, which a regenerated file \
                 does not reproduce. Not generated, so not byte-reproducible from a recipe."
            }
            Generator::Synthesized => {
                "Files past 1, 2 and 4 GiB written by Rust from an imported seed and format \
                 padding (an MP4 mdat, an EBML Void, null TS packets, an AVI JUNK chunk), sparse \
                 where the format allows. Never published: hydrate writes them from the seed and \
                 checks the size and BLAKE3 the recipe pins, so the bytes are reproducible."
            }
        }
    }
}

/// One step of a recipe. Arguments may use `{out}` (the fixture path),
/// `{work}` (a scratch directory for this fixture) and `{media}` (the media
/// root, for recipes derived from another fixture).
#[derive(Debug, Clone)]
pub(crate) enum Step {
    Ffmpeg(Vec<String>),
    Speexenc(Vec<String>),
    /// Write a text file into the work directory.
    WriteFile {
        name: String,
        contents: String,
    },
    /// A Rust pass over files already written.
    Post(post::Post),
    /// Rust writes the fixture outright.
    Build(builders::Builder),
    /// Fetch pinned bytes over HTTPS, checked against size and BLAKE3 before
    /// they land.
    Fetch {
        url: String,
        size: u64,
        blake3: String,
    },
    /// Write a large file from a seed fixture under the media root, checked
    /// against the pinned size and BLAKE3 when the recipe pins them.
    Synthesize {
        synth: synth::Synth,
        seed: &'static str,
        size: u64,
        blake3: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AudioExpect {
    pub(crate) codec: String,
    pub(crate) channels: i32,
    /// The language a parser must report; `None` for `und` or containers that
    /// carry none.
    pub(crate) language: Option<String>,
}

/// A value a check compares against.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Strs(Vec<String>),
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Str(value.to_owned())
    }
}
impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}
impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Float(value)
    }
}
impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}
impl From<&[&str]> for Value {
    fn from(values: &[&str]) -> Self {
        Value::Strs(values.iter().map(|v| (*v).to_owned()).collect())
    }
}

/// What a fixture must analyse as. Rendered into tests/media-fixtures.toml.
///
/// `checks`, `present` and `absent` address the serialized analysis by path:
/// the first segment selects `video` (the first video stream), `audio.N`,
/// `subtitle.N`, `attachment.N`, `chapter.N`, `program.N`, `caption.N`,
/// `details` or `summary` (the top-level summary fields), and the rest walks
/// object keys, for example `video.metadata.color.transfer`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Expect {
    pub(crate) container: &'static str,
    pub(crate) video_codec: Option<&'static str>,
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) fps: i32,
    pub(crate) audio: Vec<AudioExpect>,
    pub(crate) subtitle_stream_count: usize,
    pub(crate) min_duration_seconds: i32,
    pub(crate) valid_video: bool,
    pub(crate) video_bit_depth: Option<i32>,
    pub(crate) hdr_format: Option<&'static str>,
    pub(crate) dovi_profile: Option<u8>,
    pub(crate) dovi_bl_compat_id: Option<u8>,
    pub(crate) chapter_titles: Vec<&'static str>,
    /// Large fixtures: whether the analysis may stop at the budget, in which
    /// case only the invariants (no panic, reads and operations within budget)
    /// are checked when it does.
    pub(crate) may_exhaust_budget: bool,
    /// The payload is scrambled ciphertext. Only the container, the codecs
    /// named outside the payload (a `None` video codec is then not asserted,
    /// and an audio channel count of 0 is not asserted), the diagnosis and
    /// the explicit checks are asserted.
    pub(crate) payload_scrambled: bool,
    /// Exact values at analysis paths.
    pub(crate) checks: Vec<(String, Value)>,
    /// Paths that must hold a non-null value.
    pub(crate) present: Vec<String>,
    /// Paths that must be null or missing.
    pub(crate) absent: Vec<String>,
    /// Warning codes the analysis report must contain.
    pub(crate) warning_codes: Vec<&'static str>,
    /// The status structural diagnostics must report, when it is part of the fixture.
    pub(crate) diagnostic_status: Option<&'static str>,
}

impl Expect {
    pub(crate) fn video(
        container: &'static str,
        codec: &'static str,
        w: i32,
        h: i32,
        fps: i32,
    ) -> Self {
        Self {
            container,
            video_codec: Some(codec),
            width: w,
            height: h,
            fps,
            audio: Vec::new(),
            subtitle_stream_count: 0,
            min_duration_seconds: 1,
            valid_video: true,
            video_bit_depth: None,
            hdr_format: None,
            dovi_profile: None,
            dovi_bl_compat_id: None,
            chapter_titles: Vec::new(),
            may_exhaust_budget: false,
            payload_scrambled: false,
            checks: Vec::new(),
            present: Vec::new(),
            absent: Vec::new(),
            warning_codes: Vec::new(),
            diagnostic_status: None,
        }
    }

    /// A file with no video stream.
    pub(crate) fn audio_only(container: &'static str) -> Self {
        Self {
            video_codec: None,
            width: 0,
            height: 0,
            fps: 0,
            valid_video: false,
            ..Self::video(container, "", 0, 0, 0)
        }
    }

    pub(crate) fn check(mut self, path: &str, value: impl Into<Value>) -> Self {
        self.checks.push((path.to_owned(), value.into()));
        self
    }

    pub(crate) fn present(mut self, path: &str) -> Self {
        self.present.push(path.to_owned());
        self
    }

    pub(crate) fn absent(mut self, path: &str) -> Self {
        self.absent.push(path.to_owned());
        self
    }

    pub(crate) fn warning(mut self, code: &'static str) -> Self {
        self.warning_codes.push(code);
        self
    }

    pub(crate) fn audio(mut self, codec: &str, channels: i32, language: Option<&str>) -> Self {
        self.audio.push(AudioExpect {
            codec: codec.to_owned(),
            channels,
            language: language.map(str::to_owned),
        });
        self
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Fixture {
    /// Path under tests/media, forward slashes.
    pub(crate) name: String,
    pub(crate) generator: Generator,
    pub(crate) steps: Vec<Step>,
    pub(crate) expect: Expect,
    /// FFmpeg encoders the steps need, checked before running.
    pub(crate) encoders: Vec<&'static str>,
}

impl Fixture {
    pub(crate) fn path(&self) -> String {
        format!("{}/{}", crate::corpus::MEDIA_ROOT, self.name)
    }

    pub(crate) fn format(&self) -> String {
        self.name
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
    }

    pub(crate) fn profile(&self) -> &'static str {
        if matches!(self.generator, Generator::Large | Generator::Synthesized) {
            "large"
        } else {
            "unit"
        }
    }
}

/// Shapes the corpus does not carry because nothing it is allowed to run can
/// write them. Each is recorded in the ledger as `not_reproducible`.
pub(crate) struct NotReproducible {
    pub(crate) name: &'static str,
    pub(crate) reason: &'static str,
}

pub(crate) const NOT_REPRODUCIBLE: &[NotReproducible] = &[
    NotReproducible {
        name: "stubs/dv_profile5.mkv",
        reason: "The original 315-byte hand-authored Matroska metadata stub: a 3840x2160 HEVC \
                 track declared through CodecPrivate and a Dolby Vision BlockAdditionMapping with \
                 no decodable frames. FFmpeg writes those elements only around a real encoded \
                 stream. The configuration-record path is covered by dv_profile5.mkv (FFmpeg plus \
                 dvcC insertion, then an FFmpeg remux); exact stub bytes belong in the crate's \
                 own in-code builders, as the original suite's sparse_mkv support did.",
    },
    NotReproducible {
        name: "stubs/dv_profile7.mkv",
        reason: "Hand-authored metadata stub, as stubs/dv_profile5.mkv; covered by dv_profile7.mkv.",
    },
    NotReproducible {
        name: "stubs/dv_profile8.mkv",
        reason: "Hand-authored metadata stub, as stubs/dv_profile5.mkv; covered by dv_profile8.mkv.",
    },
    NotReproducible {
        name: "stubs/hevc_hdr10plus.mkv",
        reason: "The original 283-byte hand-authored stub: a 3840x2160 HEVC track whose only \
                 frame is an SEI carrying ITU-T T.35 HDR10+ metadata. Covered by \
                 hevc_hdr10plus.mkv, a real libx265 encode with dhdr10-info metadata.",
    },
    NotReproducible {
        name: "stubs/hevc_hdr10plus.mp4",
        reason: "The original 691-byte hand-authored MP4 stub of the same shape; covered by \
                 hevc_hdr10plus.mp4.",
    },
    NotReproducible {
        name: "codec/vvc.mkv",
        reason: "No VVC (H.266) encoder: the FFmpeg 8.1.1 build used to author the corpus does not link libvvenc, and no other encoder is allowed.",
    },
    NotReproducible {
        name: "audio/he_aac_v1.m4a",
        reason: "HE-AAC (SBR) needs libfdk_aac, which the FFmpeg 8.1.1 build used to author the corpus does not link; the native AAC encoder writes LC only.",
    },
    NotReproducible {
        name: "audio/he_aac_v2.m4a",
        reason: "HE-AACv2 (SBR + PS): as audio/he_aac_v1.m4a.",
    },
    NotReproducible {
        name: "audio/aac_main.m4a",
        reason: "FFmpeg 8.1.1's native AAC encoder refuses the Main profile (\"Profile not supported\"); libfdk_aac is not linked.",
    },
    NotReproducible {
        name: "audio/aac_ltp.m4a",
        reason: "FFmpeg 8.1.1's native AAC encoder refuses the LTP profile (\"Profile not supported\"); libfdk_aac is not linked.",
    },
    NotReproducible {
        name: "audio/amr_nb.3gp",
        reason: "AMR-NB needs libopencore-amrnb, which the FFmpeg 8.1.1 build used to author the corpus does not link.",
    },
    NotReproducible {
        name: "audio/amr_wb.3gp",
        reason: "AMR-WB needs libvo-amrwbenc, which the FFmpeg 8.1.1 build used to author the corpus does not link.",
    },
    NotReproducible {
        name: "audio/eac3_joc_atmos.mp4",
        reason: "No encoder writes Dolby Atmos (E-AC-3 JOC); FFmpeg's E-AC-3 encoder writes no object audio, and the JOC extension cannot be authored honestly without a reference bitstream.",
    },
    NotReproducible {
        name: "audio/truehd_atmos.mkv",
        reason: "FFmpeg's TrueHD encoder writes no Atmos substream; as audio/eac3_joc_atmos.mp4.",
    },
    NotReproducible {
        name: "audio/dts_hd_ma.mkv",
        reason: "FFmpeg's DTS encoder writes the core only; DTS-HD MA, HRA, ES, 96/24, DTS:X and DTS Express extensions have no encoder.",
    },
    NotReproducible {
        name: "audio/wma_pro.wma",
        reason: "FFmpeg has a WMA Pro decoder but no encoder.",
    },
    NotReproducible {
        name: "audio/wma_lossless.wma",
        reason: "FFmpeg has a WMA Lossless decoder but no encoder.",
    },
    NotReproducible {
        name: "codec/vc1.wmv",
        reason: "FFmpeg has VC-1 / WMV3 decoders but no encoder.",
    },
    NotReproducible {
        name: "codec/svq3.mov",
        reason: "FFmpeg has a Sorenson Video 3 decoder but no encoder.",
    },
    NotReproducible {
        name: "subtitle/dvdsub.mkv",
        reason: "FFmpeg's dvdsub encoder needs bitmap subtitle input, and nothing allowed here produces bitmap subtitles (FFmpeg cannot render text subtitles to bitmaps).",
    },
    NotReproducible {
        name: "subtitle/pgs.m2ts",
        reason: "FFmpeg has no PGS (HDMV presentation graphics) encoder.",
    },
    NotReproducible {
        name: "subtitle/dvbsub.ts",
        reason: "As subtitle/dvdsub.mkv: the DVB subtitle encoder needs bitmap input.",
    },
    NotReproducible {
        name: "codec/av1_high_444.mkv",
        reason: "AV1 High (4:4:4) and Professional profiles: SVT-AV1, the only AV1 encoder linked here, writes Main profile only.",
    },
    NotReproducible {
        name: "dovi/dvwc_profile20.mkv",
        reason: "Matroska has no dvwC block-addition mapping that FFmpeg writes; the dvwC path is covered in MP4.",
    },
    NotReproducible {
        name: "encrypted/matroska_content_encryption.mkv",
        reason: "FFmpeg's Matroska muxer writes no ContentEncryption elements.",
    },
    NotReproducible {
        name: "codec/hevc_main_still_picture.mp4",
        reason: "Asked for -profile:v mainstillpicture (or msp) on a single 4:2:0 8-bit frame, the libx265 in the FFmpeg 8.1.1 build used to author the corpus writes general_profile_idc 4 (Format Range Extensions), not 3.",
    },
    NotReproducible {
        name: "hdr/av1_hdr10.ts",
        reason: "FFmpeg 8.1.1's TS muxer has no AV1 mapping: it writes the stream as private data that no demuxer reads as AV1. Covers every AV1-in-TS shape (av1_pq_no_static.ts, av1_hlg.ts, av1_hdr10plus.ts).",
    },
    NotReproducible {
        name: "dovi/p101_av01_dvvC.ts",
        reason: "AV1 in TS, as hdr/av1_hdr10.ts.",
    },
    NotReproducible {
        name: "codec/vp8.mov",
        reason: "FFmpeg's MOV muxer refuses VP8.",
    },
    NotReproducible {
        name: "codec/mpeg4_asp_gmc.avi",
        reason: "FFmpeg's MPEG-4 encoder has no global motion compensation.",
    },
    NotReproducible {
        name: "codec/dirac_vc2.ogv",
        reason: "FFmpeg's Ogg muxer refuses Dirac / VC-2.",
    },
    NotReproducible {
        name: "lang/iso639_2_ita.ogv",
        reason: "Ogg video needs Theora or Dirac; covered audio-only by lang/iso639_2_ita.ogg.",
    },
    NotReproducible {
        name: "program/two_programs.m2ts",
        reason: "FFmpeg's m2ts mode allows one program (as Blu-ray does); two programs are covered in 188-byte TS by program/two_programs.ts.",
    },
];

/// The whole corpus, in ledger order.
pub(crate) fn all() -> Vec<Fixture> {
    let mut fixtures = matrix::fixtures();
    fixtures.extend(supplemental::fixtures());
    fixtures.extend(builders::fixtures());
    fixtures.extend(large::fixtures());
    fixtures.extend(coverage::fixtures());
    fixtures.extend(disc::fixtures());
    imported::apply(&mut fixtures);
    let synthesized = synth::fixtures(&fixtures);
    fixtures.extend(synthesized);
    fixtures
}

/// Render tests/media-fixtures.toml.
pub(crate) fn render_expectations(fixtures: &[Fixture]) -> String {
    let mut out = String::from(
        "# Rendered by `cargo xtask test-corpus expectations`; do not edit by hand.\n\
         # Every fixture the corpus carries and what it must analyse as. The fixture\n\
         # bytes are generated and published to the CDN, never committed: see\n\
         # docs/test-corpus.md.\n",
    );
    for fixture in fixtures {
        let e = &fixture.expect;
        out.push_str("\n[[fixtures]]\n");
        kv_str(&mut out, "name", &fixture.name);
        kv_str(&mut out, "generator", fixture.generator.id());
        kv_str(&mut out, "profile", fixture.profile());
        kv_str(&mut out, "container", e.container);
        if let Some(codec) = e.video_codec {
            kv_str(&mut out, "video_codec", codec);
        }
        let _ = writeln!(out, "width = {}", e.width);
        let _ = writeln!(out, "height = {}", e.height);
        let _ = writeln!(out, "fps = {}", e.fps);
        let codecs: Vec<String> = e.audio.iter().map(|a| a.codec.clone()).collect();
        kv_str_array(&mut out, "audio_codecs", &codecs);
        let channels: Vec<String> = e.audio.iter().map(|a| a.channels.to_string()).collect();
        let _ = writeln!(out, "audio_channels = [{}]", channels.join(", "));
        let languages: Vec<String> = e.audio.iter().filter_map(|a| a.language.clone()).collect();
        kv_str_array(&mut out, "audio_languages", &languages);
        let _ = writeln!(out, "subtitle_stream_count = {}", e.subtitle_stream_count);
        let _ = writeln!(out, "min_duration_seconds = {}", e.min_duration_seconds);
        let _ = writeln!(out, "valid_video = {}", e.valid_video);
        if let Some(depth) = e.video_bit_depth {
            let _ = writeln!(out, "video_bit_depth = {depth}");
        }
        if let Some(hdr) = e.hdr_format {
            kv_str(&mut out, "hdr_format", hdr);
        }
        if let Some(profile) = e.dovi_profile {
            let _ = writeln!(out, "dovi_profile = {profile}");
        }
        if let Some(compat) = e.dovi_bl_compat_id {
            let _ = writeln!(out, "dovi_bl_compat_id = {compat}");
        }
        if !e.chapter_titles.is_empty() {
            let titles: Vec<String> = e.chapter_titles.iter().map(|t| (*t).to_owned()).collect();
            kv_str_array(&mut out, "chapter_titles", &titles);
        }
        if e.may_exhaust_budget {
            out.push_str("may_exhaust_budget = true\n");
        }
        if e.payload_scrambled {
            out.push_str("payload_scrambled = true\n");
        }
        if !e.warning_codes.is_empty() {
            let codes: Vec<String> = e.warning_codes.iter().map(|c| (*c).to_owned()).collect();
            kv_str_array(&mut out, "warning_codes", &codes);
        }
        if let Some(status) = e.diagnostic_status {
            kv_str(&mut out, "diagnostic_status", status);
        }
        if !e.present.is_empty() {
            kv_str_array(&mut out, "present", &e.present);
        }
        if !e.absent.is_empty() {
            kv_str_array(&mut out, "absent", &e.absent);
        }
        if !e.checks.is_empty() {
            out.push_str("\n[fixtures.checks]\n");
            for (path, value) in &e.checks {
                let key = format!("\"{}\"", toml_escape(path));
                let rendered = match value {
                    Value::Str(s) => format!("\"{}\"", toml_escape(s)),
                    Value::Int(i) => i.to_string(),
                    Value::Float(f) => format!("{f:?}"),
                    Value::Bool(b) => b.to_string(),
                    Value::Strs(items) => {
                        let quoted: Vec<String> = items
                            .iter()
                            .map(|v| format!("\"{}\"", toml_escape(v)))
                            .collect();
                        format!("[{}]", quoted.join(", "))
                    }
                };
                let _ = writeln!(out, "{key} = {rendered}");
            }
        }
    }
    out
}

fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn kv_str(out: &mut String, key: &str, value: &str) {
    let _ = writeln!(out, "{key} = \"{}\"", toml_escape(value));
}

fn kv_str_array(out: &mut String, key: &str, values: &[String]) {
    let quoted: Vec<String> = values
        .iter()
        .map(|v| format!("\"{}\"", toml_escape(v)))
        .collect();
    let _ = writeln!(out, "{key} = [{}]", quoted.join(", "));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn fixture_names_are_unique_and_ledger_safe() {
        let fixtures = all();
        let mut seen = BTreeSet::new();
        for fixture in &fixtures {
            assert!(
                seen.insert(fixture.name.clone()),
                "duplicate {}",
                fixture.name
            );
            assert!(
                crate::corpus::valid_repo_relative(&fixture.path()),
                "{}",
                fixture.name
            );
            assert!(!fixture.steps.is_empty(), "{} has no steps", fixture.name);
        }
        for blocked in NOT_REPRODUCIBLE {
            assert!(!seen.contains(blocked.name), "{} is both", blocked.name);
        }
    }

    #[test]
    fn committed_expectations_match_the_recipes() {
        let rendered = render_expectations(&all());
        let path = crate::corpus::repo_path(crate::corpus::EXPECTATIONS);
        let committed = std::fs::read_to_string(&path).unwrap();
        assert!(
            committed == rendered,
            "{} is stale: run `cargo xtask test-corpus expectations`",
            path.display()
        );
    }

    /// Every encoder an FFmpeg step selects is declared on its fixture, and
    /// every declared encoder is in test-corpus/toolchain.json, so a build that
    /// lacks one skips the fixture by name instead of failing mid-run.
    #[test]
    fn encoders_are_declared_and_listed() {
        let toolchain = crate::corpus::toolchain::ToolchainFile::load(&crate::corpus::repo_path(
            crate::corpus::TOOLCHAIN,
        ))
        .unwrap();
        let listed: BTreeSet<&str> = toolchain.tools["ffmpeg"]
            .encoders
            .iter()
            .map(String::as_str)
            .collect();
        let mut used = BTreeSet::new();
        for fixture in all() {
            let declared: BTreeSet<&str> = fixture.encoders.iter().copied().collect();
            for step in &fixture.steps {
                let Step::Ffmpeg(args) = step else { continue };
                for pair in args.windows(2) {
                    let flag = pair[0].as_str();
                    let selects = flag == "-c"
                        || flag == "-codec"
                        || flag == "-vcodec"
                        || flag == "-acodec"
                        || flag == "-scodec"
                        || flag.starts_with("-c:")
                        || flag.starts_with("-codec:");
                    if !selects || pair[1] == "copy" {
                        continue;
                    }
                    assert!(
                        declared.contains(pair[1].as_str()),
                        "{} selects encoder {} without declaring it",
                        fixture.name,
                        pair[1]
                    );
                }
            }
            used.extend(declared);
        }
        let absent: Vec<&&str> = used.iter().filter(|e| !listed.contains(**e)).collect();
        assert!(
            absent.is_empty(),
            "fixtures need encoders toolchain.json does not list: {absent:?}"
        );
        let unused: Vec<&&str> = listed.iter().filter(|e| !used.contains(**e)).collect();
        assert!(
            unused.is_empty(),
            "toolchain.json lists encoders no fixture uses: {unused:?}"
        );
    }

    #[test]
    fn audio_expectations_line_up() {
        for fixture in all() {
            for audio in &fixture.expect.audio {
                assert!(
                    audio.channels > 0 || fixture.expect.payload_scrambled,
                    "{}",
                    fixture.name
                );
                assert_ne!(audio.language.as_deref(), Some("und"), "{}", fixture.name);
            }
        }
    }
}
