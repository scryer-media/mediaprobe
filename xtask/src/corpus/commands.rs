//! `cargo xtask test-corpus <command>`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::http::{Body, Client, Policy, PutOutcome};
use super::ledger::{self, Ledger};
use super::lock::{self, Lock, PublishedFrom};
use super::manifest::{Manifest, Provenance};
use super::profiles::ProfilesFile;
use super::sigv4::Credentials;
use super::store::{R2, Sink, Source};
use super::toolchain::{self, ToolchainFile};
use super::{Context as _, Result, error, fail, repo_path, repo_root, sigstore};
use crate::recipes::{self, run::Tools};

pub(crate) const USAGE: &str = "\
cargo xtask test-corpus <command> [options]

  generate     Run the recipes into tests/media (or --root).
               --fixture NAME (repeatable)  --profile unit|large|all
               --generator ID (repeatable)  --keep-going
               --skip-unavailable  --update-ledger  --ffmpeg PATH  --speexenc PATH
               --root DIR  --scratch DIR
  expectations Render tests/media-fixtures.toml.            [--check]
  ledger       Render test-corpus/sources.json shape.        [--check]
  verify       Check config consistency and generated files. [--root DIR] [--all-present] [--candidate]
  build        Write the manifest for the generated corpus.  [--out DIR]
  provenance   Write the provenance document.                [--out DIR] [--ffmpeg PATH] [--speexenc PATH]
  sign         Sign the manifest keylessly (publish workflow only). [--out DIR]
  verify-signature  Check a manifest bundle against the lock identity.
               --manifest FILE --bundle FILE, or the pinned revision via the lock [--from-dir DIR]
  publish      Upload a built, signed revision (create-only).
               --to-dir DIR | --r2 (R2_CORPUS_ACCESS_KEY, R2_CORPUS_ACCESS_SECRET, R2_CORPUS_ENDPOINT)
               --bucket NAME (or R2_BUCKET_NAME)  --public-url URL (or R2_PUBLIC_URL)
               [--out DIR] [--root DIR] [--dry-run] [--write-lock]
  hydrate      Download the pinned revision into tests/media.
               [--profile NAME]... [--lock FILE] [--root DIR] [--from-dir DIR] [--verify-signature]
  paths        Print the corpus paths of a profile.         [--profile NAME] [--root DIR] [--existing]
";

/// Hand-rolled flags: `--name value`, `--name=value` or a bare `--switch`.
pub(crate) struct Args {
    rest: Vec<String>,
}

impl Args {
    pub(crate) fn new(rest: Vec<String>) -> Self {
        Args { rest }
    }

    pub(crate) fn switch(&mut self, name: &str) -> bool {
        let flag = format!("--{name}");
        let before = self.rest.len();
        self.rest.retain(|a| *a != flag);
        self.rest.len() != before
    }

    pub(crate) fn values(&mut self, name: &str) -> Result<Vec<String>> {
        let flag = format!("--{name}");
        let prefix = format!("--{name}=");
        let mut found = Vec::new();
        let mut kept = Vec::new();
        let mut iter = std::mem::take(&mut self.rest).into_iter();
        while let Some(arg) = iter.next() {
            if arg == flag {
                match iter.next() {
                    Some(value) if !value.starts_with("--") => found.push(value),
                    _ => return fail(format!("{flag} needs a value")),
                }
            } else if let Some(value) = arg.strip_prefix(&prefix) {
                found.push(value.to_owned());
            } else {
                kept.push(arg);
            }
        }
        self.rest = kept;
        Ok(found)
    }

    pub(crate) fn value(&mut self, name: &str) -> Result<Option<String>> {
        let mut values = self.values(name)?;
        if values.len() > 1 {
            return fail(format!("--{name} given more than once"));
        }
        Ok(values.pop())
    }

    pub(crate) fn path(&mut self, name: &str) -> Result<Option<PathBuf>> {
        Ok(self.value(name)?.map(PathBuf::from))
    }

    pub(crate) fn finish(self) -> Result<()> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            fail(format!("unexpected arguments: {}", self.rest.join(" ")))
        }
    }
}

