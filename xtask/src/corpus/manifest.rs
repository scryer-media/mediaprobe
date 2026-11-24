//! The manifest: the canonical, content-addressed description of one corpus
//! revision, and the provenance document published beside it.
//!
//! The manifest is a pure function of the ledger, the profiles and the
//! toolchain file: the same inputs always produce the same bytes and so the
//! same BLAKE3, which is its key. Run-specific facts (commit, run, tool
//! versions, time) live in the provenance document instead, so they never
//! perturb the manifest's digest.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::ledger::{Ledger, Source};
use super::profiles::ProfilesFile;
use super::{Result, blake3_bytes, canonical_json, fail};

pub(crate) const CORPUS: &str = "mediaprobe";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub(crate) schema_version: u32,
    pub(crate) corpus: String,
    pub(crate) toolchain_blake3: String,
    pub(crate) generators: BTreeMap<String, String>,
    pub(crate) profiles: BTreeMap<String, Vec<String>>,
    pub(crate) files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestFile {
    pub(crate) path: String,
    pub(crate) size: u64,
    pub(crate) blake3: String,
    pub(crate) format: String,
    pub(crate) generator: String,
}

impl Manifest {
    /// Every generated ledger entry must carry a digest: a manifest describes
    /// a whole revision, never part of one.
    pub(crate) fn build(
        ledger: &Ledger,
        profiles: &ProfilesFile,
        toolchain_bytes: &[u8],
    ) -> Result<Self> {
        let mut files = Vec::new();
        for entry in &ledger.files {
            let Source::Generated { generator } = &entry.source else {
                continue;
            };
            let (Some(size), Some(digest)) = (entry.size, &entry.blake3) else {
                return fail(format!(
                    "{} has no digest: generate it before building a manifest",
                    entry.path
                ));
            };
            files.push(ManifestFile {
                path: entry.path.clone(),
                size,
                blake3: digest.clone(),
                format: entry.format.clone(),
                generator: generator.clone(),
            });
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let mut selected = BTreeMap::new();
        for name in profiles.profiles.keys() {
            let mut paths: Vec<String> = profiles
                .select(name, ledger)?
                .into_iter()
                .map(str::to_owned)
                .collect();
            paths.sort();
            selected.insert(name.clone(), paths);
        }
        Ok(Manifest {
            schema_version: 1,
            corpus: CORPUS.to_owned(),
            toolchain_blake3: blake3_bytes(toolchain_bytes),
            generators: ledger
                .generators
                .iter()
                .map(|(k, v)| (k.clone(), v.path.clone()))
                .collect(),
            profiles: selected,
            files,
        })
    }

    pub(crate) fn canonical_bytes(&self) -> Result<Vec<u8>> {
        canonical_json(self)
    }

    /// Parse and check internal consistency: right corpus, sorted unique
    /// paths, well-formed digests, every profile member a listed file.
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        let manifest: Manifest = serde_json::from_slice(bytes)
            .map_err(|e| super::error(format!("manifest does not parse: {e}")))?;
        if manifest.schema_version != 1 || manifest.corpus != CORPUS {
            return fail("manifest is for another corpus or schema");
        }
        let mut previous: Option<&str> = None;
        for file in &manifest.files {
            if !super::valid_repo_relative(&file.path) || !super::is_blake3_hex(&file.blake3) {
                return fail(format!("manifest entry {} is malformed", file.path));
            }
            if previous.is_some_and(|p| p >= file.path.as_str()) {
                return fail("manifest paths are not sorted and unique");
            }
            previous = Some(&file.path);
        }
        for (name, members) in &manifest.profiles {
            for member in members {
                if manifest.file(member).is_none() {
                    return fail(format!(
                        "profile {name} names {member}, which the manifest does not list"
                    ));
                }
            }
        }
        Ok(manifest)
    }

    pub(crate) fn file(&self, path: &str) -> Option<&ManifestFile> {
        self.files
            .binary_search_by(|f| f.path.as_str().cmp(path))
            .ok()
            .map(|i| &self.files[i])
    }

    pub(crate) fn select(&self, profiles: &[String]) -> Result<Vec<&ManifestFile>> {
        let mut paths = std::collections::BTreeSet::new();
        for name in profiles {
            let Some(members) = self.profiles.get(name) else {
                return fail(format!("the pinned manifest has no profile {name}"));
            };
            paths.extend(members.iter().map(String::as_str));
        }
        Ok(paths.into_iter().filter_map(|p| self.file(p)).collect())
    }

    pub(crate) fn blake3(&self) -> Result<String> {
        Ok(blake3_bytes(&self.canonical_bytes()?))
    }
}

