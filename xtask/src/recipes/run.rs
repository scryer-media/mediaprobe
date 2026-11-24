//! Running recipes.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{Fixture, Step, builders, post};
use crate::corpus::{Context as _, Result, fail};

pub(crate) struct Tools {
    pub(crate) ffmpeg: PathBuf,
    pub(crate) speexenc: PathBuf,
}

/// Generate one fixture into `media_root` (which maps to tests/media). The
/// fixture is written through a temporary name and renamed into place, so a
/// failed recipe never leaves a partial file that looks finished.
pub(crate) fn generate(
    fixture: &Fixture,
    media_root: &Path,
    scratch: &Path,
    tools: &Tools,
) -> Result<PathBuf> {
    let out = media_root.join(&fixture.name);
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).context(|| format!("create {}", parent.display()))?;
    }
    let work = scratch.join(fixture.name.replace('/', "__"));
    if work.exists() {
        std::fs::remove_dir_all(&work).context(|| format!("clear {}", work.display()))?;
    }
    std::fs::create_dir_all(&work).context(|| format!("create {}", work.display()))?;
    let staged = crate::corpus::temporary_sibling(&out);
    let substitute = |arg: &str| {
        arg.replace("{out}", &staged.to_string_lossy())
            .replace("{work}", &work.to_string_lossy())
            .replace("{media}", &media_root.to_string_lossy())
    };
    let result = (|| -> Result<()> {
        for step in &fixture.steps {
            match step {
                Step::Ffmpeg(arguments) => {
                    let mut command = Command::new(&tools.ffmpeg);
                    command.args(["-y", "-hide_banner", "-loglevel", "error", "-nostdin"]);
                    command.args(arguments.iter().map(|a| substitute(a)));
                    run(&mut command, &work)?;
                }
                Step::Speexenc(arguments) => {
                    let mut command = Command::new(&tools.speexenc);
                    command.args(arguments.iter().map(|a| substitute(a)));
                    run(&mut command, &work)?;
                }
                Step::WriteFile { name, contents } => {
                    let path = work.join(name);
                    std::fs::write(&path, contents)
                        .context(|| format!("write {}", path.display()))?;
                }
                Step::Post(pass) => post::apply(pass, &staged, media_root, &work)?,
                Step::Build(builder) => {
                    let bytes = builders::build(*builder, &work)?;
                    std::fs::write(&staged, bytes)
                        .context(|| format!("write {}", staged.display()))?;
                }
                Step::Fetch { url, size, blake3 } => {
                    crate::corpus::http::Client::new(crate::corpus::http::Policy::production())
                        .get_verified(url, &staged, blake3, *size)?;
                }
                Step::Synthesize {
                    synth,
                    seed,
                    size,
                    blake3,
                } => {
                    let _data = super::synth::synthesize(
                        *synth,
                        &media_root.join(seed),
                        &staged,
                        (!blake3.is_empty()).then_some((*size, *blake3)),
                    )?;
                }
            }
        }
        if !staged.is_file() {
            return fail("the recipe wrote no output");
        }
        crate::corpus::rename_into_place(&staged, &out)
    })();
    let _ = std::fs::remove_dir_all(&work);
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    result.map(|()| out)
}

/// A recipe step that runs longer than this is hung (every fixture is a
/// second or two of media), so it is killed and the fixture fails.
const STEP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

fn run(command: &mut Command, work: &Path) -> Result<()> {
    // stderr goes to a file, so a chatty tool cannot block on a full pipe
    // while it is being waited on.
    let log = work.join(".stderr");
    let stderr = std::fs::File::create(&log).context(|| format!("create {}", log.display()))?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .context(|| format!("start {:?}", command.get_program()))?;
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .context(|| format!("wait for {:?}", command.get_program()))?
        {
            break Some(status);
        }
        if started.elapsed() > STEP_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let stderr = std::fs::read_to_string(&log).unwrap_or_default();
    let tail: Vec<&str> = stderr.lines().rev().take(8).collect();
    let tail = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
    match status {
        Some(status) if status.success() => Ok(()),
        Some(status) => fail(format!(
            "{:?} exited with {status}:\n{tail}",
            command.get_program()
        )),
        None => fail(format!(
            "{:?} was killed after {} s:\n{tail}",
            command.get_program(),
            STEP_TIMEOUT.as_secs()
        )),
    }
}
