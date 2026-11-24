//! The corpus harness checks itself without the parser, so it is green on
//! the stub and a broken expectation or ratchet entry fails before any lane
//! that needs the parser runs.

mod support;

use serde_json::json;
use std::collections::BTreeSet;
use support::{Fixture, Observed};

const PATH_HEADS: [&str; 9] = [
    "summary",
    "details",
    "video",
    "audio",
    "subtitle",
    "attachment",
    "chapter",
    "program",
    "caption",
];

#[test]
fn expectations_are_well_formed() {
    let fixtures = support::load_expectations();
    assert!(!fixtures.is_empty());
    let mut names = BTreeSet::new();
    for f in &fixtures {
        assert!(names.insert(f.name.clone()), "{} is listed twice", f.name);
        assert!(
            !f.name.starts_with('/')
                && !f.name.contains('\\')
                && f.name
                    .split('/')
                    .all(|p| !p.is_empty() && p != "." && p != ".."),
            "{}: not a plain relative path",
            f.name
        );
        assert!(!f.extension().is_empty(), "{}: no extension", f.name);
        assert!(
            matches!(f.profile.as_str(), "unit" | "large"),
            "{}: profile {}",
            f.name,
            f.profile
        );
        assert_eq!(
            f.profile == "large",
            f.name.starts_with("large/"),
            "{}: profile and directory disagree",
            f.name
        );
        assert_eq!(
            f.audio_codecs.len(),
            f.audio_channels.len(),
            "{}: audio arrays differ in length",
            f.name
        );
        assert!(
            f.width >= 0 && f.height >= 0 && f.fps >= 0,
            "{}: negative geometry",
            f.name
        );
        for path in f.present.iter().chain(&f.absent).chain(f.checks.keys()) {
            let head = path.split('.').next().unwrap_or_default();
            assert!(
                PATH_HEADS.contains(&head),
                "{}: path {path} has an unknown head",
                f.name
            );
        }
        if f.container == "none" {
            assert!(
                f.video_codec.is_none() && !f.valid_video,
                "{}: container none with streams",
                f.name
            );
        }
    }
}

#[test]
fn known_gaps_name_real_fixtures() {
    let names: BTreeSet<String> = support::load_expectations()
        .into_iter()
        .map(|f| f.name)
        .collect();
    let gaps = support::load_known_gaps();
    for name in gaps.keys() {
        assert!(
            names.contains(name),
            "{}: {name} is not a fixture",
            support::KNOWN_GAPS
        );
    }
    // The file is exactly what the renderer writes, so a bless leaves no diff
    // beyond the entries that changed.
    if let Ok(text) = std::fs::read_to_string(support::repo_path(support::KNOWN_GAPS)) {
        assert_eq!(
            text,
            support::render_known_gaps(&gaps),
            "{} is not in rendered form",
            support::KNOWN_GAPS
        );
    }
}

fn fixture(extra: &str) -> Fixture {
    let text = format!(
        r#"[[fixtures]]
name = "unit/sample.mkv"
generator = "selftest"
profile = "unit"
container = "matroska"
video_codec = "h264"
width = 64
height = 36
fps = 24
audio_codecs = ["aac"]
audio_channels = [2]
audio_languages = ["jpn"]
subtitle_stream_count = 1
min_duration_seconds = 1
valid_video = true
{extra}"#
    );
    toml::from_str::<support::ExpectationsFile>(&text)
        .unwrap()
        .fixtures
        .remove(0)
}

fn analysis() -> serde_json::Value {
    json!({
        "container_format": "matroska",
        "video_codec": "h264",
        "video_width": 64,
        "video_height": 36,
        "video_frame_rate": "23.976",
        "duration_seconds": 2.0,
        "video_bit_depth": 8,
        "video_hdr_format": null,
        "audio_streams": [{"codec": "aac", "channels": 2, "language": "jpn"}],
        "details": {
            "streams": [
                {"kind": "video", "codec": "h264", "metadata": {"color": {"transfer": 1, "mastering_display": null}}},
                {"kind": "audio", "codec": "aac", "language": "jpn"},
                {"kind": "subtitle", "codec": "subrip", "language": "und"}
            ],
            "chapters": [{"title": "One"}],
            "attachments": [],
            "programs": [],
            "caption_services": [],
            "report": {"budget_exhausted": false, "warnings": [{"code": "w1"}], "bytes_read": 10, "seeks": 1}
        }
    })
}

fn keys(f: &Fixture, a: serde_json::Value) -> Vec<String> {
    support::compare(
        f,
        &Observed {
            analysis: Ok(a),
            diagnostic_status: Some("complete".into()),
        },
    )
    .into_iter()
    .map(|m| m.key)
    .collect()
}

