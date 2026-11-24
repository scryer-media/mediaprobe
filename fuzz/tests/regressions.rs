//! Every literal in `src/regressions.rs` replays through the target that
//! found it: cleanly once fixed, and still failing while marked open.

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
        match (outcome.is_err(), r.open) {
            (true, false) => failed.push(format!("{}: {}", r.name, r.finding)),
            (false, true) => failed.push(format!(
                "{}: replays cleanly now; set `open: false`",
                r.name
            )),
            _ => {}
        }
    }
    assert!(
        failed.is_empty(),
        "{} regressions are not in their recorded state:\n{}",
        failed.len(),
        failed.join("\n")
    );
}