/// Facts about the run that produced a revision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Provenance {
    pub(crate) schema_version: u32,
    pub(crate) manifest_blake3: String,
    pub(crate) generated_at: String,
    /// `github-actions` or `local`.
    pub(crate) builder: String,
    pub(crate) repository: Option<String>,
    pub(crate) commit: Option<String>,
    pub(crate) git_ref: Option<String>,
    pub(crate) run_url: Option<String>,
    pub(crate) runner: Option<String>,
    /// First `-version` line of every tool the recipes ran.
    pub(crate) tools: BTreeMap<String, String>,
}

impl Provenance {
    pub(crate) fn from_environment(manifest_blake3: &str, tools: BTreeMap<String, String>) -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let actions = var("GITHUB_ACTIONS").as_deref() == Some("true");
        let run_url = match (
            var("GITHUB_SERVER_URL"),
            var("GITHUB_REPOSITORY"),
            var("GITHUB_RUN_ID"),
        ) {
            (Some(server), Some(repo), Some(run)) if actions => {
                Some(format!("{server}/{repo}/actions/runs/{run}"))
            }
            _ => None,
        };
        Provenance {
            schema_version: 1,
            manifest_blake3: manifest_blake3.to_owned(),
            generated_at: super::utc_now_rfc3339(),
            builder: if actions { "github-actions" } else { "local" }.to_owned(),
            repository: actions.then(|| var("GITHUB_REPOSITORY")).flatten(),
            commit: actions.then(|| var("GITHUB_SHA")).flatten(),
            git_ref: actions.then(|| var("GITHUB_REF")).flatten(),
            run_url,
            runner: actions
                .then(|| {
                    var("RUNNER_OS")
                        .zip(var("RUNNER_ARCH"))
                        .map(|(o, a)| format!("{o}-{a}"))
                })
                .flatten(),
            tools,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipes;

    fn inputs() -> (Ledger, ProfilesFile) {
        let fixtures = recipes::all();
        let mut ledger = Ledger::from_recipes(&fixtures, None);
        for (i, entry) in ledger.files.iter_mut().enumerate() {
            if entry.is_published_shape() {
                entry.size = Some(i as u64 + 1);
                entry.blake3 = Some(blake3_bytes(entry.path.as_bytes()));
            }
        }
        let profiles =
            ProfilesFile::load(&super::super::repo_path(super::super::PROFILES)).unwrap();
        (ledger, profiles)
    }

    #[test]
    fn manifest_is_canonical_and_a_pure_function_of_its_inputs() {
        let (ledger, profiles) = inputs();
        let a = Manifest::build(&ledger, &profiles, b"toolchain").unwrap();
        let b = Manifest::build(&ledger, &profiles, b"toolchain").unwrap();
        assert_eq!(a.canonical_bytes().unwrap(), b.canonical_bytes().unwrap());
        let parsed = Manifest::parse(&a.canonical_bytes().unwrap()).unwrap();
        assert_eq!(parsed, a);
        let c = Manifest::build(&ledger, &profiles, b"other toolchain").unwrap();
        assert_ne!(a.blake3().unwrap(), c.blake3().unwrap());
        assert!(a.files.iter().all(|f| !f.path.contains("/stubs/")));
        assert!(!a.select(&["large".into()]).unwrap().is_empty());
    }

    #[test]
    fn a_revision_with_a_missing_digest_does_not_build() {
        let (mut ledger, profiles) = inputs();
        ledger.files[0].blake3 = None;
        ledger.files[0].size = None;
        assert!(Manifest::build(&ledger, &profiles, b"t").is_err());
    }

    #[test]
    fn parse_rejects_foreign_or_inconsistent_manifests() {
        let (ledger, profiles) = inputs();
        let manifest = Manifest::build(&ledger, &profiles, b"t").unwrap();
        let mut foreign = manifest.clone();
        foreign.corpus = "rarpar".into();
        assert!(Manifest::parse(&foreign.canonical_bytes().unwrap()).is_err());
        let mut unsorted = manifest.clone();
        unsorted.files.swap(0, 1);
        assert!(Manifest::parse(&unsorted.canonical_bytes().unwrap()).is_err());
        let mut dangling = manifest;
        dangling
            .profiles
            .insert("x".into(), vec!["tests/media/nope.mkv".into()]);
        assert!(Manifest::parse(&dangling.canonical_bytes().unwrap()).is_err());
    }

    #[test]
    fn provenance_outside_actions_says_local() {
        if std::env::var("GITHUB_ACTIONS").is_ok() {
            return;
        }
        let p = Provenance::from_environment("ab", BTreeMap::new());
        assert_eq!(p.builder, "local");
        assert_eq!(p.commit, None);
    }
}