pub(crate) fn dispatch(command: &str, args: Args) -> Result<()> {
    match command {
        "generate" => generate(args),
        "expectations" => expectations(args),
        "ledger" => ledger_command(args),
        "verify" => verify(args),
        "build" => build(args),
        "provenance" => provenance(args),
        "sign" => sign(args),
        "verify-signature" => verify_signature(args),
        "publish" => publish(args),
        "hydrate" => hydrate(args),
        "paths" => paths(args),
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        other => fail(format!("unknown test-corpus command {other:?}\n\n{USAGE}")),
    }
}

/// Where a run reads and writes fixtures: `<root>/tests/media/...`. The
/// ledger paths already start with `tests/media/`, so they join onto `root`.
fn root_arg(args: &mut Args) -> Result<PathBuf> {
    Ok(args.path("root")?.unwrap_or_else(repo_root))
}

fn out_dir(args: &mut Args) -> Result<PathBuf> {
    Ok(args
        .path("out")?
        .unwrap_or_else(|| repo_path("target/test-corpus")))
}

fn mib(bytes: u64) -> String {
    format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
}

struct Toolchain {
    tools: Tools,
    versions: BTreeMap<String, String>,
    missing_encoders: BTreeSet<String>,
}

fn probe_toolchain(args: &mut Args) -> Result<Toolchain> {
    let path = repo_path(super::TOOLCHAIN);
    let file = ToolchainFile::load(&path)?;
    let ffmpeg = args.path("ffmpeg")?;
    let speexenc = args.path("speexenc")?;
    let mut chosen = BTreeMap::new();
    let mut versions = BTreeMap::new();
    let mut missing_encoders = BTreeSet::new();
    for (name, tool) in &file.tools {
        let program = match name.as_str() {
            "ffmpeg" => ffmpeg.clone(),
            "speexenc" => speexenc.clone(),
            _ => None,
        }
        .unwrap_or_else(|| PathBuf::from(&tool.program));
        let probed = toolchain::probe(&program, &tool.encoders)?;
        match probed.major {
            Some(major) if major >= tool.min_major => {}
            _ => {
                return fail(format!(
                    "{name}: {:?} does not satisfy major version >= {}",
                    probed.version_line, tool.min_major
                ));
            }
        }
        missing_encoders.extend(probed.missing_encoders);
        versions.insert(name.clone(), probed.version_line);
        chosen.insert(name.clone(), program);
    }
    let take = |name: &str| {
        chosen
            .get(name)
            .cloned()
            .ok_or_else(|| error(format!("toolchain.json names no {name}")))
    };
    Ok(Toolchain {
        tools: Tools {
            ffmpeg: take("ffmpeg")?,
            speexenc: take("speexenc")?,
        },
        versions,
        missing_encoders,
    })
}

