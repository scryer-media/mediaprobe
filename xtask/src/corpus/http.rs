//! The only network client the corpus tooling has.
//!
//! - HTTPS only. Plain HTTP is accepted solely for a loopback address, and
//!   only when the caller builds the client with [`Policy::loopback_for_tests`];
//!   no environment variable can turn it on.
//! - Redirects are never followed: a CDN that redirects is misconfigured, and
//!   following one would let a response choose where the next request goes.
//! - Every body is capped: a fixture at the size its manifest pins (and
//!   never above [`MAX_BODY`]), a document at [`MAX_DOCUMENT`]. A longer body
//!   fails without being retried.
//! - Transport errors, 429 and 5xx are retried a bounded number of times.
//! - A PUT is conditional (`If-None-Match: *`), so an object, once written,
//!   is never overwritten. A 412 means it already exists; the caller decides
//!   whether the existing bytes are the right ones.

use std::io::{Read as _, Write as _};
use std::path::Path;
use std::time::Duration;

use super::{Context as _, Result, fail};

/// No fixture, manifest or bundle is near this size; anything larger is a
/// misbehaving server.
pub(crate) const MAX_BODY: u64 = 1 << 30;
/// Manifests, Sigstore bundles and provenance records are read whole into
/// memory before their digest or signature is checked. The largest is a few
/// hundred KiB; a body past this cap is refused before it is buffered.
pub(crate) const MAX_DOCUMENT: u64 = 16 << 20;
const ATTEMPTS: u32 = 4;

#[derive(Debug, Clone)]
pub(crate) struct Policy {
    allow_loopback_http: bool,
    backoff: Duration,
}

impl Policy {
    pub(crate) fn production() -> Self {
        Policy {
            allow_loopback_http: false,
            backoff: Duration::from_secs(2),
        }
    }

    #[cfg(test)]
    pub(crate) fn loopback_for_tests() -> Self {
        Policy {
            allow_loopback_http: true,
            backoff: Duration::ZERO,
        }
    }
}

