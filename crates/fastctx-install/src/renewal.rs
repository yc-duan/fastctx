//! Durable local executable replacement. Images are immutable and the intent
//! is flushed before the installed name changes. No World records are edited.

use anyhow::Context as _;
use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

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
    /// Recovery checks this build against the stable program. The path is
    /// where it was observed, not an extra executable kept for rollback.
    pub previous: Option<Program>,
    pub phase: Phase,
    pub error: Option<String>,
    #[serde(default)]
    pub error_version: Option<String>,
    #[serde(default)]
    pub retry: fastctx_update::retry::Retry,
    #[serde(default)]
    pub keeper_restarted: bool,
    /// A repair may replace an unreadable image, after checking all known versions.
    #[serde(default)]
    pub repair: bool,
}

pub fn directory(root: &Path) -> PathBuf {
    root.join("renewal")
}

pub fn installed(root: &Path) -> PathBuf {
    root.join("bin").join(EXECUTABLE)
}

/// The SHA-256 of a file. Every command asks it of the World program and of
/// itself, and hashing a program costs seconds in a debug build, so the
/// answer is kept under fastctx's own directory beside the file's snapshot;
/// a file replaced or written since has another snapshot and is hashed
/// again. Keeping it is only a saving: where it cannot be kept, the digest
/// is still exact.
pub fn digest(path: &Path) -> io::Result<String> {
    let mut file = fastctx_platform::file_read::open(path)?;
    let metadata = file.metadata()?;
    let mark = fastctx_platform::file_read::snapshot(&file, &metadata)?.fingerprint();
    let kept = kept_digest(path);
    if let Some(kept) = &kept
        && let Ok(text) = fs::read_to_string(kept)
        && let Some((held, digest)) = text.trim().split_once(' ')
        && held == mark
        && digest.len() == 64
        && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Ok(digest.to_string());
    }
    let mut hash = Sha256::new();
    io::copy(&mut file, &mut hash)?;
    let digest = format!("{:x}", hash.finalize());
    // Kept only when the file did not change while it was read.
    let after = fastctx_platform::file_read::snapshot(&file, &file.metadata()?)?.fingerprint();
    if let Some(kept) = kept
        && after == mark
        && let Some(parent) = kept.parent()
        && fs::create_dir_all(parent).is_ok()
    {
        let _ = publish(&kept, format!("{mark} {digest}\n").as_bytes(), Permissions::Private);
    }
    Ok(digest)
}

/// Where the digest of `path` is kept: one small file per path, named by the
/// hash of the path's canonical spelling. Only in a World that is there: a
/// program asking as another account, such as the service's guardian
/// running as SYSTEM, would otherwise make a fastctx directory in that
/// account's profile, which nothing reads and no removal knows of.
fn kept_digest(path: &Path) -> Option<PathBuf> {
    let canonical = fs::canonicalize(path).ok()?;
    let root = fastctx_platform::fastctx_root().ok()?;
    if !root.is_dir() {
        return None;
    }
    let name = format!("{:x}", Sha256::digest(canonical.as_os_str().as_encoded_bytes()));
    Some(root.join("cache").join("digests").join(name))
}

pub fn build() -> io::Result<Build> {
    static DIGEST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let digest = match DIGEST.get() {
        Some(digest) => digest.clone(),
        None => {
            let digest = own_digest()?;
            let _ = DIGEST.set(digest.clone());
            digest
        }
    };
    Ok(Build {
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: fastctx_types::ipc::PROTOCOL,
        digest,
        built: option_env!("FASTCTX_BUILT").and_then(|built| built.parse().ok()),
    })
}

fn own_digest() -> io::Result<String> {
    #[cfg(target_os = "linux")]
    return digest(Path::new("/proc/self/exe"));
    #[cfg(not(target_os = "linux"))]
    digest(&std::env::current_exe()?)
}

pub fn read(root: &Path) -> io::Result<Option<Plan>> {
    read_json(&directory(root).join("plan.json"))
}

pub fn save(root: &Path, plan: &Plan) -> io::Result<()> {
    write_json(&directory(root).join("plan.json"), plan)
}