fn generate(mut args: Args) -> Result<()> {
    let names: BTreeSet<String> = args.values("fixture")?.into_iter().collect();
    let profile = args.value("profile")?;
    let generators: BTreeSet<String> = args.values("generator")?.into_iter().collect();
    let keep_going = args.switch("keep-going");
    let skip_unavailable = args.switch("skip-unavailable");
    let update_ledger = args.switch("update-ledger");
    let root = root_arg(&mut args)?;
    let scratch = args
        .path("scratch")?
        .unwrap_or_else(|| repo_path("target/test-corpus-work"));
    let toolchain = probe_toolchain(&mut args)?;
    args.finish()?;

    let fixtures = recipes::all();
    let known: BTreeSet<&str> = fixtures.iter().map(|f| f.name.as_str()).collect();
    if let Some(unknown) = names.iter().find(|n| !known.contains(n.as_str())) {
        return fail(format!("no fixture named {unknown}"));
    }
    if let Some(unknown) = generators
        .iter()
        .find(|g| !recipes::Generator::ALL.iter().any(|k| k.id() == g.as_str()))
    {
        return fail(format!("no generator named {unknown}"));
    }
    if keep_going && update_ledger {
        return fail(
            "--keep-going cannot be combined with --update-ledger: the ledger records complete runs only",
        );
    }
    let selected: Vec<&recipes::Fixture> = fixtures
        .iter()
        .filter(|f| names.is_empty() || names.contains(&f.name))
        .filter(|f| generators.is_empty() || generators.contains(f.generator.id()))
        .filter(|f| match profile.as_deref() {
            None | Some("all") => true,
            Some(p) => f.profile() == p,
        })
        .collect();
    if selected.is_empty() {
        return fail("the selection names no fixture");
    }
    let (runnable, blocked): (Vec<_>, Vec<_>) = selected.into_iter().partition(|f| {
        f.encoders
            .iter()
            .all(|e| !toolchain.missing_encoders.contains(*e))
    });
    if !blocked.is_empty() && !skip_unavailable {
        return fail(format!(
            "this FFmpeg lacks {} needed by {} fixture(s) (first: {}); install them or pass --skip-unavailable",
            toolchain
                .missing_encoders
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            blocked.len(),
            blocked[0].name
        ));
    }
    let media_root = root.join(super::MEDIA_ROOT);
    let mut ledger = if update_ledger {
        Some(Ledger::load(&repo_path(super::SOURCES))?)
    } else {
        None
    };
    let started = Instant::now();
    let mut total = 0u64;
    let mut large = Vec::new();
    let mut failed = Vec::new();
    for fixture in &runnable {
        let begun = Instant::now();
        let path = match recipes::run::generate(fixture, &media_root, &scratch, &toolchain.tools) {
            Ok(path) => path,
            Err(e) if keep_going => {
                println!(
                    "    FAILED  {:>7.1}s  {}: {e}",
                    begun.elapsed().as_secs_f64(),
                    fixture.name
                );
                failed.push(fixture.name.clone());
                continue;
            }
            Err(e) => return fail(format!("{}: {e}", fixture.name)),
        };
        let (digest, size) = super::digest_file(&path)?;
        total += size;
        if fixture.profile() == "large" {
            large.push((fixture.name.clone(), size));
        }
        println!(
            "{:>10}  {:>7.1}s  {}",
            size,
            begun.elapsed().as_secs_f64(),
            fixture.name
        );
        if let Some(ledger) = ledger.as_mut() {
            let entry = ledger.entry_mut(&fixture.path()).ok_or_else(|| {
                error(format!(
                    "{} is not in sources.json: run `test-corpus ledger`",
                    fixture.path()
                ))
            })?;
            entry.size = Some(size);
            entry.blake3 = Some(digest);
        }
    }
    if let Some(ledger) = ledger {
        ledger.validate()?;
        super::write_atomic(&repo_path(super::SOURCES), &super::canonical_json(&ledger)?)?;
    }
    println!(
        "generated {} fixture(s), {} ({total} bytes), in {:.1}s",
        runnable.len() - failed.len(),
        mib(total),
        started.elapsed().as_secs_f64()
    );
    for (name, size) in &large {
        println!("  large: {name}: {} ({size} bytes)", mib(*size));
    }
    for fixture in &blocked {
        let missing: Vec<&str> = fixture
            .encoders
            .iter()
            .copied()
            .filter(|e| toolchain.missing_encoders.contains(*e))
            .collect();
        println!(
            "  skipped {} (this FFmpeg lacks {})",
            fixture.name,
            missing.join(", ")
        );
    }
    if !failed.is_empty() {
        return fail(format!(
            "{} fixture(s) failed: {}",
            failed.len(),
            failed.join(", ")
        ));
    }
    Ok(())
}

fn expectations(mut args: Args) -> Result<()> {
    let check = args.switch("check");
    args.finish()?;
    let rendered = recipes::render_expectations(&recipes::all());
    let path = repo_path(super::EXPECTATIONS);
    if check {
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        if committed != rendered {
            return fail(format!(
                "{} is stale: run `cargo xtask test-corpus expectations`",
                super::EXPECTATIONS
            ));
        }
        return Ok(());
    }
    super::write_atomic(&path, rendered.as_bytes())
}

