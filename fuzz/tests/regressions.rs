//! Every literal in `src/regressions.rs` replays cleanly through the target
//! that found it.

use mediaprobe_fuzz::regressions::{REGRESSIONS, Target};
use mediaprobe_fuzz::targets;

#[test]
fn every_regression_replays_cleanly() {
    let mut failed = Vec::new();
    for r in REGRESSIONS {
        let outcome = std::panic::catch_unwind(|| match r.target {
            Target::Container(ext) => targets::container(r.input, ext),
            Target::Dispatch => targets::dispatch(r.input),
            Target::CodecConfig => targets::codec_config(r.input),
            Target::Bitstream => targets::bitstream(r.input),
            Target::HdrSei => targets::hdr_sei(r.input),
            Target::DolbyVision => targets::dolby_vision(r.input),
            Target::Disc => targets::disc(r.input),
            Target::SimdDifferential => targets::simd_differential(r.input),
            Target::Budget => targets::budget(r.input),
            Target::ProbeDepths => targets::probe_depths(r.input),
            Target::MkvTags => targets::mkv_tags(r.input),
            Target::ModelSerde => targets::model_serde(r.input),
        });
        if outcome.is_err() {
            failed.push(format!("{}: {}", r.name, r.finding));
        }
    }
    assert!(
        failed.is_empty(),
        "{} regressions fail:\n{}",
        failed.len(),
        failed.join("\n")
    );
}
