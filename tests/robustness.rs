//! Truncated and bit-flipped copies of every small fixture never panic the
//! parser or the structural diagnosis.
//!
//! Each input yields the cases of `support::mutation_cases`: 12 proportional
//! and up to 8 head truncations, and 16 seeded bit-flip cases. Every case is
//! analysed from memory under `catch_unwind`; an error is fine, a panic is a
//! finding. Needs the parser and a hydrated corpus, like `media_fixtures`.
#![cfg(feature = "probe")]

mod support;

use std::panic::{AssertUnwindSafe, catch_unwind};
use support::shim;

fn panics(bytes: &[u8], extension: &str) -> Option<&'static str> {
    if catch_unwind(AssertUnwindSafe(|| shim::analyze_bytes(bytes, extension))).is_err() {
        return Some("analyze_source");
    }
    if catch_unwind(AssertUnwindSafe(|| shim::diagnose_bytes(bytes, extension))).is_err() {
        return Some("diagnose_source");
    }
    None
}

#[test]
fn truncated_and_bit_flipped_fixtures_never_panic() {
    let mut failures = Vec::new();
    let mut cases = 0usize;
    for (fixture, path) in support::required_fixtures() {
        if fixture.profile == "large" {
            continue;
        }
        let bytes = std::fs::read(&path).expect("reading a fixture");
        let extension = fixture.extension();
        for case in std::iter::once(support::Case {
            label: "unmodified".to_owned(),
            bytes: bytes.clone(),
        })
        .chain(support::mutation_cases(&bytes))
        {
            cases += 1;
            if let Some(entry) = panics(&case.bytes, &extension) {
                failures.push(format!(
                    "{} [{}]: {entry} panicked",
                    fixture.name, case.label
                ));
            }
        }
    }
    assert!(cases > 0);
    assert!(
        failures.is_empty(),
        "{} of {cases} cases panicked:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