fn ledger_command(mut args: Args) -> Result<()> {
    let check = args.switch("check");
    args.finish()?;
    let path = repo_path(super::SOURCES);
    let previous = if path.is_file() {
        Some(Ledger::load(&path)?)
    } else {
        None
    };
    if check {
        let Some(committed) = previous else {
            return fail(format!("{} is missing", super::SOURCES));
        };
        return match ledger::shape_drift(&committed) {
            Some(problem) => fail(problem),
            None => Ok(()),
        };
    }
    let rendered = Ledger::from_recipes(&recipes::all(), previous.as_ref());
    rendered.validate()?;
    super::write_atomic(&path, &super::canonical_json(&rendered)?)
}

fn verify(mut args: Args) -> Result<()> {
    let all_present = args.switch("all-present");
    let candidate = args.switch("candidate");
    let root = root_arg(&mut args)?;
    args.finish()?;
    let ledger = Ledger::load(&repo_path(super::SOURCES))?;
    let mut problems = Vec::new();
    if let Some(drift) = ledger::shape_drift(&ledger) {
        problems.push(drift);
    }
    if std::fs::read_to_string(repo_path(super::EXPECTATIONS)).unwrap_or_default()
        != recipes::render_expectations(&recipes::all())
    {
        problems.push(format!(
            "{} is stale: run `cargo xtask test-corpus expectations`",
            super::EXPECTATIONS
        ));
    }
    let profiles = ProfilesFile::load(&repo_path(super::PROFILES))?;
    for name in profiles.profiles.keys() {
        if let Err(e) = profiles.select(name, &ledger) {
            problems.push(e.to_string());
        }
    }
    ToolchainFile::load(&repo_path(super::TOOLCHAIN))?;
    let lock = Lock::load(&repo_path(super::LOCK))?;
    if !lock.is_unpublished() && !candidate && ledger.files.iter().any(|f| f.pending()) {
        problems.push("the lock pins a revision but sources.json has pending entries".to_owned());
    }
    problems.extend(ledger.check_tree(&root, all_present || candidate, candidate));
    if problems.is_empty() {
        let pending = ledger.files.iter().filter(|f| f.pending()).count();
        println!(
            "test-corpus: consistent ({} files, {} pending first publication, lock {})",
            ledger
                .files
                .iter()
                .filter(|f| f.is_published_shape())
                .count(),
            pending,
            if lock.is_unpublished() {
                "unpublished"
            } else {
                "pinned"
            }
        );
        Ok(())
    } else {
        fail(format!(
            "test-corpus verify found {} problem(s):\n  {}",
            problems.len(),
            problems.join("\n  ")
        ))
    }
}

fn build_manifest() -> Result<(Manifest, Vec<u8>)> {
    let ledger = Ledger::load(&repo_path(super::SOURCES))?;
    if let Some(drift) = ledger::shape_drift(&ledger) {
        return fail(drift);
    }
    let profiles = ProfilesFile::load(&repo_path(super::PROFILES))?;
    let toolchain_bytes =
        std::fs::read(repo_path(super::TOOLCHAIN)).context(|| "read toolchain.json".to_owned())?;
    let manifest = Manifest::build(&ledger, &profiles, &toolchain_bytes)?;
    let bytes = manifest.canonical_bytes()?;
    Ok((manifest, bytes))
}

const MANIFEST_FILE: &str = "manifest.json";
const BUNDLE_FILE: &str = "manifest.json.sigstore.json";
const PROVENANCE_FILE: &str = "provenance.json";

fn build(mut args: Args) -> Result<()> {
    let out = out_dir(&mut args)?;
    args.finish()?;
    let (manifest, bytes) = build_manifest()?;
    super::write_atomic(&out.join(MANIFEST_FILE), &bytes)?;
    let total: u64 = manifest.files.iter().map(|f| f.size).sum();
    println!(
        "manifest {} ({} files, {})",
        manifest.blake3()?,
        manifest.files.len(),
        mib(total)
    );
    Ok(())
}