#[test]
fn compare_accepts_a_matching_analysis() {
    let f = fixture(
        "chapter_titles = [\"One\"]\nwarning_codes = [\"w1\"]\ndiagnostic_status = \"complete\"\n\
         present = [\"video.metadata.color.transfer\"]\nabsent = [\"video.metadata.color.mastering_display\"]\n\
         [fixtures.checks]\n\"video.metadata.color.transfer\" = 1\n\"subtitle.0.codec\" = \"subrip\"\n",
    );
    assert_eq!(keys(&f, analysis()), Vec::<String>::new());
}

#[test]
fn compare_reports_each_disagreement_by_a_stable_key() {
    let f = fixture(
        "warning_codes = [\"w2\"]\npresent = [\"chapter.3.title\"]\nabsent = [\"chapter.0.title\"]\n\
         [fixtures.checks]\n\"video.metadata.color.transfer\" = 16\n",
    );
    let mut a = analysis();
    a["video_codec"] = json!("hevc");
    a["video_frame_rate"] = json!("30");
    a["audio_streams"][0]["channels"] = json!(6);
    a["audio_streams"][0]["language"] = json!("und");
    assert_eq!(
        keys(&f, a),
        [
            "video_codec",
            "fps",
            "audio.0.channels",
            "audio_languages",
            "warning:w2",
            "present:chapter.3.title",
            "absent:chapter.0.title",
            "video.metadata.color.transfer",
        ]
    );
}

#[test]
fn compare_handles_errors_none_scrambled_and_exhausted_budgets() {
    let f = fixture("");
    let err = Observed {
        analysis: Err("boom".into()),
        diagnostic_status: None,
    };
    assert_eq!(support::compare(&f, &err)[0].key, "error");

    let mut none = fixture("");
    none.container = "none".into();
    assert!(support::compare(&none, &err).is_empty());
    assert_eq!(keys(&none, analysis()), ["container"]);

    let mut scrambled = fixture("");
    scrambled.payload_scrambled = true;
    scrambled.video_codec = None;
    let mut a = analysis();
    a["video_codec"] = json!(null);
    a["duration_seconds"] = json!(null);
    a["audio_streams"][0]["language"] = json!(null);
    assert_eq!(keys(&scrambled, a.clone()), Vec::<String>::new());

    let mut budget = fixture("may_exhaust_budget = true\n");
    a["details"]["report"]["budget_exhausted"] = json!(true);
    budget.width = 999;
    assert_eq!(keys(&budget, a), Vec::<String>::new());
}

#[test]
fn numbers_compare_by_value_with_a_float_tolerance() {
    assert!(support::values_match(&json!(16), &json!(16)));
    assert!(support::values_match(&json!(16.0), &json!(16)));
    assert!(support::values_match(&json!(0.0050000001), &json!(0.005)));
    assert!(!support::values_match(&json!(0.0051), &json!(0.005)));
    assert!(!support::values_match(&json!(true), &json!(1)));
    assert!(!support::values_match(&json!(null), &json!(0)));
}

#[test]
fn ratchet_fails_new_mismatches_and_fixed_gaps() {
    let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
    let mut known = support::KnownGaps::new();
    known.insert("a.mkv".into(), set(&["fps"]));
    known.insert("b.mkv".into(), set(&["video_codec"]));
    let mut observed = support::KnownGaps::new();
    observed.insert("a.mkv".into(), set(&["fps", "width"]));
    let checked = set(&["a.mkv", "b.mkv"]);
    let problems = support::ratchet(&observed, &known, &checked, &checked);
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(problems[0].contains("new mismatches") && problems[0].contains("width"));
    assert!(problems[1].contains("now passes"));
    assert!(support::ratchet(&known, &known, &checked, &checked).is_empty());
    assert_eq!(
        support::ratchet(&known, &known, &set(&[]), &set(&[])).len(),
        2
    );
}

#[test]
fn mutation_cases_are_deterministic_and_complete() {
    let input: Vec<u8> = (0..10_000u32).map(|i| (i * 31 % 251) as u8).collect();
    let a = support::mutation_cases(&input);
    assert_eq!(a, support::mutation_cases(&input));
    assert_eq!(a.len(), 12 + 8 + 16);
    for (i, case) in a[20..].iter().enumerate() {
        let flipped: u32 = case
            .bytes
            .iter()
            .zip(&input)
            .map(|(x, y)| (x ^ y).count_ones())
            .sum();
        let requested = 1 + (i as u32 % 4) * 8;
        // Two picks of the same bit cancel, so at most the requested count.
        assert!(
            flipped <= requested && flipped % 2 == requested % 2,
            "case {i}"
        );
        if i % 2 == 0 {
            assert_eq!(
                case.bytes[support::FLIP_HEAD..],
                input[support::FLIP_HEAD..],
                "case {i} left the head"
            );
        }
    }
    assert_eq!(a[0].bytes.len(), 100);
    assert_eq!(a[11].bytes.len(), 9_900);
    assert_eq!(a[19].bytes.len(), 512);
    // Short inputs drop the head truncations they cannot express.
    assert_eq!(support::mutation_cases(&[1, 2, 3]).len(), 12 + 1 + 16);
    assert_eq!(support::mutation_cases(&[]).len(), 12);
}
