//! Where a revision is read from and written to.
//!
//! Reads go through the public, unauthenticated base URL (or, for local
//! tests and offline mirrors, a directory laid out with the same keys).
//! Writes go through R2's S3-compatible API with SigV4 (or a directory).
//! Every write is create-only: an existing key is never overwritten.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use super::http::{Body, Client, MAX_DOCUMENT, Policy, PutOutcome};
use super::sigv4::{self, Credentials, Signable};
use super::{Context as _, Result, fail};

pub(crate) enum Source {
    Http { client: Client, base_url: String },
    Dir { root: PathBuf },
}

impl Source {
    pub(crate) fn http(base_url: &str) -> Self {
        Source::Http {
            client: Client::new(Policy::production()),
            base_url: base_url.trim_end_matches('/').to_owned(),
        }
    }

    pub(crate) fn describe(&self) -> String {
        match self {
            Source::Http { base_url, .. } => base_url.clone(),
            Source::Dir { root } => root.display().to_string(),
        }
    }

    /// A small document (manifest, bundle, provenance). `None` if absent.
    /// One longer than [`MAX_DOCUMENT`] fails before it is read whole: its
    /// digest is checked only after it is in memory.
    pub(crate) fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        match self {
            Source::Http { client, base_url } => {
                client.get_bytes(&super::lock::join(base_url, key))
            }
            Source::Dir { root } => {
                let path = root.join(key);
                if !path.is_file() {
                    return Ok(None);
                }
                let mut bytes = Vec::new();
                std::fs::File::open(&path)
                    .and_then(|file| file.take(MAX_DOCUMENT + 1).read_to_end(&mut bytes))
                    .context(|| format!("read {}", path.display()))?;
                if bytes.len() as u64 > MAX_DOCUMENT {
                    return fail(format!(
                        "{}: longer than the {MAX_DOCUMENT} bytes a document may have",
                        path.display()
                    ));
                }
                Ok(Some(bytes))
            }
        }
    }

    /// One object, landed only if its size and BLAKE3 match.
    pub(crate) fn fetch(
        &self,
        key: &str,
        destination: &Path,
        blake3: &str,
        size: u64,
    ) -> Result<()> {
        match self {
            Source::Http { client, base_url } => {
                client.get_verified(&super::lock::join(base_url, key), destination, blake3, size)
            }
            Source::Dir { root } => {
                let source = root.join(key);
                let (actual, actual_size) = super::digest_file(&source)?;
                if actual != blake3 || actual_size != size {
                    return fail(format!(
                        "{}: does not match the digest the manifest pins",
                        source.display()
                    ));
                }
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent)
                        .context(|| format!("create {}", parent.display()))?;
                }
                let temporary = super::temporary_sibling(destination);
                std::fs::copy(&source, &temporary)
                    .context(|| format!("copy {}", source.display()))?;
                // Re-check what landed: the source could change under us.
                let (landed, landed_size) = super::digest_file(&temporary)?;
                if landed != blake3 || landed_size != size {
                    let _ = std::fs::remove_file(&temporary);
                    return fail(format!("{}: changed while it was copied", source.display()));
                }
                super::rename_into_place(&temporary, destination)
            }
        }
    }
}

pub(crate) enum Sink {
    R2(R2),
    Dir { root: PathBuf },
}

pub(crate) struct R2 {
    client: Client,
    endpoint: String,
    host: String,
    bucket: String,
    credentials: Credentials,
}

