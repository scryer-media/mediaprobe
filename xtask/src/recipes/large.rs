//! Large fixtures that push the bounded-read and I/O-operation budgets. Each
//! is generated cheaply: a short encode, then stream-copy looping or a Rust
//! structure pass, so the publish runner does not spend minutes encoding.

use super::matrix::args;
use super::post::Post;
use super::{Expect, Fixture, Generator, Step};

/// 72 MiB: comfortably past the 64 MiB default read budget.
pub(crate) const MOOV_PADDING: u64 = 72 * 1024 * 1024;

fn large(mut e: Expect) -> Expect {
    e.may_exhaust_budget = true;
    e
}

pub(crate) fn fixtures() -> Vec<Fixture> {
    vec![
        // A movie header bigger than the read budget: a normal MP4 whose moov
        // (after mdat) gains a 72 MiB free box as its first child.
        Fixture {
            name: "large/mp4_moov_over_budget.mp4".into(),
            generator: Generator::Large,
            steps: vec![
                Step::Ffmpeg(args(&[
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=128x72:rate=24:duration=2",
                    "-f",
                    "lavfi",
                    "-t",
                    "2",
                    "-i",
                    "anullsrc=r=48000:cl=stereo",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:a",
                    "aac",
                    "-b:a",
                    "64k",
                    "-f",
                    "mp4",
                    "-shortest",
                    "{out}",
                ])),
                Step::Post(Post::GrowMoov {
                    bytes: MOOV_PADDING,
                }),
            ],
            expect: large(Expect::video("mp4", "h264", 128, 72, 24).audio("aac", 2, None)),
            encoders: vec!["libx264", "aac"],
        },
        // A four-million-entry sample table: one second of 1000 fps 16x16
        // video, stream-copied 4096 times. stsz alone is 16 MiB.
        Fixture {
            name: "large/mp4_sample_table_4m.mp4".into(),
            generator: Generator::Large,
            steps: vec![
                Step::Ffmpeg(args(&[
                    "-f",
                    "lavfi",
                    "-i",
                    "color=c=black:s=16x16:r=1000:d=1",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-g",
                    "1000000",
                    "-bf",
                    "0",
                    "-pix_fmt",
                    "yuv420p",
                    "-f",
                    "mp4",
                    "{work}/second.mp4",
                ])),
                Step::Ffmpeg(args(&[
                    "-stream_loop",
                    "4095",
                    "-i",
                    "{work}/second.mp4",
                    "-c",
                    "copy",
                    "-f",
                    "mp4",
                    "{out}",
                ])),
            ],
            expect: large(Expect::video("mp4", "h264", 16, 16, 1000)),
            encoders: vec!["libx264"],
        },
        // A live Matroska stream: no SeekHead, no Cues, unknown-size segment
        // and clusters, about 90 MiB of noisy interleaved video and audio, so
        // nothing short of a linear walk finds the end.
        Fixture {
            name: "large/mkv_live_interleaved.mkv".into(),
            generator: Generator::Large,
            steps: vec![Step::Ffmpeg(args(&[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=24:duration=90,noise=alls=100:allf=t",
                "-f",
                "lavfi",
                "-t",
                "90",
                "-i",
                "sine=frequency=440:sample_rate=48000",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-b:v",
                "8M",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
                "-metadata:s:a:0",
                "language=eng",
                "-f",
                "matroska",
                "-live",
                "1",
                "-shortest",
                "{out}",
            ]))],
            expect: {
                let mut e = large(Expect::video("matroska", "h264", 320, 180, 24).audio(
                    "aac",
                    1,
                    Some("eng"),
                ));
                e.min_duration_seconds = 1;
                e
            },
            encoders: vec!["libx264", "aac"],
        },
        // The same streams as a long interleaved MPEG-TS (stream copy).
        Fixture {
            name: "large/ts_interleaved_long.ts".into(),
            generator: Generator::Large,
            steps: vec![Step::Ffmpeg(args(&[
                "-i",
                "{media}/large/mkv_live_interleaved.mkv",
                "-c",
                "copy",
                "-f",
                "mpegts",
                "{out}",
            ]))],
            expect: {
                let mut e = large(Expect::video("mpegts", "h264", 320, 180, 24).audio(
                    "aac",
                    1,
                    Some("eng"),
                ));
                e.min_duration_seconds = 1;
                e
            },
            encoders: Vec::new(),
        },
    ]
}