/// Local maintenance facts remain available independently of business IPC.
pub async fn status(root: &Path) -> anyhow::Result<fastctx_types::renewal::Status> {
    use fastctx_client::renewal::Probe;
    let plan = read(root)?;
    let (node, mismatch) = match fastctx_client::renewal::probe(&fastctx_types::layout::WorldDir::under(root)).await? {
        Probe::Running(node) => (Some(node), None),
        Probe::Legacy { mismatch, .. } => (None, Some(mismatch)),
        Probe::Absent => (None, None),
    };
    let root_for_files = root.to_owned();
    let (client, installed, commands) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let client = Some(build()?);
        let installed_path = self::installed(&root_for_files);
        let installed =
            if installed_path.try_exists()? { Some(crate::program::inspect(&installed_path)?) } else { None };
        Ok((client, installed, crate::path_command::inspect(&root_for_files)))
    })
    .await??;
    let phase = plan.as_ref().map(|plan| {
        if plan.phase == Phase::Complete
            && (mismatch.is_some() || node.as_ref().is_some_and(|node| node.build != plan.candidate.build))
        {
            Phase::Staged
        } else {
            plan.phase.clone()
        }
    });
    Ok(fastctx_types::renewal::Status {
        client,
        installed,
        commands,
        node,
        mismatch,
        pending: plan.as_ref().filter(|_| phase != Some(Phase::Complete)).map(|plan| plan.candidate.build.clone()),
        phase,
        error: plan.and_then(|plan| plan.error),
    })
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| at("decoding renewal record", path, io::Error::other(e))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(at("reading renewal record", path, error)),
    }
}

pub fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| at("encoding renewal record", path, io::Error::other(e)))?;
    publish(path, &bytes, Permissions::Private).map_err(|e| at("publishing renewal record", path, e))
}

/// Held by program collection around the renewal's own locks.
pub fn collection_lock(root: &Path) -> PathBuf {
    directory(root).join("collection.lock")
}

/// How long a placement waits for a node to hand the World to the successor
/// waiting for it.
const HANDOVER: std::time::Duration = std::time::Duration::from_secs(60);