impl R2 {
    pub(crate) fn new(
        client: Client,
        endpoint: &str,
        bucket: &str,
        credentials: Credentials,
    ) -> Result<Self> {
        let endpoint = endpoint.trim_end_matches('/').to_owned();
        client.check_url(&endpoint)?;
        let authority = endpoint
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or_default();
        if authority.is_empty() || authority.contains('/') || authority.contains('@') {
            return fail(
                "the R2 endpoint must be a bare scheme and host, with no path or user info",
            );
        }
        let valid_bucket = (3..=63).contains(&bucket.len())
            && bucket
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !valid_bucket {
            return fail(format!("not a valid bucket name: {bucket:?}"));
        }
        if credentials.access_key_id.is_empty() || credentials.secret_access_key.is_empty() {
            return fail("R2 credentials are empty");
        }
        Ok(R2 {
            client,
            host: authority.to_owned(),
            endpoint,
            bucket: bucket.to_owned(),
            credentials,
        })
    }

    /// Signed, conditional, path-style PUT. The payload hash is signed, so
    /// R2 rejects a body that differs from the bytes hashed here.
    fn put(&self, key: &str, body: Body<'_>, content_type: &str, now: u64) -> Result<PutOutcome> {
        let payload_sha256 = match &body {
            Body::Bytes(bytes) => sigv4::sha256_hex(bytes),
            Body::File(path) => sigv4::sha256_file(path)?,
        };
        let stamp = sigv4::timestamp(now);
        let path = format!("/{}/{key}", self.bucket);
        let mut headers = vec![
            ("content-type".to_owned(), content_type.to_owned()),
            ("host".to_owned(), self.host.clone()),
            ("if-none-match".to_owned(), "*".to_owned()),
            ("x-amz-content-sha256".to_owned(), payload_sha256.clone()),
            ("x-amz-date".to_owned(), stamp.clone()),
        ];
        let authorization = Signable {
            method: "PUT",
            path: &path,
            query: "",
            headers: &headers,
            payload_sha256: &payload_sha256,
            timestamp: &stamp,
            region: sigv4::REGION,
            service: sigv4::SERVICE,
        }
        .authorization(&self.credentials)?;
        // The HTTP client derives Host from the URL, the same authority signed above.
        headers.retain(|(name, _)| name != "host");
        headers.push(("authorization".to_owned(), authorization));
        self.client
            .put_if_absent(&format!("{}{path}", self.endpoint), &headers, body)
    }
}

impl Sink {
    pub(crate) fn describe(&self) -> String {
        match self {
            Sink::R2(r2) => format!("R2 bucket {}", r2.bucket),
            Sink::Dir { root } => root.display().to_string(),
        }
    }

