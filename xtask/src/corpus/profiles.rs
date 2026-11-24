//! `test-corpus/profiles.json`: named subsets of the corpus that a CI lane or a
//! contributor hydrates, as include and exclude glob patterns over ledger
//! paths.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::ledger::{FileEntry, Ledger};
use super::{Result, fail};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfilesFile {
    pub(crate) schema_version: u32,
    pub(crate) profiles: BTreeMap<String, Profile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    pub(crate) description: String,
    pub(crate) include: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) exclude: Vec<String>,
}

impl ProfilesFile {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let file: Self = super::read_json(path)?;
        if file.schema_version != 1 {
            return fail("profiles.json: unsupported schema_version");
        }
        Ok(file)
    }

    /// The generated ledger paths a profile selects. A profile that selects
    /// nothing is a mistake, not an empty hydrate.
    pub(crate) fn select<'a>(&self, name: &str, ledger: &'a Ledger) -> Result<Vec<&'a str>> {
        let selected: Vec<&str> = self
            .matching(name, ledger, FileEntry::is_published_shape)?
            .into_iter()
            .map(|f| f.path.as_str())
            .collect();
        if selected.is_empty() {
            return fail(format!("profile {name} selects no fixtures"));
        }
        Ok(selected)
    }

    /// The synthesised ledger entries a profile selects: written at hydrate
    /// time, never published, so absent from the manifest. May be empty.
    pub(crate) fn select_synthesized<'a>(
        &self,
        name: &str,
        ledger: &'a Ledger,
    ) -> Result<Vec<&'a FileEntry>> {
        self.matching(name, ledger, FileEntry::is_synthesized)
    }

    fn matching<'a>(
        &self,
        name: &str,
        ledger: &'a Ledger,
        keep: fn(&FileEntry) -> bool,
    ) -> Result<Vec<&'a FileEntry>> {
        let Some(profile) = self.profiles.get(name) else {
            return fail(format!(
                "unknown profile {name}; known: {}",
                self.profiles.keys().cloned().collect::<Vec<_>>().join(", ")
            ));
        };
        Ok(ledger
            .files
            .iter()
            .filter(|f| keep(f))
            .filter(|f| profile.include.iter().any(|p| glob_match(p, &f.path)))
            .filter(|f| !profile.exclude.iter().any(|p| glob_match(p, &f.path)))
            .collect())
    }
}

/// `*` matches within one path component, `**` across components, `?` one
/// character. Nothing else is special.
pub(crate) fn glob_match(pattern: &str, path: &str) -> bool {
    fn go(p: &[u8], s: &[u8]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some(b'*') if p.get(1) == Some(&b'*') => {
                let rest = &p[2..];
                let rest = rest.strip_prefix(b"/").unwrap_or(rest);
                (0..=s.len()).any(|i| go(rest, &s[i..]))
            }
            Some(b'*') => {
                let rest = &p[1..];
                let mut i = 0;
                loop {
                    if go(rest, &s[i..]) {
                        return true;
                    }
                    if i == s.len() || s[i] == b'/' {
                        return false;
                    }
                    i += 1;
                }
            }
            Some(b'?') => !s.is_empty() && s[0] != b'/' && go(&p[1..], &s[1..]),
            Some(c) => s.first() == Some(c) && go(&p[1..], &s[1..]),
        }
    }
    go(pattern.as_bytes(), path.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("tests/media/*.mkv", "tests/media/a.mkv"));
        assert!(!glob_match("tests/media/*.mkv", "tests/media/large/a.mkv"));
        assert!(glob_match("tests/media/**", "tests/media/large/a.mkv"));
        assert!(glob_match("tests/media/**/*.ts", "tests/media/x.ts"));
        assert!(glob_match("tests/media/**/*.ts", "tests/media/large/x.ts"));
        assert!(glob_match(
            "tests/media/dv_profile?.mp4",
            "tests/media/dv_profile8.mp4"
        ));
        assert!(!glob_match(
            "tests/media/dv_profile?.mp4",
            "tests/media/dv_profile10.mp4"
        ));
    }

    #[test]
    fn committed_profiles_each_select_something() {
        let profiles =
            ProfilesFile::load(&super::super::repo_path(super::super::PROFILES)).unwrap();
        let ledger = Ledger::load(&super::super::repo_path(super::super::SOURCES)).unwrap();
        for name in profiles.profiles.keys() {
            profiles.select(name, &ledger).unwrap();
        }
        let unit = profiles.select("unit", &ledger).unwrap();
        assert!(unit.iter().all(|p| !p.starts_with("tests/media/large/")));
        let large = profiles.select("large", &ledger).unwrap();
        assert!(large.iter().all(|p| p.starts_with("tests/media/large/")));
    }
}
