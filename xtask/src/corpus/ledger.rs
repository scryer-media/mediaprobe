//! `test-corpus/sources.json`: the provenance ledger. One entry per fixture
//! path, naming the generator and recipe that wrote it, its format, and — once
//! a revision containing it has been published — its size and BLAKE3.
//!
//! The ledger's shape (paths, generators, formats, profiles) is rendered from
//! the recipe table; only the size and digest come from a generation run. A
//! path with no digest yet is *pending first publication*.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Result, fail, is_blake3_hex, valid_repo_relative};
use crate::recipes::{self, Fixture, Generator, NOT_REPRODUCIBLE};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Ledger {
    pub(crate) schema_version: u32,
    pub(crate) toolchain: String,
    pub(crate) generators: BTreeMap<String, GeneratorEntry>,
    pub(crate) files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct GeneratorEntry {
    pub(crate) path: String,
    pub(crate) toolchains: Vec<String>,
    pub(crate) byte_reproducible: bool,
    pub(crate) notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileEntry {
    pub(crate) path: String,
    pub(crate) format: String,
    pub(crate) profile: String,
    pub(crate) size: Option<u64>,
    pub(crate) blake3: Option<String>,
    pub(crate) source: Source,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Source {
    Generated {
        generator: String,
    },
    /// Recorded so the gap is visible; never generated, never published.
    NotReproducible {
        reason: String,
    },
    /// Written at hydrate time from `seed`, a published entry; never
    /// published itself. Size and digest are pinned by the recipe.
    Synthesized {
        generator: String,
        seed: String,
    },
}

impl FileEntry {
    pub(crate) fn is_published_shape(&self) -> bool {
        matches!(self.source, Source::Generated { .. })
    }

    pub(crate) fn is_synthesized(&self) -> bool {
        matches!(self.source, Source::Synthesized { .. })
    }

    pub(crate) fn pending(&self) -> bool {
        self.is_published_shape() && self.blake3.is_none()
    }
}

impl Ledger {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let ledger: Self = super::read_json(path)?;
        ledger.validate()?;
        Ok(ledger)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return fail(format!(
                "sources.json: unsupported schema_version {}",
                self.schema_version
            ));
        }
        let mut seen = BTreeSet::new();
        for entry in &self.files {
            if !valid_repo_relative(&entry.path) {
                return fail(format!("sources.json: invalid path {}", entry.path));
            }
            if !seen.insert(&entry.path) {
                return fail(format!("sources.json: duplicate path {}", entry.path));
            }
            if let Some(digest) = &entry.blake3
                && !is_blake3_hex(digest)
            {
                return fail(format!(
                    "sources.json: {} has a malformed digest",
                    entry.path
                ));
            }
            if entry.blake3.is_some() != entry.size.is_some() {
                return fail(format!(
                    "sources.json: {} has a digest without a size or the reverse",
                    entry.path
                ));
            }
            match &entry.source {
                Source::Generated { generator } if !self.generators.contains_key(generator) => {
                    return fail(format!(
                        "sources.json: {} names unknown generator {generator}",
                        entry.path
                    ));
                }
                Source::Synthesized { generator, .. }
                    if !self.generators.contains_key(generator) =>
                {
                    return fail(format!(
                        "sources.json: {} names unknown generator {generator}",
                        entry.path
                    ));
                }
                Source::Synthesized { seed, .. }
                    if !self
                        .files
                        .iter()
                        .any(|f| &f.path == seed && f.is_published_shape()) =>
                {
                    return fail(format!(
                        "sources.json: {} seeds from {seed}, which is not a published entry",
                        entry.path
                    ));
                }
                Source::Synthesized { .. } if entry.blake3.is_none() => {
                    return fail(format!(
                        "sources.json: {} is synthesised but pins no digest",
                        entry.path
                    ));
                }
                Source::NotReproducible { .. } if entry.blake3.is_some() => {
                    return fail(format!(
                        "sources.json: {} is not reproducible but has a digest",
                        entry.path
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The ledger the recipe table describes, keeping the sizes and digests
    /// `previous` already recorded for paths that are still present.
    pub(crate) fn from_recipes(fixtures: &[Fixture], previous: Option<&Ledger>) -> Ledger {
        let known: BTreeMap<&str, &FileEntry> = previous
            .map(|ledger| ledger.files.iter().map(|f| (f.path.as_str(), f)).collect())
            .unwrap_or_default();
        let generators = Generator::ALL
            .iter()
            .map(|g| {
                (
                    g.id().to_owned(),
                    GeneratorEntry {
                        path: g.path().to_owned(),
                        toolchains: g.toolchains().iter().map(|t| (*t).to_owned()).collect(),
                        byte_reproducible: false,
                        notes: g.notes().to_owned(),
                    },
                )
            })
            .collect();
        let mut files: Vec<FileEntry> = fixtures
            .iter()
            .map(|fixture| {
                let path = fixture.path();
                if let Some((size, blake3)) = recipes::synth::pinned(&fixture.name) {
                    let seed = match fixture.steps.first() {
                        Some(recipes::Step::Synthesize { seed, .. }) => *seed,
                        _ => "",
                    };
                    return FileEntry {
                        format: fixture.format(),
                        profile: fixture.profile().to_owned(),
                        size: (!blake3.is_empty()).then_some(size),
                        blake3: (!blake3.is_empty()).then(|| blake3.to_owned()),
                        source: Source::Synthesized {
                            generator: fixture.generator.id().to_owned(),
                            seed: format!("{}/{seed}", super::MEDIA_ROOT),
                        },
                        path,
                    };
                }
                let old = known.get(path.as_str());
                FileEntry {
                    format: fixture.format(),
                    profile: fixture.profile().to_owned(),
                    size: old.and_then(|o| o.size),
                    blake3: old.and_then(|o| o.blake3.clone()),
                    source: Source::Generated {
                        generator: fixture.generator.id().to_owned(),
                    },
                    path,
                }
            })
            .collect();
        for blocked in NOT_REPRODUCIBLE {
            let path = format!("{}/{}", super::MEDIA_ROOT, blocked.name);
            files.push(FileEntry {
                format: blocked
                    .name
                    .rsplit('.')
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                profile: "none".to_owned(),
                size: None,
                blake3: None,
                source: Source::NotReproducible {
                    reason: blocked
                        .reason
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                },
                path,
            });
        }
        Ledger {
            schema_version: 1,
            toolchain: super::TOOLCHAIN.to_owned(),
            generators,
            files,
        }
    }

    pub(crate) fn entry_mut(&mut self, path: &str) -> Option<&mut FileEntry> {
        self.files.iter_mut().find(|f| f.path == path)
    }

    /// Every generated entry and its expected digest, for a tree check.
    pub(crate) fn check_tree(
        &self,
        root: &Path,
        all_present: bool,
        require_digests: bool,
    ) -> Vec<String> {
        let mut problems = Vec::new();
        for entry in self
            .files
            .iter()
            .filter(|f| f.is_published_shape() || f.is_synthesized())
        {
            let path = root.join(&entry.path);
            if !path.is_file() {
                if all_present {
                    problems.push(format!("{}: missing", entry.path));
                }
                continue;
            }
            let Some(expected) = &entry.blake3 else {
                if require_digests {
                    problems.push(format!("{}: no recorded digest", entry.path));
                }
                continue;
            };
            match super::digest_file(&path) {
                Ok((digest, size)) => {
                    if &digest != expected || Some(size) != entry.size {
                        problems.push(format!(
                            "{}: digest or size differs from the ledger",
                            entry.path
                        ));
                    }
                }
                Err(error) => problems.push(format!("{}: {error}", entry.path)),
            }
        }
        problems
    }
}

/// The committed ledger must be exactly what the recipe table renders, apart
/// from sizes and digests.
pub(crate) fn shape_drift(committed: &Ledger) -> Option<String> {
    let rendered = Ledger::from_recipes(&recipes::all(), Some(committed));
    (rendered != *committed).then(|| {
        "test-corpus/sources.json does not match the recipes: run `cargo xtask test-corpus ledger`".to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_ledger_matches_the_recipes() {
        let ledger = Ledger::load(&super::super::repo_path(super::super::SOURCES)).unwrap();
        assert_eq!(shape_drift(&ledger), None);
    }

    #[test]
    fn validation_rejects_inconsistent_entries() {
        let fixtures = recipes::all();
        let mut ledger = Ledger::from_recipes(&fixtures[..2], None);
        ledger.validate().unwrap();
        ledger.files[0].blake3 = Some("00".repeat(32));
        assert!(ledger.validate().is_err(), "digest without size");
        ledger.files[0].size = Some(1);
        ledger.validate().unwrap();
        ledger.files[1].path = ledger.files[0].path.clone();
        assert!(ledger.validate().is_err(), "duplicate");
        ledger.files[1].path = "tests/media/../x".into();
        assert!(ledger.validate().is_err(), "escape");
    }

    #[test]
    fn regenerating_keeps_known_digests() {
        let fixtures = recipes::all();
        let mut first = Ledger::from_recipes(&fixtures, None);
        let path = first.files[0].path.clone();
        first.entry_mut(&path).unwrap().size = Some(7);
        first.entry_mut(&path).unwrap().blake3 = Some("ab".repeat(32));
        let second = Ledger::from_recipes(&fixtures, Some(&first));
        assert_eq!(second, first);
        assert!(
            second
                .files
                .iter()
                .any(|f| matches!(f.source, Source::NotReproducible { .. }))
        );
    }
}