fn provenance(mut args: Args) -> Result<()> {
    let out = out_dir(&mut args)?;
    let toolchain = probe_toolchain(&mut args)?;
    args.finish()?;
    let bytes = std::fs::read(out.join(MANIFEST_FILE))
        .context(|| "read the built manifest (run build first)".to_owned())?;
    let manifest = Manifest::parse(&bytes)?;
    let provenance = Provenance::from_environment(&manifest.blake3()?, toolchain.versions);
    super::write_atomic(
        &out.join(PROVENANCE_FILE),
        &super::canonical_json(&provenance)?,
    )
}

fn sign(mut args: Args) -> Result<()> {
    let out = out_dir(&mut args)?;
    args.finish()?;
    sigstore::sign(
        &sigstore::program(),
        &out.join(MANIFEST_FILE),
        &out.join(BUNDLE_FILE),
    )
}

fn verify_signature(mut args: Args) -> Result<()> {
    let manifest = args.path("manifest")?;
    let bundle = args.path("bundle")?;
    let from_dir = args.path("from-dir")?;
    args.finish()?;
    let cosign = sigstore::program();
    if let (Some(manifest), Some(bundle)) = (&manifest, &bundle) {
        return sigstore::verify(&cosign, manifest, bundle, lock::IDENTITY, lock::ISSUER);
    }
    if manifest.is_some() || bundle.is_some() {
        return fail("--manifest and --bundle go together");
    }
    let lock = Lock::load(&repo_path(super::LOCK))?;
    let source = from_dir
        .map(|root| Source::Dir { root })
        .unwrap_or_else(|| Source::http(&lock.base_url));
    let (_, bytes) = pinned_manifest(&lock, &source)?;
    verify_pinned_signature(&lock, &source, &bytes)
}

fn pinned_manifest(lock: &Lock, source: &Source) -> Result<(Manifest, Vec<u8>)> {
    let Some(pinned) = &lock.manifest else {
        return fail(
            "test-corpus/lock.json is unpublished: no corpus revision has been published yet. \
             Generate fixtures locally with `cargo xtask test-corpus generate` instead.",
        );
    };
    let bytes = source
        .get(&lock::manifest_key(&pinned.blake3))?
        .ok_or_else(|| {
            error(format!(
                "the pinned manifest is missing from {}",
                source.describe()
            ))
        })?;
    let actual = super::blake3_bytes(&bytes);
    if actual != pinned.blake3 {
        return fail(format!(
            "the manifest served is {actual}, the lock pins {}",
            pinned.blake3
        ));
    }
    Ok((Manifest::parse(&bytes)?, bytes))
}

fn verify_pinned_signature(lock: &Lock, source: &Source, manifest_bytes: &[u8]) -> Result<()> {
    let digest = super::blake3_bytes(manifest_bytes);
    let bundle = source
        .get(&lock::bundle_key(&digest))?
        .ok_or_else(|| error("the manifest's Sigstore bundle is missing"))?;
    let dir = repo_path("target/test-corpus-verify");
    std::fs::create_dir_all(&dir).context(|| format!("create {}", dir.display()))?;
    let manifest_path = dir.join(format!("{digest}.json"));
    let bundle_path = dir.join(format!("{digest}.json.sigstore.json"));
    super::write_atomic(&manifest_path, manifest_bytes)?;
    super::write_atomic(&bundle_path, &bundle)?;
    sigstore::verify(
        &sigstore::program(),
        &manifest_path,
        &bundle_path,
        &lock.certificate_identity,
        &lock.certificate_oidc_issuer,
    )?;
    println!(
        "signature: manifest {digest} was signed by {}",
        lock.certificate_identity
    );
    Ok(())
}

fn env_required(name: &str) -> Result<String> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| error(format!("{name} is not set")))
}