    pub(crate) fn put(&self, key: &str, body: Body<'_>, content_type: &str) -> Result<PutOutcome> {
        match self {
            Sink::R2(r2) => r2.put(key, body, content_type, super::utc_now_seconds()),
            Sink::Dir { root } => {
                let destination = root.join(key);
                if destination.exists() {
                    return Ok(PutOutcome::AlreadyExists);
                }
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent)
                        .context(|| format!("create {}", parent.display()))?;
                }
                let temporary = super::temporary_sibling(&destination);
                match body {
                    Body::Bytes(bytes) => std::fs::write(&temporary, bytes)
                        .context(|| format!("write {}", temporary.display()))?,
                    Body::File(path) => {
                        std::fs::copy(path, &temporary)
                            .context(|| format!("copy {}", path.display()))?;
                    }
                }
                // hard_link fails if the key appeared meanwhile: create-only, like R2.
                let linked = std::fs::hard_link(&temporary, &destination);
                let _ = std::fs::remove_file(&temporary);
                match linked {
                    Ok(()) => Ok(PutOutcome::Created),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        Ok(PutOutcome::AlreadyExists)
                    }
                    Err(e) => fail(format!("publish {}: {e}", destination.display())),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::http::fake::serve;
    use super::*;

    fn credentials() -> Credentials {
        Credentials {
            access_key_id: "test-id".into(),
            secret_access_key: "test-secret".into(),
        }
    }

    #[test]
    fn r2_put_is_signed_path_style_and_conditional() {
        let server = serve(Box::new(|_| (200, vec![], vec![])));
        let r2 = R2::new(
            Client::new(Policy::loopback_for_tests()),
            &server.base,
            "corpus-bucket",
            credentials(),
        )
        .unwrap();
        let key = "test-corpus/objects/blake3/ab";
        assert_eq!(
            r2.put(
                key,
                Body::Bytes(b"payload"),
                "application/octet-stream",
                1_440_938_160
            )
            .unwrap(),
            PutOutcome::Created
        );
        let recorded = server.requests.lock().unwrap();
        let request = &recorded[0];
        assert_eq!(request.method, "PUT");
        assert_eq!(request.path, format!("/corpus-bucket/{key}"));
        let header = |name: &str| {
            request
                .headers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(header("if-none-match").as_deref(), Some("*"));
        assert_eq!(header("x-amz-date").as_deref(), Some("20150830T123600Z"));
        assert_eq!(
            header("x-amz-content-sha256"),
            Some(sigv4::sha256_hex(b"payload"))
        );
        assert_eq!(
            header("host"),
            Some(server.base.trim_start_matches("http://").to_owned())
        );
        let authorization = header("authorization").unwrap();
        assert!(
            authorization
                .starts_with("AWS4-HMAC-SHA256 Credential=test-id/20150830/auto/s3/aws4_request")
        );
        assert!(authorization.contains(
            "SignedHeaders=content-type;host;if-none-match;x-amz-content-sha256;x-amz-date"
        ));
        assert!(!authorization.contains("test-secret"));
        assert_eq!(request.body, b"payload");
    }

    #[test]
    fn r2_configuration_is_checked_before_any_request() {
        let client = || Client::new(Policy::production());
        assert!(
            R2::new(
                client(),
                "http://r2.example",
                "corpus-bucket",
                credentials()
            )
            .is_err()
        );
        assert!(
            R2::new(
                client(),
                "https://r2.example/path",
                "corpus-bucket",
                credentials()
            )
            .is_err()
        );
        assert!(R2::new(client(), "https://r2.example", "Bad_Bucket", credentials()).is_err());
        let empty = Credentials {
            access_key_id: String::new(),
            secret_access_key: String::new(),
        };
        assert!(R2::new(client(), "https://r2.example", "corpus-bucket", empty).is_err());
        assert!(
            R2::new(
                client(),
                "https://r2.example/",
                "corpus-bucket",
                credentials()
            )
            .is_ok()
        );
    }

    #[test]
    fn directory_sink_is_create_only_and_source_checks_digests() {
        let root =
            std::env::temp_dir().join(format!("mediaprobe-xtask-store-{}", std::process::id()));
        let sink = Sink::Dir { root: root.clone() };
        assert_eq!(
            sink.put("a/b", Body::Bytes(b"one"), "x").unwrap(),
            PutOutcome::Created
        );
        assert_eq!(
            sink.put("a/b", Body::Bytes(b"two"), "x").unwrap(),
            PutOutcome::AlreadyExists
        );
        assert_eq!(std::fs::read(root.join("a/b")).unwrap(), b"one");
        let source = Source::Dir { root: root.clone() };
        assert_eq!(source.get("a/b").unwrap(), Some(b"one".to_vec()));
        assert_eq!(source.get("a/missing").unwrap(), None);
        // A sparse file far past the cap: refused after reading one byte past
        // the cap, never read whole.
        let oversized = std::fs::File::create(root.join("a/oversized")).unwrap();
        oversized.set_len(MAX_DOCUMENT * 64).unwrap();
        drop(oversized);
        let refused = source.get("a/oversized").unwrap_err().to_string();
        assert!(refused.contains("longer than"), "{refused}");
        let out = root.join("out/x");
        source
            .fetch("a/b", &out, &super::super::blake3_bytes(b"one"), 3)
            .unwrap();
        assert!(
            source
                .fetch(
                    "a/b",
                    &root.join("out/y"),
                    &super::super::blake3_bytes(b"two"),
                    3
                )
                .is_err()
        );
        assert!(!root.join("out/y").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