/// Waits out a handover to a successor waiting for the World, the entry
/// lock once it serves; none when no successor waits, or it does not serve
/// within `HANDOVER`. The node serving now is asked to yield when it is not
/// yielding already: whoever installed the service may not have reached it,
/// and the two would wait for each other for good.
fn handed_over(root: &Path, entry: &Path) -> anyhow::Result<Option<FileLock>> {
    let world = fastctx_types::layout::WorldDir::under(root);
    if !world.node_successor().try_exists()? {
        return Ok(None);
    }
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    if let Ok(fastctx_client::renewal::Probe::Running(node)) = runtime.block_on(fastctx_client::renewal::probe(&world))
        && !node.yielding
    {
        // Not answered, the successor still waits: the wait below says so.
        let _ = runtime.block_on(fastctx_client::renewal::request(&world, fastctx_types::renewal::Action::Yield));
    }
    let began = std::time::Instant::now();
    while began.elapsed() < HANDOVER {
        if let Some(lock) = FileLock::try_acquire(entry)? {
            return Ok(Some(lock));
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Ok(None)
}

/// Where the process coordinating the handover to the program `digest`
/// says who it is (`fastctx_node::replacement::prepare`).
pub fn coordinator(root: &Path, digest: &str) -> PathBuf {
    directory(root).join(format!("coordinator-{digest}.json"))
}

/// A handover its boot service never took keeps its gate for good: the
/// process coordinating it holds the gate for as long as the stable node it
/// started serves, and that node serves on while the service refuses its
/// program, as an earlier release's guardian refused one without its own
/// signature; an earlier release also left its coordinator holding the gate
/// after it counted its handover complete. A newer program offered, with no
/// handover going on (the worker's gate is held by this staging), ends every
/// coordinator still alive for another program, which by then only holds
/// the gate, its node, detached from it, going on serving; and takes the
/// gate, which nothing else ever would. The gate once they have let it go;
/// none where no such handover holds it.
pub(crate) fn superseded(
    root: &Path,
    entry: &Path,
    source: &Path,
    repair: bool,
    mut ending: impl FnMut(&fastctx_types::ProcessIdentity) -> anyhow::Result<()>,
) -> anyhow::Result<Option<FileLock>> {
    let plan = read(root)?;
    let stuck = plan.as_ref().is_none_or(|plan| {
        matches!(plan.phase, Phase::Complete | Phase::AwaitingAdministrator | Phase::Failed)
            || (plan.phase == Phase::UpdatingGuardian && plan.error.is_some())
    });
    if !stuck {
        return Ok(None);
    }
    let offered = crate::program::inspect(source)?;
    if let Some(plan) = &plan
        && plan.candidate.build != offered
        && !crate::program::newer(&offered, &plan.candidate.build)?
    {
        return Ok(None);
    }
    let mut holders = Vec::new();
    for found in fs::read_dir(directory(root))? {
        let name = found?.file_name();
        let Some(digest) = name.to_str().and_then(|name| name.strip_prefix("coordinator-")?.strip_suffix(".json"))
        else {
            continue;
        };
        if digest == offered.digest && !repair {
            continue;
        }
        if let Some(holder) = read_json::<fastctx_types::ProcessIdentity>(&coordinator(root, digest))?
            && fastctx_platform::is_alive(&holder)?
        {
            holders.push(holder);
        }
    }
    if holders.is_empty() {
        return Ok(None);
    }
    for holder in &holders {
        ending(holder)?;
        fastctx_platform::terminate::terminate(holder)
            .with_context(|| format!("ending superseded coordinator {} for {}", holder.pid, entry.display()))?;
    }
    let began = std::time::Instant::now();
    while began.elapsed() < HANDOVER {
        if let Some(lock) = FileLock::try_acquire(entry)? {
            return Ok(Some(lock));
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Ok(None)
}

/// Takes the program out of a release's checked package, its own signature
/// beside it (`fastctx_update::kernel::unpack`), and stages it, unpacking in
/// `unpacking`, a directory of the caller's emptied first and after.
pub fn stage_package(root: &Path, package: &Path, unpacking: &Path) -> anyhow::Result<Plan> {
    let _ = fs::remove_dir_all(unpacking);
    fs::create_dir_all(unpacking)?;
    let staged = fastctx_update::kernel::unpack(package, unpacking)
        .map_err(anyhow::Error::msg)
        .and_then(|program| stage(root, &program, None));
    let _ = fs::remove_dir_all(unpacking);
    staged
}

/// Stores and checks the new program before publishing either the intent or
/// the installed name. Repeating an unfinished placement retains its state.
pub fn stage(root: &Path, source: &Path, _running: Option<&Path>) -> anyhow::Result<Plan> {
    stage_inner(root, source, false).with_context(|| format!("staging {} for {}", source.display(), root.display()))
}

/// Restages a checked repair without discarding the previous durable intent.
pub fn stage_repair(root: &Path, source: &Path) -> anyhow::Result<Plan> {
    crate::repair::records::check_version(root, &crate::program::inspect(source)?)?;
    stage_inner(root, source, true)
        .with_context(|| format!("staging repair {} for {}", source.display(), root.display()))
}

fn stage_inner(root: &Path, source: &Path, repair: bool) -> anyhow::Result<Plan> {
    anyhow::ensure!(
        !root.join("uninstalled.json").try_exists()?,
        "this World was uninstalled; reinstall it before placing a program"
    );
    ensure_private_dir(&directory(root))
        .with_context(|| format!("preparing renewal directory {}", directory(root).display()))?;
    let worker = directory(root).join("worker.lock");
    let _lock = match FileLock::try_acquire(&worker)
        .with_context(|| format!("opening renewal worker gate {}", worker.display()))?
    {
        Some(lock) => lock,
        // Program collection holds the worker lock only inside the collection
        // lock: waiting for that one waits out a collection, and while it is
        // held no collection begins, so a worker lock still taken then is a
        // renewal's.
        None => {
            let _collection = FileLock::acquire(&collection_lock(root))?;
            FileLock::try_acquire(&worker)
                .with_context(|| format!("opening renewal worker gate {}", worker.display()))?
                .ok_or_else(|| {
                    anyhow::anyhow!("the World program is already being replaced; its plan was left unchanged")
                })?
        }
    };
    let entry = directory(root).join("node-entry.lock");
    let _node_entry = match FileLock::try_acquire(&entry).with_context(|| format!("opening node entry gate {}", entry.display()))? {
        Some(lock) => lock,
        None => match handed_over(root, &entry)? {
            Some(lock) => lock,
            None => superseded(root, &entry, source, false, |_| Ok(()))?.ok_or_else(|| {
                anyhow::anyhow!(
                    "the node is completing its program handover; it was asked to finish it, and its plan was left unchanged"
                )
            })?,
        },
    };
    let _activation = FileLock::try_acquire(&directory(root).join("activation.lock"))?.ok_or_else(|| {
        anyhow::anyhow!("the World program is starting or being published; its plan was left unchanged")
    })?;
    let world = fastctx_types::layout::WorldDir::under(root);
    ensure_private_dir(world.path())
        .with_context(|| format!("preparing World directory {}", world.path().display()))?;
    let stable = installed(root);
    let mut source = source;
    if stable.is_file()
        && let Some(current) = installed_build(&stable, repair)?
    {
        let offered = crate::program::inspect(source)?;
        if current != offered && !crate::program::newer(&offered, &current)? {
            source = &stable;
        }
    }
    let copied = if repair {
        let digest = digest(source)?;
        let candidate = directory(root).join("images").join(&digest).join(EXECUTABLE);
        if candidate.is_file() && self::digest(&candidate)? != digest {
            publish_image(source, &candidate)?;
        }
        copy_image(root, source)?
    } else {
        copy_image(root, source)?
    };
    let build = crate::program::inspect(&copied)?;
    anyhow::ensure!(build.protocol != 0, "the offered program has no World kernel identity");
    let old_plan = read(root)?;
    if let Some(plan) = &old_plan {
        if !repair && plan.candidate.build == build && plan.phase != Phase::Complete {
            return Ok(plan.clone());
        }
        anyhow::ensure!(
            plan.candidate.build == build || crate::program::newer(&build, &plan.candidate.build)?,
            "the offered kernel {} is older than the recorded renewal {}; the plan was left unchanged",
            build.version,
            plan.candidate.build.version
        );
    }
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let serving = runtime
        .block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(30), fastctx_client::renewal::probe(&world)).await
        })
        .with_context(|| format!("waiting for renewal status under {} exceeded 30 seconds", root.display()))??;
    let installed_build = installed_build(&installed(root), repair)?;
    if let Some(installed) = &installed_build {
        anyhow::ensure!(
            installed == &build || crate::program::newer(&build, installed)?,
            "offered kernel {} is older than installed {}; the World program was left unchanged",
            build.version,
            installed.version
        );
    }
    let previous_path = match &serving {
        fastctx_client::renewal::Probe::Running(node) => Some(node.executable.clone().into()),
        fastctx_client::renewal::Probe::Legacy { .. } => runtime.block_on(fastctx_client::renewal::image(&world))?,
        fastctx_client::renewal::Probe::Absent => installed_build.as_ref().map(|_| installed(root)),
    };
    let previous = if let Some(previous_path) = previous_path {
        let previous = match (&serving, repair) {
            (fastctx_client::renewal::Probe::Running(node), true) => node.build.clone(),
            _ => crate::program::inspect(&previous_path)?,
        };
        if let fastctx_client::renewal::Probe::Running(node) = &serving {
            anyhow::ensure!(
                previous == node.build,
                "the serving node's image cannot be identified; its plan was left unchanged"
            );
        }
        anyhow::ensure!(
            previous == build || crate::program::newer(&build, &previous)?,
            "offered kernel {} is older than installed {}; the World program was left unchanged",
            build.version,
            previous.version
        );
        Some(Program { build: previous, path: previous_path })
    } else {
        None
    };
    // A plan for another program starts with nothing failed: what went
    // wrong replacing an earlier one is not this one's to say.
    let mut plan = Plan {
        candidate: Program { path: copied, build },
        previous,
        phase: Phase::Staged,
        error: None,
        error_version: None,
        retry: fastctx_update::retry::Retry::default(),
        keeper_restarted: false,
        repair,
    };
    save(root, &plan)?;
    if !world.node_service().exists()
        && matches!(serving, fastctx_client::renewal::Probe::Absent)
        && let Some(_world) = FileLock::try_acquire(&world.node_lock())?
    {
        install_plan(root, &plan)?;
        plan.phase = Phase::Complete;
        plan.error = None;
        plan.error_version = None;
        save(root, &plan)?;
    } else if installed_build.as_ref() == Some(&plan.candidate.build)
        && plan.previous.as_ref().is_some_and(|previous| previous.build == plan.candidate.build)
        && match &serving {
            fastctx_client::renewal::Probe::Running(node) => {
                std::fs::canonicalize(&node.executable)? == std::fs::canonicalize(installed(root))?
            }
            fastctx_client::renewal::Probe::Absent => true,
            fastctx_client::renewal::Probe::Legacy { .. } => false,
        }
        && old_plan.as_ref().is_none_or(|old| old.phase == Phase::Complete)
    {
        plan.phase = Phase::Complete;
        plan.error = None;
        plan.error_version = None;
        save(root, &plan)?;
    }
    Ok(plan)
}

pub fn verify(program: &Program) -> io::Result<()> {
    if digest(&program.path)? != program.build.digest {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "staged executable digest differs"));
    }
    Ok(())
}