fn publish(mut args: Args) -> Result<()> {
    let to_dir = args.path("to-dir")?;
    let r2 = args.switch("r2");
    let bucket = args.value("bucket")?;
    let public_url = args.value("public-url")?;
    let out = out_dir(&mut args)?;
    let root = root_arg(&mut args)?;
    let dry_run = args.switch("dry-run");
    let write_lock = args.switch("write-lock");
    args.finish()?;

    let (sink, readback) = match (to_dir, r2) {
        (Some(dir), false) => (Sink::Dir { root: dir.clone() }, Source::Dir { root: dir }),
        (None, true) => {
            let public = public_url
                .clone()
                .map_or_else(|| env_required("R2_PUBLIC_URL"), Ok)?;
            let bucket = bucket.map_or_else(|| env_required("R2_BUCKET_NAME"), Ok)?;
            let credentials = Credentials {
                access_key_id: env_required("R2_CORPUS_ACCESS_KEY")?,
                secret_access_key: env_required("R2_CORPUS_ACCESS_SECRET")?,
            };
            let endpoint = env_required("R2_CORPUS_ENDPOINT")?;
            let sink = Sink::R2(R2::new(
                Client::new(Policy::production()),
                &endpoint,
                &bucket,
                credentials,
            )?);
            (sink, Source::http(&public))
        }
        _ => return fail("publish needs exactly one of --to-dir DIR or --r2"),
    };
    let public_url = match public_url {
        Some(url) => url,
        None if r2 => env_required("R2_PUBLIC_URL")?,
        None if write_lock => {
            return fail("--write-lock needs --public-url for a directory publish");
        }
        None => String::new(),
    };

    // What is uploaded must be exactly what the ledger and the tree say now.
    let (manifest, rebuilt) = build_manifest()?;
    let staged = std::fs::read(out.join(MANIFEST_FILE))
        .context(|| "read the built manifest (run build first)".to_owned())?;
    if staged != rebuilt {
        return fail(
            "the built manifest no longer matches sources.json: rebuild before publishing",
        );
    }
    let digest = manifest.blake3()?;
    let provenance_bytes =
        std::fs::read(out.join(PROVENANCE_FILE)).context(|| "read provenance.json".to_owned())?;
    let provenance: Provenance =
        serde_json::from_slice(&provenance_bytes).context(|| "parse provenance.json".to_owned())?;
    if provenance.manifest_blake3 != digest {
        return fail("provenance.json describes another manifest");
    }
    let bundle_path = out.join(BUNDLE_FILE);
    if !bundle_path.is_file() {
        return fail("the manifest is not signed: run `test-corpus sign` in the publish workflow");
    }
    for file in &manifest.files {
        let (actual, size) = super::digest_file(&root.join(&file.path))?;
        if actual != file.blake3 || size != file.size {
            return fail(format!(
                "{}: the file on disk is not the one the manifest names",
                file.path
            ));
        }
    }

    let total: u64 = manifest.files.iter().map(|f| f.size).sum();
    println!(
        "publishing manifest {digest}: {} objects, {} to {}",
        manifest.files.len(),
        mib(total),
        sink.describe()
    );
    if dry_run {
        for file in &manifest.files {
            println!(
                "  would put {} <- {}",
                lock::object_key(&file.blake3),
                file.path
            );
        }
        println!("  would put {}", lock::provenance_key(&digest));
        println!("  would put {}", lock::bundle_key(&digest));
        println!("  would put {}", lock::manifest_key(&digest));
        return Ok(());
    }

    let mut created = 0usize;
    let mut existing = 0usize;
    let mut seen = BTreeSet::new();
    for file in &manifest.files {
        if !seen.insert(file.blake3.as_str()) {
            continue;
        }
        let key = lock::object_key(&file.blake3);
        match sink.put(
            &key,
            Body::File(&root.join(&file.path)),
            "application/octet-stream",
        )? {
            PutOutcome::Created => created += 1,
            PutOutcome::AlreadyExists => {
                // A content-addressed key that exists must hold those bytes.
                let check = repo_path("target/test-corpus-verify/readback");
                readback.fetch(&key, &check, &file.blake3, file.size)?;
                let _ = std::fs::remove_file(&check);
                existing += 1;
            }
        }
    }
    println!("  objects: {created} uploaded, {existing} already present and verified");

    // Provenance and bundle are keyed by the manifest digest. If an earlier,
    // interrupted run of this same manifest already wrote them, adopt the
    // existing ones after checking them, rather than overwrite.
    let provenance_key = lock::provenance_key(&digest);
    let provenance_blake3 = match sink.put(
        &provenance_key,
        Body::Bytes(&provenance_bytes),
        "application/json",
    )? {
        PutOutcome::Created => super::blake3_bytes(&provenance_bytes),
        PutOutcome::AlreadyExists => {
            let bytes = readback
                .get(&provenance_key)?
                .ok_or_else(|| error("provenance exists but cannot be read back"))?;
            let earlier: Provenance = serde_json::from_slice(&bytes)
                .context(|| "parse the existing provenance".to_owned())?;
            if earlier.manifest_blake3 != digest {
                return fail("the existing provenance names another manifest");
            }
            println!("  provenance: adopted the one an earlier run published");
            super::blake3_bytes(&bytes)
        }
    };
    let bundle_key = lock::bundle_key(&digest);
    if sink.put(&bundle_key, Body::File(&bundle_path), "application/json")?
        == PutOutcome::AlreadyExists
    {
        println!("  signature: a bundle already exists; verifying it");
    }
    let manifest_key = lock::manifest_key(&digest);
    if sink.put(&manifest_key, Body::Bytes(&rebuilt), "application/json")?
        == PutOutcome::AlreadyExists
    {
        let bytes = readback
            .get(&manifest_key)?
            .ok_or_else(|| error("manifest exists but cannot be read back"))?;
        if bytes != rebuilt {
            return fail("a different manifest is stored under this digest");
        }
        println!("  manifest: already published");
    }

    let published = Lock::published(
        if public_url.is_empty() {
            "https://unset.invalid"
        } else {
            &public_url
        },
        &digest,
        &provenance_blake3,
        PublishedFrom {
            commit: provenance.commit.clone(),
            run: provenance.run_url.clone(),
        },
    );
    published.validate()?;
    // Read the revision back the way hydrate will and check the signature
    // that is now stored, whichever run wrote it.
    let (_, served) = pinned_manifest(&published, &readback)?;
    verify_pinned_signature(&published, &readback, &served)?;
    if write_lock {
        super::write_atomic(&repo_path(super::LOCK), &super::canonical_json(&published)?)?;
        println!(
            "  wrote {}: commit it together with {}",
            super::LOCK,
            super::SOURCES
        );
    }
    println!("published manifest {digest}");
    Ok(())
}

