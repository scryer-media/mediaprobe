//! AWS Signature Version 4 for the one request this tooling signs: a
//! conditional PUT of one object to R2's S3-compatible endpoint.
//!
//! SHA-256 is here only because SigV4 specifies it. The secret is used as an
//! HMAC key in this process and nowhere else: never an argument, a header,
//! an error message or `Debug` output.

use hmac::{Hmac, KeyInit as _, Mac as _};
use sha2::{Digest as _, Sha256};

use super::{Result, fail, hex, utc_parts};

/// R2 signs in the `auto` region with S3's service name.
pub(crate) const REGION: &str = "auto";
pub(crate) const SERVICE: &str = "s3";
const ALGORITHM: &str = "AWS4-HMAC-SHA256";

#[derive(Clone)]
pub(crate) struct Credentials {
    pub(crate) access_key_id: String,
    pub(crate) secret_access_key: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Credentials(access_key_id={}, secret=<redacted>)",
            self.access_key_id
        )
    }
}

pub(crate) fn timestamp(unix_seconds: u64) -> String {
    let (y, mo, d, h, mi, s) = utc_parts(unix_seconds);
    format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z")
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub(crate) fn sha256_file(path: &std::path::Path) -> Result<String> {
    use super::Context as _;
    use std::io::Read as _;
    let mut file = std::fs::File::open(path).context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file
            .read(&mut buffer)
            .context(|| format!("read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

/// One request to sign. Every header listed is signed; nothing else is.
pub(crate) struct Signable<'a> {
    pub(crate) method: &'a str,
    pub(crate) path: &'a str,
    pub(crate) query: &'a str,
    pub(crate) headers: &'a [(String, String)],
    pub(crate) payload_sha256: &'a str,
    pub(crate) timestamp: &'a str,
    pub(crate) region: &'a str,
    pub(crate) service: &'a str,
}

impl Signable<'_> {
    fn sorted_headers(&self) -> Vec<(String, String)> {
        let mut headers: Vec<(String, String)> = self
            .headers
            .iter()
            .map(|(n, v)| (n.to_ascii_lowercase(), v.trim().to_owned()))
            .collect();
        headers.sort();
        headers
    }

    pub(crate) fn signed_headers(&self) -> String {
        self.sorted_headers()
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>()
            .join(";")
    }

    pub(crate) fn canonical_request(&self) -> Result<String> {
        let mut out = format!(
            "{}\n{}\n{}\n",
            self.method,
            canonical_path(self.path)?,
            canonical_query(self.query)
        );
        for (name, value) in self.sorted_headers() {
            out.push_str(&format!("{name}:{value}\n"));
        }
        out.push('\n');
        out.push_str(&self.signed_headers());
        out.push('\n');
        out.push_str(self.payload_sha256);
        Ok(out)
    }

    fn date(&self) -> Result<&str> {
        let s = self.timestamp;
        let ok = s.len() == 16
            && s.as_bytes()[8] == b'T'
            && s.ends_with('Z')
            && s[..8].bytes().all(|b| b.is_ascii_digit())
            && s[9..15].bytes().all(|b| b.is_ascii_digit());
        if !ok {
            return fail(format!("not an x-amz-date stamp: {s:?}"));
        }
        Ok(&s[..8])
    }

    pub(crate) fn scope(&self) -> Result<String> {
        Ok(format!(
            "{}/{}/{}/aws4_request",
            self.date()?,
            self.region,
            self.service
        ))
    }

    pub(crate) fn string_to_sign(&self) -> Result<String> {
        Ok(format!(
            "{ALGORITHM}\n{}\n{}\n{}",
            self.timestamp,
            self.scope()?,
            sha256_hex(self.canonical_request()?.as_bytes())
        ))
    }

    pub(crate) fn authorization(&self, credentials: &Credentials) -> Result<String> {
        let mut key = hmac(
            format!("AWS4{}", credentials.secret_access_key).as_bytes(),
            self.date()?.as_bytes(),
        );
        key = hmac(&key, self.region.as_bytes());
        key = hmac(&key, self.service.as_bytes());
        key = hmac(&key, b"aws4_request");
        let signature = hex(&hmac(&key, self.string_to_sign()?.as_bytes()));
        Ok(format!(
            "{ALGORITHM} Credential={}/{}, SignedHeaders={}, Signature={signature}",
            credentials.access_key_id,
            self.scope()?,
            self.signed_headers()
        ))
    }
}

/// S3 encodes the path once. A path that already holds a `%` cannot be
/// encoded without guessing how the far end decodes it, so it is refused.
fn canonical_path(path: &str) -> Result<String> {
    if path.contains('%') {
        return fail(format!("refusing to sign a percent-escaped path: {path}"));
    }
    if path.is_empty() {
        return Ok("/".to_owned());
    }
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte == b'/' || byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
        {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    Ok(out)
}

fn canonical_query(query: &str) -> String {
    let mut pairs: Vec<(&str, &str)> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| p.split_once('=').unwrap_or((p, "")))
        .collect();
    pairs.sort_unstable();
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// AWS's published `get-vanilla` vector from the SigV4 test suite.
    #[test]
    fn the_published_aws_test_vector_signs_byte_for_byte() {
        let signed = headers(&[
            ("host", "example.amazonaws.com"),
            ("x-amz-date", "20150830T123600Z"),
        ]);
        let request = Signable {
            method: "GET",
            path: "/",
            query: "",
            headers: &signed,
            payload_sha256: &sha256_hex(b""),
            timestamp: "20150830T123600Z",
            region: "us-east-1",
            service: "service",
        };
        assert_eq!(
            request.string_to_sign().unwrap(),
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\n\
             bb579772317eb040ac9ed261061d46c1f17a8133879d6129b6e1c25292927e63"
        );
        let credentials = Credentials {
            access_key_id: "AKIDEXAMPLE".into(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
        };
        assert_eq!(
            request.authorization(&credentials).unwrap(),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    #[test]
    fn the_secret_never_reaches_debug_output_or_the_header() {
        let credentials = Credentials {
            access_key_id: "id".into(),
            secret_access_key: "very-secret".into(),
        };
        assert!(!format!("{credentials:?}").contains("very-secret"));
        let signed = headers(&[("host", "h"), ("x-amz-date", "20260101T000000Z")]);
        let request = Signable {
            method: "PUT",
            path: "/bucket/test-corpus/objects/blake3/ab",
            query: "",
            headers: &signed,
            payload_sha256: &sha256_hex(b"x"),
            timestamp: "20260101T000000Z",
            region: REGION,
            service: SERVICE,
        };
        assert!(
            !request
                .authorization(&credentials)
                .unwrap()
                .contains("very-secret")
        );
    }

    #[test]
    fn malformed_inputs_fail_before_signing() {
        let signed = headers(&[("host", "h")]);
        let mut request = Signable {
            method: "PUT",
            path: "/a%20b",
            query: "",
            headers: &signed,
            payload_sha256: "",
            timestamp: "20260101T000000Z",
            region: REGION,
            service: SERVICE,
        };
        assert!(request.canonical_request().is_err());
        request.path = "/ok";
        request.timestamp = "2026-01-01";
        assert!(request.scope().is_err());
        assert_eq!(timestamp(1_440_938_160), "20150830T123600Z");
        assert_eq!(canonical_query("b=2&a=1"), "a=1&b=2");
    }
}