pub fn install(root: &Path, program: &Program) -> io::Result<()> {
    install_inner(root, program, false)
}

/// Publishes a plan using the version bounds verified by its staging process.
pub fn install_plan(root: &Path, plan: &Plan) -> io::Result<()> {
    install_inner(root, &plan.candidate, plan.repair)
}

fn installed_build(path: &Path, repair: bool) -> anyhow::Result<Option<Build>> {
    if !path.try_exists()? {
        return Ok(None);
    }
    match crate::program::inspect(path) {
        Ok(build) => Ok(Some(build)),
        Err(_) if repair => Ok(None),
        Err(error) => Err(error),
    }
}

fn install_inner(root: &Path, program: &Program, repair: bool) -> io::Result<()> {
    verify(program)?;
    ensure_private_dir(&root.join("bin")).map_err(|e| at("preparing executable directory", &root.join("bin"), e))?;
    let _lock = FileLock::acquire(&root.join("apply.lock"))?;
    let installed = installed(root);
    if let Some(current) = installed_build(&installed, repair).map_err(io::Error::other)?
        && current != program.build
        && !crate::program::newer(&program.build, &current).map_err(io::Error::other)?
    {
        return Err(io::Error::other("the stable World program advanced after staging; no older image was installed"));
    }
    publish_image(&program.path, &installed)?;
    // The installed program's signature is its own, or none.
    let signature = signature_of(&program.path);
    if signature.is_file() {
        fastctx_platform::publish(
            &signature_of(&installed),
            &fs::read(&signature)?,
            fastctx_platform::Permissions::Private,
        )
    } else {
        match fs::remove_file(signature_of(&installed)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

/// Keeps a hard link to a mapped Windows image before replacing its name.
/// Without it NTFS refuses replacing the last name of an executable in use.
pub fn publish_image(source: &Path, target: &Path) -> io::Result<()> {
    use crate::transaction::{FileChange, Snapshot, Transaction};
    let before = Snapshot::read(target).map_err(|e| at("reading executable snapshot", target, io::Error::other(e)))?;
    let copy = FileChange::copy_executable(before, source)
        .map_err(|e| at("preparing executable copy", source, io::Error::other(e)))?;
    let receipt =
        Transaction::new([copy]).commit().map_err(|e| at("publishing executable", target, io::Error::other(e)))?;
    // Mapped images are intentionally retained; the existing cleanup helper
    // retries recognized links at the next placement. Immutable images
    // retained as renewal provenance live separately in renewal/images.
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
/// Explicit requests bypass a failed attempt's delay; recovery respects it.
pub fn request(root: &Path, retry: bool) -> anyhow::Result<()> {
    let _starting = FileLock::acquire(&directory(root).join("request.lock"))?;
    if let Some(plan) = read(root)? {
        if plan.phase == Phase::Failed && !retry && !plan.retry.ready(jiff::Timestamp::now()) {
            return Ok(());
        }
        let Some(worker) = FileLock::try_acquire(&directory(root).join("worker.lock"))? else { return Ok(()) };
        verify(&plan.candidate)?;
        drop(worker);
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
    // Maintenance preserves the selected host homes and executable search
    // path. Credentials and other launcher variables still come only from
    // the user's logon environment; jobs keep their own fresh environment.
    // On Linux the display the user's desktop gives goes too, so a repair
    // begun from the application checks its window there.
    let display: &[&str] =
        if cfg!(target_os = "linux") { &["DISPLAY", "XAUTHORITY", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR"] } else { &[] };
    for name in ["CODEX_HOME", "CLAUDE_CONFIG_DIR", "PATH"].iter().chain(display).copied() {
        if let Some(value) = std::env::var_os(name) {
            environment.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
            environment.push((name.into(), value));
        }
    }
    fastctx_platform::spawn_detached(
        program,
        &args.iter().map(|arg| arg.as_os_str()).collect::<Vec<_>>(),
        root,
        &environment,
    )
}

/// Keeps a delivery executable without changing the installed name or plan.
/// The caller holds collection_lock through launching the retained image.
pub fn keep_image(root: &Path, source: &Path) -> io::Result<PathBuf> {
    copy_image(root, source)
}

fn copy_image(root: &Path, source: &Path) -> io::Result<PathBuf> {
    let hash = digest(source)?;
    let folder = directory(root).join("images").join(&hash);
    ensure_private_dir(&folder).map_err(|e| at("preparing immutable image directory", &folder, e))?;
    let path = folder.join(EXECUTABLE);
    if !path.try_exists()? {
        copy_to(source, &path)?;
    }
    if digest(&path)? != hash {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "immutable executable digest differs"));
    }
    // A release's program carries its official signature beside it: the
    // boot service takes a program it can check without an administrator
    // (`world-upgrade.md`, 换版不要人确认).
    let signature = signature_of(source);
    if signature.is_file() && !signature_of(&path).is_file() {
        fs::copy(&signature, signature_of(&path))?;
    }
    Ok(path)
}

/// Makes the official signature beside the installed program, `program`,
/// the program's own: a node of an earlier release kept its package's,
/// which is over the archive, and the Windows guardian refuses the program
/// by it. The program's own comes from that package, kept in the World's
/// updates, when it carries one. Whether the signature beside the program
/// checks out now.
pub fn mend_signature(root: &Path, program: &Program) -> io::Result<bool> {
    let installed = installed(root);
    if digest(&installed)? != program.build.digest {
        return Ok(false);
    }
    let bytes = fs::read(&installed)?;
    let version = &program.build.version;
    let checks = |signature: &str| fastctx_update::verify::bytes(&bytes, signature, version).is_ok();
    if fs::read_to_string(signature_of(&installed)).is_ok_and(|signature| checks(&signature)) {
        return Ok(true);
    }
    let cache = fastctx_update::Cache::new(root.join("updates").join("packages"));
    let Some(archive) = cache.get(version, &fastctx_update::kernel::target()) else { return Ok(false) };
    let Some(signature) = fastctx_update::kernel::signature(&archive).map_err(io::Error::other)? else {
        return Ok(false);
    };
    if !checks(&signature) {
        return Ok(false);
    }
    for beside in [signature_of(&installed), signature_of(&program.path)] {
        publish(&beside, signature.as_bytes(), Permissions::Private)?;
    }
    Ok(true)
}

/// Publishes the checked signature beside an executable.
pub fn write_signature(path: &Path, signature: &str) -> io::Result<()> {
    publish(path, signature.as_bytes(), Permissions::Private)
}

/// Where the official signature of the program at `program` is kept.
pub fn signature_of(program: &Path) -> PathBuf {
    let mut name = program.as_os_str().to_owned();
    name.push(".sig");
    PathBuf::from(name)
}

fn copy_to(source: &Path, target: &Path) -> io::Result<()> {
    if target.try_exists()? && digest(source)? == digest(target)? {
        return Ok(());
    }
    let mut input = fs::File::open(source).map_err(|e| at("opening executable source", source, e))?;
    let mut staged = fastctx_platform::atomic::Staged::create(target, Some(0o700))
        .map_err(|e| at("creating staged executable", target, e))?;
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = input.read(&mut buffer).map_err(|e| at("reading executable source", source, e))?;
        if count == 0 {
            break;
        }
        staged.write_all(&buffer[..count]).map_err(|e| at("writing staged executable", target, e))?;
    }
    staged.commit().map_err(|e| at("publishing staged executable", target, e))
}

fn at(step: &str, path: &Path, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{step} {}: {error}", path.display()))
}