fn hydrate(mut args: Args) -> Result<()> {
    let mut profiles = args.values("profile")?;
    let lock_path = args.path("lock")?.unwrap_or_else(|| repo_path(super::LOCK));
    let root = root_arg(&mut args)?;
    let from_dir = args.path("from-dir")?;
    let check_signature = args.switch("verify-signature");
    args.finish()?;
    if profiles.is_empty() {
        profiles.push("unit".to_owned());
    }
    let lock = Lock::load(&lock_path)?;
    let source = from_dir
        .map(|root| Source::Dir { root })
        .unwrap_or_else(|| Source::http(&lock.base_url));
    let (manifest, bytes) = pinned_manifest(&lock, &source)?;
    if check_signature {
        verify_pinned_signature(&lock, &source, &bytes)?;
    }
    let selected = manifest.select(&profiles)?;
    let started = Instant::now();
    let (mut fetched, mut present, mut bytes_fetched) = (0usize, 0usize, 0u64);
    for file in &selected {
        let destination = root.join(&file.path);
        if destination.is_file()
            && let Ok((digest, size)) = super::digest_file(&destination)
            && digest == file.blake3
            && size == file.size
        {
            present += 1;
            continue;
        }
        source.fetch(
            &lock::object_key(&file.blake3),
            &destination,
            &file.blake3,
            file.size,
        )?;
        fetched += 1;
        bytes_fetched += file.size;
    }
    println!(
        "hydrated {} ({} files: {fetched} fetched, {}, {present} already present) in {:.1}s",
        profiles.join("+"),
        selected.len(),
        mib(bytes_fetched),
        started.elapsed().as_secs_f64()
    );
    synthesize_selected(&profiles, &root, &manifest, &source)
}

