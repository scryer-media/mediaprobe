//! No fixture makes the parser read past its byte budget or its I/O-operation
//! budget, whatever the profile, and the cost of a probe does not grow with
//! the file's length. Run with `MEDIAPROBE_FIXTURE_PROFILE=large` for the
//! budget fixtures (hundreds of MiB to over 4 GiB each).
#![cfg(feature = "probe")]

mod support;

use serde_json::Value;
use support::shim;

#[test]
fn every_fixture_stays_inside_the_read_budgets() {
    let mut failures = Vec::new();
    let mut checked = 0usize;
    for (fixture, path) in support::required_fixtures() {
        if fixture.is_disc_image() {
            // Disc images are analysed only through the path API, which
            // reads each title through a bounded source of its own.
            continue;
        }
        let Ok(analysis) = shim::analyze_file(&path, &fixture.extension()) else {
            continue;
        };
        checked += 1;
        let report = analysis
            .pointer("/details/report")
            .cloned()
            .unwrap_or(Value::Null);
        let read = report.get("bytes_read").and_then(Value::as_u64);
        let seeks = report.get("seeks").and_then(Value::as_u64);
        match (read, seeks) {
            (Some(read), Some(seeks)) => {
                if read > support::BYTE_BUDGET {
                    failures.push(format!("{}: read {read} bytes", fixture.name));
                }
                if seeks > support::IO_LIMIT {
                    failures.push(format!("{}: {seeks} seeks", fixture.name));
                }
            }
            _ => failures.push(format!(
                "{}: report has no bytes_read or seeks",
                fixture.name
            )),
        }
    }
    assert!(checked > 0, "no fixture analysed");
    assert!(
        failures.is_empty(),
        "budget overruns:\n{}",
        failures.join("\n")
    );
}

/// Each pair is the same structure at about 100 MiB and past 4 GiB: only the
/// padding between the parts the parser reads differs. Probing either must
/// read the same bytes with the same number of seeks.
const LENGTH_TWINS: &[(&str, &str)] = &[
    ("large/mp4_co64_100m.mp4", "large/mp4_co64_over_4g.mp4"),
    (
        "large/mkv_seekhead_cues_tags_100m.mkv",
        "large/mkv_seekhead_cues_tags_over_4g.mkv",
    ),
];

#[test]
fn probe_cost_does_not_grow_with_length() {
    let fixtures = support::required_fixtures();
    let find = |name: &str| fixtures.iter().find(|(f, _)| f.name == name);
    let mut compared = 0usize;
    for (short, long) in LENGTH_TWINS {
        let (Some((fixture, short_path)), Some((_, long_path))) = (find(short), find(long)) else {
            continue;
        };
        let cost = |path: &std::path::Path| {
            let analysis = shim::analyze_file(path, &fixture.extension())
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let report = analysis
                .pointer("/details/report")
                .cloned()
                .unwrap_or(Value::Null);
            (
                report.get("bytes_read").and_then(Value::as_u64),
                report.get("seeks").and_then(Value::as_u64),
            )
        };
        let (a, b) = (cost(short_path), cost(long_path));
        eprintln!("{short}: {a:?}; {long}: {b:?} (bytes read, seeks)");
        assert!(
            a.0.is_some() && a.1.is_some(),
            "{short}: no cost in the report"
        );
        assert_eq!(a, b, "{long} cost more to probe than {short}");
        compared += 1;
    }
    let large = matches!(support::required_profile().as_str(), "large" | "all");
    assert!(
        !large || compared == LENGTH_TWINS.len(),
        "the large profile is required but only {compared} twin pair(s) were compared"
    );
}
