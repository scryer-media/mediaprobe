//! One parser per container format. Each reads through a bounded
//! [`MediaSource`](crate::source::MediaSource) and returns a
//! [`RawContainer`](crate::raw::RawContainer).

pub(crate) mod asf;
pub(crate) mod audio_only;
pub(crate) mod avi;
pub(crate) mod dv;
pub(crate) mod elementary;
pub(crate) mod flv;
pub(crate) mod mkv;
pub(crate) mod mp4;
pub(crate) mod ogg;
pub(crate) mod ps;
pub(crate) mod realmedia;
pub(crate) mod ts;

#[cfg(test)]
mod read_budget_tests;

/// Tracks a container parser reports at most. Real files carry a few dozen;
/// a header that declares more is cut here, with the probe marked
/// incomplete, so a small header cannot make the probe build and enrich
/// hundreds of thousands of tracks.
pub(crate) const MAX_TRACKS: usize = 4096;