/// Write the synthesised fixtures the profiles select from their seeds,
/// fetching a seed the selection did not already bring. Each is checked
/// against the size and digest its recipe pins.
fn synthesize_selected(
    profiles: &[String],
    root: &Path,
    manifest: &Manifest,
    source: &Source,
) -> Result<()> {
    let ledger = Ledger::load(&repo_path(super::SOURCES))?;
    let profile_file = ProfilesFile::load(&repo_path(super::PROFILES))?;
    let fixtures = recipes::all();
    let mut entries: Vec<&ledger::FileEntry> = Vec::new();
    for name in profiles {
        for entry in profile_file.select_synthesized(name, &ledger)? {
            if !entries.iter().any(|e| e.path == entry.path) {
                entries.push(entry);
            }
        }
    }
    for entry in entries {
        let started = Instant::now();
        let ledger::Source::Synthesized { seed, .. } = &entry.source else {
            continue;
        };
        let (Some(size), Some(blake3)) = (entry.size, entry.blake3.as_deref()) else {
            return fail(format!("{} pins no digest", entry.path));
        };
        let destination = root.join(&entry.path);
        if destination.is_file()
            && let Ok((digest, length)) = super::digest_file(&destination)
            && digest == blake3
            && length == size
        {
            continue;
        }
        let Some(seed_file) = manifest.files.iter().find(|f| &f.path == seed) else {
            return fail(format!(
                "{} seeds from {seed}, which the manifest does not list",
                entry.path
            ));
        };
        let seed_path = root.join(seed);
        let seed_present = seed_path.is_file()
            && super::digest_file(&seed_path)
                .is_ok_and(|(d, s)| d == seed_file.blake3 && s == seed_file.size);
        if !seed_present {
            source.fetch(
                &lock::object_key(&seed_file.blake3),
                &seed_path,
                &seed_file.blake3,
                seed_file.size,
            )?;
        }
        let Some(recipes::Step::Synthesize { synth, .. }) = fixtures
            .iter()
            .find(|f| f.path() == entry.path)
            .and_then(|f| f.steps.first())
        else {
            return fail(format!("{} has no synthesis recipe", entry.path));
        };
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).context(|| format!("create {}", parent.display()))?;
        }
        let staged = super::temporary_sibling(&destination);
        let data = recipes::synth::synthesize(*synth, &seed_path, &staged, Some((size, blake3)))?;
        super::rename_into_place(&staged, &destination)?;
        println!(
            "synthesised {} ({}, {} of it data) in {:.1}s",
            entry.path,
            mib(size),
            mib(data),
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

fn paths(mut args: Args) -> Result<()> {
    let profile = args.value("profile")?.unwrap_or_else(|| "unit".to_owned());
    let root = root_arg(&mut args)?;
    let existing = args.switch("existing");
    args.finish()?;
    let ledger = Ledger::load(&repo_path(super::SOURCES))?;
    let profiles = ProfilesFile::load(&repo_path(super::PROFILES))?;
    for path in profiles.select(&profile, &ledger)? {
        let full = root.join(path);
        if !existing || full.is_file() {
            println!("{}", full.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_parse_in_both_spellings_and_leftovers_fail() {
        let mut args = Args::new(
            [
                "--fixture",
                "a.mkv",
                "--fixture=b.mp4",
                "--check",
                "--root",
                "/r",
            ]
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        );
        assert_eq!(args.values("fixture").unwrap(), vec!["a.mkv", "b.mp4"]);
        assert!(args.switch("check"));
        assert!(!args.switch("check"));
        assert_eq!(args.value("root").unwrap().as_deref(), Some("/r"));
        args.finish().unwrap();
        let mut bad = Args::new(vec!["--root".into(), "--check".into()]);
        assert!(bad.value("root").is_err());
        assert!(Args::new(vec!["stray".into()]).finish().is_err());
    }

    #[test]
    fn hydrate_refuses_an_unpublished_lock() {
        let lock = Lock::load(&repo_path(super::super::LOCK)).unwrap();
        if lock.is_unpublished() {
            let source = Source::Dir {
                root: std::env::temp_dir(),
            };
            let refused = pinned_manifest(&lock, &source).unwrap_err().to_string();
            assert!(refused.contains("unpublished"));
        }
    }

    #[test]
    fn committed_expectations_are_current() {
        expectations(Args::new(vec!["--check".into()])).unwrap();
    }
}