pub(crate) enum Body<'a> {
    Bytes(&'a [u8]),
    File(&'a Path),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PutOutcome {
    Created,
    AlreadyExists,
}

pub(crate) struct Client {
    agent: ureq::Agent,
    policy: Policy,
}

enum Attempt<T> {
    Done(T),
    Retry(String),
}

impl Client {
    pub(crate) fn new(policy: Policy) -> Self {
        let config = ureq::Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .https_only(!policy.allow_loopback_http)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(120)))
            .timeout_global(Some(Duration::from_secs(60 * 60)))
            .build();
        Client {
            agent: ureq::Agent::new_with_config(config),
            policy,
        }
    }

    pub(crate) fn check_url(&self, url: &str) -> Result<()> {
        if url.starts_with("https://") {
            return Ok(());
        }
        if self.policy.allow_loopback_http
            && ["http://127.0.0.1:", "http://[::1]:", "http://localhost:"]
                .iter()
                .any(|p| url.starts_with(p))
        {
            return Ok(());
        }
        fail(format!("refusing a non-HTTPS URL: {url}"))
    }

    fn retrying<T>(
        &self,
        what: &str,
        mut attempt: impl FnMut() -> Result<Attempt<T>>,
    ) -> Result<T> {
        let mut last = String::new();
        for n in 0..ATTEMPTS {
            if n > 0 {
                std::thread::sleep(self.policy.backoff * 2u32.pow(n - 1));
            }
            match attempt()? {
                Attempt::Done(value) => return Ok(value),
                Attempt::Retry(reason) => last = reason,
            }
        }
        fail(format!("{what}: gave up after {ATTEMPTS} attempts: {last}"))
    }

    /// GET a small document. `None` is a 404; any other non-2xx fails, and
    /// so does a body longer than [`MAX_DOCUMENT`].
    pub(crate) fn get_bytes(&self, url: &str) -> Result<Option<Vec<u8>>> {
        self.check_url(url)?;
        self.retrying(&format!("GET {url}"), || {
            let response = match self.agent.get(url).call() {
                Ok(response) => response,
                Err(e) => return Ok(Attempt::Retry(e.to_string())),
            };
            let status = response.status().as_u16();
            match status {
                200 => {
                    // One byte past the cap is enough to tell an oversized
                    // body apart; nothing beyond it is read or kept.
                    let mut bytes = Vec::new();
                    let read = response
                        .into_body()
                        .into_with_config()
                        .limit(MAX_DOCUMENT + 1)
                        .reader()
                        .take(MAX_DOCUMENT + 1)
                        .read_to_end(&mut bytes);
                    match read {
                        Err(e) => Ok(Attempt::Retry(format!("GET {url}: {e}"))),
                        Ok(_) if bytes.len() as u64 > MAX_DOCUMENT => fail(format!(
                            "GET {url}: longer than the {MAX_DOCUMENT} bytes a document may have"
                        )),
                        Ok(_) => Ok(Attempt::Done(Some(bytes))),
                    }
                }
                404 => Ok(Attempt::Done(None)),
                429 | 500..=599 => Ok(Attempt::Retry(format!("HTTP {status}"))),
                _ => fail(format!("GET {url}: HTTP {status}")),
            }
        })
    }

    /// GET straight into `destination` through a temporary sibling, checking
    /// size and BLAKE3 before the rename. Wrong bytes never land.
    pub(crate) fn get_verified(
        &self,
        url: &str,
        destination: &Path,
        blake3: &str,
        size: u64,
    ) -> Result<()> {
        self.check_url(url)?;
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).context(|| format!("create {}", parent.display()))?;
        }
        let temporary = super::temporary_sibling(destination);
        let outcome = self.retrying(&format!("GET {url}"), || {
            let response = match self.agent.get(url).call() {
                Ok(response) => response,
                Err(e) => return Ok(Attempt::Retry(e.to_string())),
            };
            let status = response.status().as_u16();
            match status {
                200 => {}
                404 => return fail(format!("GET {url}: not found; the pinned revision is missing an object")),
                429 | 500..=599 => return Ok(Attempt::Retry(format!("HTTP {status}"))),
                _ => return fail(format!("GET {url}: HTTP {status}")),
            }
            let mut reader = response.into_body().into_with_config().limit(size.min(MAX_BODY) + 1).reader();
            let mut file = std::fs::File::create(&temporary).context(|| format!("create {}", temporary.display()))?;
            let mut hasher = blake3::Hasher::new();
            let mut buffer = vec![0u8; 1 << 20];
            let mut written = 0u64;
            loop {
                let read = match reader.read(&mut buffer) {
                    Ok(read) => read,
                    Err(e) => return Ok(Attempt::Retry(e.to_string())),
                };
                if read == 0 {
                    break;
                }
                written += read as u64;
                if written > size {
                    return fail(format!("GET {url}: longer than the {size} bytes the manifest pins"));
                }
                hasher.update(&buffer[..read]);
                file.write_all(&buffer[..read]).context(|| format!("write {}", temporary.display()))?;
            }
            file.sync_all().context(|| format!("sync {}", temporary.display()))?;
            let actual = hasher.finalize().to_hex().to_string();
            if written != size || actual != blake3 {
                return fail(format!(
                    "GET {url}: got {written} bytes with BLAKE3 {actual}, the manifest pins {size} bytes with {blake3}"
                ));
            }
            Ok(Attempt::Done(()))
        });
        match outcome {
            Ok(()) => super::rename_into_place(&temporary, destination),
            Err(e) => {
                let _ = std::fs::remove_file(&temporary);
                Err(e)
            }
        }
    }

    /// Conditional PUT. `headers` must already carry the signature; this
    /// adds nothing the signature does not cover except `content-length`.
    pub(crate) fn put_if_absent(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: Body<'_>,
    ) -> Result<PutOutcome> {
        self.check_url(url)?;
        if !headers
            .iter()
            .any(|(n, v)| n.eq_ignore_ascii_case("if-none-match") && v == "*")
        {
            return fail("internal: an upload must be conditional on the object being absent");
        }
        self.retrying(&format!("PUT {url}"), || {
            let mut request = self.agent.put(url);
            for (name, value) in headers {
                request = request.header(name.as_str(), value.as_str());
            }
            let sent = match &body {
                Body::Bytes(bytes) => request.send(*bytes),
                Body::File(path) => {
                    let file =
                        std::fs::File::open(path).context(|| format!("open {}", path.display()))?;
                    request.send(file)
                }
            };
            let response = match sent {
                Ok(response) => response,
                Err(e) => return Ok(Attempt::Retry(e.to_string())),
            };
            let status = response.status().as_u16();
            match status {
                200 | 201 | 204 => Ok(Attempt::Done(PutOutcome::Created)),
                412 => Ok(Attempt::Done(PutOutcome::AlreadyExists)),
                429 | 500..=599 => Ok(Attempt::Retry(format!("HTTP {status}"))),
                _ => {
                    // S3 error bodies name the failing condition; keep a short,
                    // bounded excerpt. They never echo the credentials.
                    let detail = response
                        .into_body()
                        .into_with_config()
                        .limit(4096)
                        .lossy_utf8(true)
                        .read_to_string()
                        .unwrap_or_default();
                    fail(format!(
                        "PUT {url}: HTTP {status}: {}",
                        detail.chars().take(512).collect::<String>()
                    ))
                }
            }
        })
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! A loopback HTTP/1.1 server for tests: one canned response per
    //! connection, every request recorded.

    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone)]
    pub(crate) struct Recorded {
        pub(crate) method: String,
        pub(crate) path: String,
        pub(crate) headers: Vec<(String, String)>,
        pub(crate) body: Vec<u8>,
    }

    pub(crate) type Handler =
        dyn Fn(&Recorded) -> (u16, Vec<(String, String)>, Vec<u8>) + Send + Sync;

    pub(crate) struct Server {
        pub(crate) base: String,
        pub(crate) requests: Arc<Mutex<Vec<Recorded>>>,
    }

    pub(crate) fn serve(handler: Box<Handler>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_owned();
                let path = parts.next().unwrap_or_default().to_owned();
                let mut headers = Vec::new();
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    let header = header.trim_end();
                    if header.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':') {
                        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
                    }
                }
                let length = headers
                    .iter()
                    .find(|(n, _)| n == "content-length")
                    .and_then(|(_, v)| v.parse::<usize>().ok())
                    .unwrap_or(0);
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).unwrap();
                let recorded = Recorded {
                    method,
                    path,
                    headers,
                    body,
                };
                let (status, extra, payload) = handler(&recorded);
                log.lock().unwrap().push(recorded);
                let mut response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n",
                    payload.len()
                );
                for (name, value) in extra {
                    response.push_str(&format!("{name}: {value}\r\n"));
                }
                response.push_str("\r\n");
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&payload);
                let _ = stream.flush();
            }
        });
        Server { base, requests }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::serve;
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn plain_http_is_refused_unless_loopback_and_opted_in() {
        let production = Client::new(Policy::production());
        assert!(production.check_url("http://127.0.0.1:1/x").is_err());
        assert!(production.check_url("http://cdn.example/x").is_err());
        assert!(production.check_url("https://cdn.example/x").is_ok());
        let test = Client::new(Policy::loopback_for_tests());
        assert!(test.check_url("http://127.0.0.1:1/x").is_ok());
        assert!(test.check_url("http://cdn.example/x").is_err());
    }

    #[test]
    fn get_maps_404_to_none_retries_5xx_and_never_follows_redirects() {
        let calls = Arc::new(AtomicU32::new(0));
        let seen = Arc::clone(&calls);
        let server = serve(Box::new(move |request| {
            let n = seen.fetch_add(1, Ordering::SeqCst);
            match request.path.as_str() {
                "/missing" => (404, vec![], vec![]),
                "/flaky" if n < 2 => (503, vec![], vec![]),
                "/flaky" => (200, vec![], b"ok".to_vec()),
                "/moved" => (302, vec![("location".into(), "/flaky".into())], vec![]),
                _ => (500, vec![], vec![]),
            }
        }));
        let client = Client::new(Policy::loopback_for_tests());
        assert_eq!(
            client
                .get_bytes(&format!("{}/missing", server.base))
                .unwrap(),
            None
        );
        assert_eq!(
            client.get_bytes(&format!("{}/flaky", server.base)).unwrap(),
            Some(b"ok".to_vec())
        );
        assert!(client.get_bytes(&format!("{}/moved", server.base)).is_err());
        let paths: Vec<String> = server
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.path.clone())
            .collect();
        assert_eq!(paths.iter().filter(|p| *p == "/moved").count(), 1);
        assert_eq!(paths.last().map(String::as_str), Some("/moved"));
        assert!(
            client
                .get_bytes(&format!("{}/always-down", server.base))
                .is_err()
        );
    }

    #[test]
    fn an_oversized_document_fails_once_without_being_buffered_whole() {
        let server = serve(Box::new(|request| match request.path.as_str() {
            "/at-cap" => (200, vec![], vec![b'a'; MAX_DOCUMENT as usize]),
            _ => (200, vec![], vec![b'a'; MAX_DOCUMENT as usize + 1]),
        }));
        let client = Client::new(Policy::loopback_for_tests());
        let at_cap = client
            .get_bytes(&format!("{}/at-cap", server.base))
            .unwrap()
            .unwrap();
        assert_eq!(at_cap.len() as u64, MAX_DOCUMENT);
        let refused = client
            .get_bytes(&format!("{}/oversized", server.base))
            .unwrap_err()
            .to_string();
        assert!(refused.contains("longer than"), "{refused}");
        let requests = server.requests.lock().unwrap();
        assert_eq!(
            requests.iter().filter(|r| r.path == "/oversized").count(),
            1,
            "an oversized body is refused, not retried"
        );
    }

    #[test]
    fn verified_get_lands_only_the_pinned_bytes() {
        let server = serve(Box::new(|request| match request.path.as_str() {
            "/good" => (200, vec![], b"fixture".to_vec()),
            _ => (200, vec![], b"tampered".to_vec()),
        }));
        let dir =
            std::env::temp_dir().join(format!("mediaprobe-xtask-http-{}", std::process::id()));
        let destination = dir.join("a.bin");
        let client = Client::new(Policy::loopback_for_tests());
        let digest = super::super::blake3_bytes(b"fixture");
        client
            .get_verified(&format!("{}/good", server.base), &destination, &digest, 7)
            .unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"fixture");
        let other = dir.join("b.bin");
        assert!(
            client
                .get_verified(&format!("{}/bad", server.base), &other, &digest, 7)
                .is_err()
        );
        assert!(
            client
                .get_verified(&format!("{}/bad", server.base), &other, &digest, 8)
                .is_err()
        );
        assert!(!other.exists());
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "no partial file is left behind"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn put_is_conditional_and_reports_existing_objects() {
        let server = serve(Box::new(|request| match request.path.as_str() {
            "/exists" => (412, vec![], vec![]),
            "/forbidden" => (
                403,
                vec![],
                b"<Error><Code>AccessDenied</Code></Error>".to_vec(),
            ),
            _ => (200, vec![], vec![]),
        }));
        let client = Client::new(Policy::loopback_for_tests());
        let conditional = vec![("if-none-match".to_owned(), "*".to_owned())];
        assert_eq!(
            client
                .put_if_absent(
                    &format!("{}/new", server.base),
                    &conditional,
                    Body::Bytes(b"abc")
                )
                .unwrap(),
            PutOutcome::Created
        );
        assert_eq!(
            client
                .put_if_absent(
                    &format!("{}/exists", server.base),
                    &conditional,
                    Body::Bytes(b"abc")
                )
                .unwrap(),
            PutOutcome::AlreadyExists
        );
        let denied = client.put_if_absent(
            &format!("{}/forbidden", server.base),
            &conditional,
            Body::Bytes(b"abc"),
        );
        assert!(denied.unwrap_err().to_string().contains("AccessDenied"));
        assert!(
            client
                .put_if_absent(&format!("{}/new", server.base), &[], Body::Bytes(b"abc"))
                .is_err()
        );
        let recorded = server.requests.lock().unwrap();
        assert_eq!(recorded[0].body, b"abc");
        assert!(
            recorded[0]
                .headers
                .iter()
                .any(|(n, v)| n == "if-none-match" && v == "*")
        );
        assert!(
            recorded[0]
                .headers
                .iter()
                .any(|(n, v)| n == "content-length" && v == "3")
        );
    }
}
