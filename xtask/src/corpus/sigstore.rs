//! Keyless Sigstore signing and verification of the manifest, through the
//! `cosign` binary. Signing only works inside the publish workflow, where
//! GitHub's OIDC token names that workflow on `main`; verification pins
//! exactly that identity and issuer, so a bundle signed anywhere else fails.
//!
//! `MEDIAPROBE_COSIGN` overrides the program (CI installs a pinned cosign;
//! tests substitute a stub).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{Context as _, Result, fail};

pub(crate) fn program() -> PathBuf {
    std::env::var_os("MEDIAPROBE_COSIGN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cosign"))
}

fn run(program: &Path, args: &[&std::ffi::OsStr], what: &str) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context(|| format!("run {} ({what})", program.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(12).collect();
        return fail(format!(
            "{what} failed ({}):\n{}",
            output.status,
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        ));
    }
    Ok(())
}

/// `cosign sign-blob --yes --bundle <bundle> <blob>`: a keyless signature
/// whose certificate carries the ambient OIDC identity.
pub(crate) fn sign(program: &Path, blob: &Path, bundle: &Path) -> Result<()> {
    run(
        program,
        &[
            "sign-blob".as_ref(),
            "--yes".as_ref(),
            "--bundle".as_ref(),
            bundle.as_os_str(),
            blob.as_os_str(),
        ],
        "cosign sign-blob",
    )
}

/// `cosign verify-blob` against the exact identity and issuer the lock names.
pub(crate) fn verify(
    program: &Path,
    blob: &Path,
    bundle: &Path,
    identity: &str,
    issuer: &str,
) -> Result<()> {
    run(
        program,
        &[
            "verify-blob".as_ref(),
            "--bundle".as_ref(),
            bundle.as_os_str(),
            "--certificate-identity".as_ref(),
            identity.as_ref(),
            "--certificate-oidc-issuer".as_ref(),
            issuer.as_ref(),
            blob.as_os_str(),
        ],
        "cosign verify-blob",
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    /// A stub that records its arguments and succeeds only for the right identity.
    fn stub(dir: &Path) -> PathBuf {
        let path = dir.join("cosign-stub");
        std::fs::write(
            &path,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$(dirname \"$0\")/args\"\n\
             for a in \"$@\"; do [ \"$a\" = wrong-identity ] && { echo 'none of the expected identities matched' >&2; exit 1; }; done\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn verification_passes_the_pinned_identity_and_fails_closed() {
        let dir =
            std::env::temp_dir().join(format!("mediaprobe-xtask-cosign-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cosign = stub(&dir);
        let blob = dir.join("m.json");
        let bundle = dir.join("m.json.sigstore.json");
        verify(
            &cosign,
            &blob,
            &bundle,
            super::super::lock::IDENTITY,
            super::super::lock::ISSUER,
        )
        .unwrap();
        let args = std::fs::read_to_string(dir.join("args")).unwrap();
        let args: Vec<&str> = args.lines().collect();
        assert_eq!(args[0], "verify-blob");
        let after = |flag: &str| args[args.iter().position(|a| *a == flag).unwrap() + 1];
        assert_eq!(
            after("--certificate-identity"),
            super::super::lock::IDENTITY
        );
        assert_eq!(
            after("--certificate-oidc-issuer"),
            super::super::lock::ISSUER
        );
        let refused = verify(
            &cosign,
            &blob,
            &bundle,
            "wrong-identity",
            super::super::lock::ISSUER,
        )
        .unwrap_err();
        assert!(refused.to_string().contains("expected identities"));
        sign(&cosign, &blob, &bundle).unwrap();
        assert!(
            std::fs::read_to_string(dir.join("args"))
                .unwrap()
                .starts_with("sign-blob\n--yes\n--bundle\n")
        );
        assert!(verify(&dir.join("absent"), &blob, &bundle, "i", "j").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
