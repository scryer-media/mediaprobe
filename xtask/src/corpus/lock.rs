//! `test-corpus/lock.json`: the one published revision the repository uses.
//!
//! The lock pins the manifest by BLAKE3 and records where it, its Sigstore
//! bundle and its provenance document live, plus the exact certificate
//! identity the bundle must carry. A lock with no manifest is *unpublished*:
//! hydrate refuses it rather than fall back to anything.

use serde::{Deserialize, Serialize};

use super::{KEY_PREFIX, Result, fail, is_blake3_hex};

pub(crate) const IDENTITY: &str = "https://github.com/scryer-media/mediaprobe/.github/workflows/test-corpus-publish.yml@refs/heads/main";
pub(crate) const ISSUER: &str = "https://token.actions.githubusercontent.com";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Lock {
    pub(crate) schema_version: u32,
    /// The public, unauthenticated download base. Publish writes through the
    /// S3 API; everything reads through this.
    pub(crate) base_url: String,
    pub(crate) certificate_identity: String,
    pub(crate) certificate_oidc_issuer: String,
    pub(crate) manifest: Option<Pinned>,
    pub(crate) signature: Option<Signature>,
    pub(crate) provenance: Option<Pinned>,
    pub(crate) published_from: Option<PublishedFrom>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pinned {
    pub(crate) blake3: String,
    pub(crate) url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Signature {
    pub(crate) bundle_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublishedFrom {
    pub(crate) commit: Option<String>,
    pub(crate) run: Option<String>,
}

pub(crate) fn object_key(blake3: &str) -> String {
    format!("{KEY_PREFIX}/objects/blake3/{blake3}")
}

pub(crate) fn manifest_key(blake3: &str) -> String {
    format!("{KEY_PREFIX}/manifests/blake3/{blake3}.json")
}

pub(crate) fn bundle_key(manifest_blake3: &str) -> String {
    format!("{}.sigstore.json", manifest_key(manifest_blake3))
}

pub(crate) fn provenance_key(manifest_blake3: &str) -> String {
    format!("{KEY_PREFIX}/manifests/blake3/{manifest_blake3}.provenance.json")
}

pub(crate) fn join(base: &str, key: &str) -> String {
    format!("{}/{key}", base.trim_end_matches('/'))
}

impl Lock {
    pub(crate) fn load(path: &std::path::Path) -> Result<Self> {
        let lock: Lock = super::read_json(path)?;
        lock.validate()?;
        Ok(lock)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return fail("lock.json: unsupported schema_version");
        }
        if !self.base_url.starts_with("https://") || self.base_url.ends_with('/') {
            return fail("lock.json: base_url must be an https URL without a trailing slash");
        }
        if self.certificate_identity != IDENTITY || self.certificate_oidc_issuer != ISSUER {
            return fail(
                "lock.json: the signing identity must be this repository's publish workflow on main",
            );
        }
        match (&self.manifest, &self.signature, &self.provenance) {
            (None, None, None) => Ok(()),
            (Some(manifest), Some(signature), Some(provenance)) => {
                if !is_blake3_hex(&manifest.blake3) || !is_blake3_hex(&provenance.blake3) {
                    return fail("lock.json: malformed digest");
                }
                let expected = [
                    (
                        &manifest.url,
                        join(&self.base_url, &manifest_key(&manifest.blake3)),
                    ),
                    (
                        &signature.bundle_url,
                        join(&self.base_url, &bundle_key(&manifest.blake3)),
                    ),
                    (
                        &provenance.url,
                        join(&self.base_url, &provenance_key(&manifest.blake3)),
                    ),
                ];
                for (actual, wanted) in expected {
                    if *actual != wanted {
                        return fail(format!(
                            "lock.json: {actual} is not where the manifest digest puts it ({wanted})"
                        ));
                    }
                }
                Ok(())
            }
            _ => fail(
                "lock.json: manifest, signature and provenance are pinned together or not at all",
            ),
        }
    }

    pub(crate) fn is_unpublished(&self) -> bool {
        self.manifest.is_none()
    }

    pub(crate) fn published(
        base_url: &str,
        manifest_blake3: &str,
        provenance_blake3: &str,
        published_from: PublishedFrom,
    ) -> Lock {
        Lock {
            schema_version: 1,
            base_url: base_url.trim_end_matches('/').to_owned(),
            certificate_identity: IDENTITY.to_owned(),
            certificate_oidc_issuer: ISSUER.to_owned(),
            manifest: Some(Pinned {
                blake3: manifest_blake3.to_owned(),
                url: join(base_url, &manifest_key(manifest_blake3)),
            }),
            signature: Some(Signature {
                bundle_url: join(base_url, &bundle_key(manifest_blake3)),
            }),
            provenance: Some(Pinned {
                blake3: provenance_blake3.to_owned(),
                url: join(base_url, &provenance_key(manifest_blake3)),
            }),
            published_from: Some(published_from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_lock_is_valid() {
        Lock::load(&super::super::repo_path(super::super::LOCK)).unwrap();
    }

    #[test]
    fn a_published_lock_round_trips_and_pins_its_urls() {
        let digest = "ab".repeat(32);
        let lock = Lock::published(
            "https://cdn.example",
            &digest,
            &"cd".repeat(32),
            PublishedFrom {
                commit: None,
                run: None,
            },
        );
        lock.validate().unwrap();
        assert!(!lock.is_unpublished());
        assert_eq!(
            lock.manifest.as_ref().unwrap().url,
            format!("https://cdn.example/test-corpus/manifests/blake3/{digest}.json")
        );
        let mut moved = lock.clone();
        moved.manifest.as_mut().unwrap().url = "https://elsewhere.example/m.json".into();
        assert!(moved.validate().is_err());
        let mut partial = lock.clone();
        partial.signature = None;
        assert!(partial.validate().is_err());
        let mut foreign = lock;
        foreign.certificate_identity = foreign.certificate_identity.replace("main", "feature");
        assert!(foreign.validate().is_err());
    }

    #[test]
    fn plain_http_bases_are_refused() {
        let mut lock = Lock::published(
            "https://cdn.example",
            &"ab".repeat(32),
            &"cd".repeat(32),
            PublishedFrom {
                commit: None,
                run: None,
            },
        );
        lock.base_url = "http://cdn.example".into();
        assert!(lock.validate().is_err());
    }
}
