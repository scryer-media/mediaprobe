//! Every fixture of the hydrated corpus analyses as `tests/media-fixtures.toml`
//! says, apart from the parser gaps `tests/media-known-gaps.toml` records.
//!
//! Needs a hydrated corpus:
//! `cargo xtask test-corpus hydrate`. `MEDIAPROBE_FIXTURE_PROFILE` picks the
//! profile (default `unit`).
#![cfg(feature = "probe")]

mod support;

use std::collections::BTreeSet;
use support::{KnownGaps, shim};

#[test]
fn corpus_matches_expectations_apart_from_known_gaps() {
    let fixtures = support::required_fixtures();
    let mut observed = KnownGaps::new();
    let mut checked = BTreeSet::new();
    let mut details = Vec::new();
    for (fixture, path) in &fixtures {
        checked.insert(fixture.name.clone());
        let mismatches = support::compare(fixture, &shim::observe_file(path));
        if mismatches.is_empty() {
            continue;
        }
        for mismatch in &mismatches {
            details.push(format!("  {}: {mismatch}", fixture.name));
        }
        observed.insert(
            fixture.name.clone(),
            mismatches.into_iter().map(|m| m.key).collect(),
        );
    }

    let known = support::load_known_gaps();
    if std::env::var(support::BLESS_ENV).as_deref() == Ok("1") {
        // Keep entries for fixtures this run did not check (another profile).
        let mut blessed: KnownGaps = known
            .into_iter()
            .filter(|(name, _)| !checked.contains(name))
            .collect();
        blessed.extend(observed);
        std::fs::write(
            support::repo_path(support::KNOWN_GAPS),
            support::render_known_gaps(&blessed),
        )
        .expect("writing the known-gaps file");
        eprintln!("rewrote {}; review the diff", support::KNOWN_GAPS);
        return;
    }

    let all: BTreeSet<String> = support::load_expectations()
        .into_iter()
        .map(|f| f.name)
        .collect();
    let problems = support::ratchet(&observed, &known, &checked, &all);
    assert!(
        problems.is_empty(),
        "{} of {} fixtures disagree with the known-gaps ratchet:\n{}\n\nAll mismatches:\n{}",
        problems.len(),
        checked.len(),
        problems.join("\n"),
        details.join("\n"),
    );
}
