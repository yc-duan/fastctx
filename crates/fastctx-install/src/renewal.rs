//! Durable local executable replacement. Images are immutable and the intent
//! is flushed before the installed name changes. No World records are edited.

use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use fastctx_platform::{FileLock, Permissions, ensure_private_dir, publish};
use fastctx_types::renewal::{Build, Phase};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub const EXECUTABLE: &str = if cfg!(windows) { "fastctx.exe" } else { "fastctx" };

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Program {
    pub path: PathBuf,
    pub build: Build,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    pub candidate: Program,
    pub previous: Option<Program>,
    pub phase: Phase,
    pub error: Option<String>,
}

pub fn directory(root: &Path) -> PathBuf {
    root.join("renewal")
}

pub fn installed(root: &Path) -> PathBuf {
    root.join("bin").join(EXECUTABLE)
}

pub fn digest(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    io::copy(&mut file, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

pub fn build() -> io::Result<Build> {
    Ok(Build {
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: fastctx_types::ipc::PROTOCOL,
        digest: digest(&std::env::current_exe()?)?,
    })
}

pub fn read(root: &Path) -> io::Result<Option<Plan>> {
    read_json(&directory(root).join("plan.json"))
}

pub fn save(root: &Path, plan: &Plan) -> io::Result<()> {
    write_json(&directory(root).join("plan.json"), plan)
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    publish(path, &serde_json::to_vec(value).map_err(io::Error::other)?, Permissions::Private)
}

/// Stores and checks the new program before publishing either the intent or
/// the installed name. Repeating an unfinished placement retains its state.
pub fn stage(root: &Path, source: &Path, running: Option<&Path>) -> anyhow::Result<Plan> {
    anyhow::ensure!(!root.join("uninstalled.json").try_exists()?, "this World was uninstalled; reinstall it before placing a program");
    ensure_private_dir(&directory(root))?;
    let _lock = FileLock::acquire(&directory(root).join("worker.lock"))?;
    let copied = copy_image(root, source)?;
    let output = command(&copied).args(["renew", "identify"]).output()?;
    anyhow::ensure!(output.status.success(), "candidate could not identify itself: {}", output.status);
    let build: Build = serde_json::from_slice(&output.stdout)?;
    anyhow::ensure!(build.digest == digest(&copied)?, "candidate image changed while being checked");
    if let Some(plan) = read(root)?
        && plan.candidate.build == build
    {
        install(root, &plan.candidate)?;
        return Ok(plan);
    }
    let previous_path = running.map(Path::to_path_buf).or_else(|| installed(root).is_file().then(|| installed(root)));
    let previous = if let Some(previous_path) = previous_path {
        let path = copy_image(root, &previous_path)?;
        let output = command(&path).arg("--version").output()?;
        anyhow::ensure!(output.status.success(), "previous program cannot report its version");
        let version = String::from_utf8(output.stdout)?;
        let version = version.trim().strip_prefix("fastctx ").ok_or_else(|| anyhow::anyhow!("not fastctx"))?;
        Some(Program { build: Build { version: version.into(), protocol: 0, digest: digest(&path)? }, path })
    } else {
        None
    };
    let plan = Plan { candidate: Program { path: copied, build }, previous, phase: Phase::Staged, error: None };
    save(root, &plan)?;
    install(root, &plan.candidate)?;
    Ok(plan)
}

pub fn verify(program: &Program) -> io::Result<()> {
    if digest(&program.path)? != program.build.digest {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "staged executable digest differs"));
    }
    Ok(())
}

pub fn install(root: &Path, program: &Program) -> io::Result<()> {
    verify(program)?;
    ensure_private_dir(&root.join("bin"))?;
    let _lock = FileLock::acquire(&root.join("apply.lock"))?;
    publish_image(&program.path, &installed(root))
}

/// Keeps a hard link to a mapped Windows image before replacing its name.
/// Without it NTFS refuses replacing the last name of an executable in use.
pub fn publish_image(source: &Path, target: &Path) -> io::Result<()> {
    use crate::transaction::{FileChange, Snapshot, Transaction};
    let before = Snapshot::read(target).map_err(io::Error::other)?;
    let copy = FileChange::copy_executable(before, source).map_err(io::Error::other)?;
    let receipt = Transaction::new([copy]).commit().map_err(io::Error::other)?;
    // Mapped images are intentionally retained; the existing cleanup helper
    // retries recognized links at the next placement. Immutable rollback
    // images live separately in renewal/images.
    let retained = crate::executable::cleanup_retired(target);
    write_json(
        &target.with_extension("renewal-files.json"),
        &serde_json::json!({
            "preserved": receipt.preserved,
            "cleanup": receipt.cleanup.iter().chain(&retained).map(ToString::to_string).collect::<Vec<_>>(),
        }),
    )
}

/// Requests progress in a separate user process; closing the interface does
/// not cancel an idle handover. The worker serializes concurrent requests.
/// Only an explicit placement, retry, or approval retries a known failure.
pub fn request(root: &Path, retry: bool) -> anyhow::Result<()> {
    if let Some(plan) = read(root)? {
        verify(&plan.candidate)?;
        let mut args = vec!["renew".into(), "run".into(), "--dir".into(), root.into()];
        if retry {
            args.push("--retry".into());
        }
        launch(root, &plan.candidate.path, &args)?;
    }
    Ok(())
}

pub fn launch(
    root: &Path,
    program: &Path,
    args: &[std::ffi::OsString],
) -> io::Result<fastctx_platform::DetachedProcess> {
    let mut environment = fastctx_platform::world_process_environment()?;
    environment.retain(|(name, _)| !name.eq_ignore_ascii_case("FASTCTX_HOME"));
    environment.push(("FASTCTX_HOME".into(), root.into()));
    if let Some(home) = std::env::var_os("CODEX_HOME") {
        environment.retain(|(name, _)| !name.eq_ignore_ascii_case("CODEX_HOME"));
        environment.push(("CODEX_HOME".into(), home));
    }
    fastctx_platform::spawn_detached(
        program,
        &args.iter().map(|arg| arg.as_os_str()).collect::<Vec<_>>(),
        root,
        &environment,
    )
}

fn copy_image(root: &Path, source: &Path) -> io::Result<PathBuf> {
    let hash = digest(source)?;
    let folder = directory(root).join("images").join(&hash);
    ensure_private_dir(&folder)?;
    let path = folder.join(EXECUTABLE);
    if !path.try_exists()? {
        copy_to(source, &path)?;
    }
    if digest(&path)? != hash {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "immutable executable digest differs"));
    }
    Ok(path)
}

fn copy_to(source: &Path, target: &Path) -> io::Result<()> {
    if target.try_exists()? && digest(source)? == digest(target)? {
        return Ok(());
    }
    let mut input = fs::File::open(source)?;
    let mut staged = fastctx_platform::atomic::Staged::create(target, Some(0o700))?;
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        staged.write_all(&buffer[..count])?;
    }
    staged.commit()
}

fn command(program: &Path) -> Command {
    let mut command = Command::new(program);
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x08000000);
    }
    command
}
