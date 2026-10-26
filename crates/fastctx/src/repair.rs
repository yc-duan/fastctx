//! Standalone repair. The process carrying this code survives node handovers.

use anyhow::{Context as _, bail, ensure};
use clap::Args;
use fastctx_client::renewal::Probe;

mod service;
use fastctx_install::{
    renewal as store,
    repair::{Journal, package, records},
};
use fastctx_types::{
    layout::WorldDir,
    renewal::{Build, Phase},
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Args, Clone)]
pub struct RepairArgs {
    #[arg(long, value_parser = crate::paths::parser())]
    dir: PathBuf,
    #[arg(long)]
    version: Option<String>,
    #[arg(long, default_value = "local")]
    by: String,
    #[arg(long)]
    id: Option<String>,
    /// An already downloaded command-line package; verified again before use.
    #[arg(long, value_parser = crate::paths::parser())]
    package: Option<PathBuf>,
    /// Starts an independent repair and returns its request identifier.
    #[arg(long)]
    background: bool,
    #[arg(long, hide = true)]
    accepted: bool,
    /// Repairs a device-only interface without placing a local node.
    #[arg(long)]
    application_only: bool,
    /// Opens this registered application after a successful repair.
    #[arg(long, value_parser = crate::paths::parser())]
    reopen: Option<PathBuf>,
}

pub fn run(args: RepairArgs) -> anyhow::Result<()> {
    let root = args.dir.clone();
    let id = args.id.clone();
    let result = if args.background { background(args) } else { run_inner(args) };
    if let Err(error) = &result
        && let Err(recording) = fastctx_install::repair::record_failure(&root, id.as_deref(), &format!("{error:#}"))
    {
        return Err(anyhow::anyhow!("{error:#}; recording repair failure: {recording:#}"));
    }
    result
}

fn background(args: RepairArgs) -> anyhow::Result<()> {
    let root = std::path::absolute(&args.dir)?;
    fastctx_platform::ensure_private_dir(&root.join("updates"))?;
    let _gate = fastctx_platform::FileLock::try_acquire(&root.join("updates/repair.lock"))?
        .context("a repair is already running")?;
    let mut journal =
        Journal::start(&root, args.version.as_deref().unwrap_or(env!("CARGO_PKG_VERSION")), &args.by, args.id)?;
    journal.begin("package")?;
    let _collection = fastctx_platform::FileLock::acquire(&store::collection_lock(&root))?;
    let source = std::env::current_exe()?;
    let digest = store::digest(&source)?;
    let retained = store::directory(&root).join("images").join(&digest).join(store::EXECUTABLE);
    if retained.is_file() && store::digest(&retained)? != digest {
        journal.action(format!(
            "restore damaged repair controller {} from {}",
            retained.display(),
            source.display()
        ))?;
        store::publish_image(&source, &retained)?;
    }
    let program = store::keep_image(&root, &source)?;
    let mut words = vec![
        "renew".into(),
        "repair".into(),
        "--dir".into(),
        root.as_os_str().into(),
        "--id".into(),
        journal.report.id.clone().into(),
        "--by".into(),
        args.by.into(),
        "--accepted".into(),
    ];
    if args.application_only {
        words.push("--application-only".into());
    }
    if let Some(reopen) = args.reopen {
        words.extend(["--reopen".into(), reopen.into_os_string()]);
    }
    if let Some(version) = args.version {
        words.extend(["--version".into(), version.into()]);
    }
    if let Some(package) = args.package {
        words.extend(["--package".into(), package.into_os_string()]);
    }
    let process = store::launch(&root, &program, &words)?.identity.context("repair exited during launch")?;
    if let Err(error) = journal.handoff(process.clone()) {
        fastctx_platform::terminate::terminate(&process)?;
        return Err(error);
    }
    drop(_gate);
    println!("{}", serde_json::json!({"id":journal.report.id,"accepted":true,"process":process}));
    Ok(())
}

fn run_inner(args: RepairArgs) -> anyhow::Result<()> {
    let root = std::path::absolute(&args.dir)?;
    fastctx_platform::ensure_private_dir(&root.join("updates"))?;
    let began = Instant::now();
    let gate = loop {
        if let Some(gate) = fastctx_platform::FileLock::try_acquire(&root.join("updates/repair.lock"))? {
            break gate;
        }
        ensure!(args.accepted && began.elapsed() < Duration::from_secs(10), "a repair is already running");
        ensure!(
            fastctx_install::repair::read(&root)?.is_some_and(|r| Some(r.id.as_str()) == args.id.as_deref()),
            "repair acceptance changed before launch"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let uninstall =
        fastctx_platform::uninstall_gate::Gate::try_acquire(&root)?.context("uninstall is still running")?;
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let source = std::env::current_exe()?;
    let build = store::build()?;
    let mut journal = Journal::start(&root, &build.version, &args.by, args.id.clone())?;
    journal.begin("package")?;
    let target = (|| -> anyhow::Result<String> {
        if args.application_only {
            return requested_application(&root, args.version.as_deref(), &build.version);
        }
        let serving = match runtime.block_on(crate::renew::observing(&WorldDir::under(&root)))? {
            Probe::Running(node) => Some(node.build.version),
            _ => None,
        };
        fastctx_install::repair::target::select(&root, &build.version, args.version.as_deref(), serving.as_deref())
    })();
    let version = match target {
        Ok(version) => version,
        Err(error) => {
            journal.end(Err(error))?;
            journal.finish()?;
            bail!("{}", journal.report.error.unwrap_or_default());
        }
    };
    journal.set_version(version.clone())?;
    let distributed = fastctx_platform::RELEASED || std::env::var_os("FASTCTX_UPDATE_RELEASES").is_some();
    let manifest = if distributed || args.package.is_some() || version != build.version {
        match runtime.block_on(package::manifest(&root, &version)) {
            Ok(manifest) => Some(manifest),
            Err(error) => {
                journal.end(Err(error))?;
                journal.finish()?;
                bail!("{}", journal.report.error.unwrap_or_default());
            }
        }
    } else {
        None
    };
    let mut brought = None;
    let checked = (|| -> anyhow::Result<Option<PathBuf>> {
        if let Some(manifest) = &manifest {
            let target = fastctx_update::kernel::target();
            let archive = if let Some(path) = &args.package {
                let signature = &manifest.package(&target).context("manifest has no command-line package")?.signature;
                fastctx_update::verify::file(path, signature, &version).map_err(anyhow::Error::msg)?;
                path.clone()
            } else {
                let obtained = runtime.block_on(package::obtain(&root, manifest, &target, &mut journal))?;
                brought = obtained.brought.then(|| obtained.path.clone());
                obtained.path
            };
            if version == build.version {
                // A Windows package carries the program's own signature, which
                // its boot guardian takes it by; elsewhere the package's
                // signature, checked above, vouches for the program inside.
                match fastctx_update::kernel::signature(&archive).map_err(anyhow::Error::msg)? {
                    Some(signature) => fastctx_update::verify::file(&source, &signature, &version)
                        .map_err(anyhow::Error::msg)
                        .with_context(|| format!("checking repair program {} against its release", source.display()))?,
                    None => ensure!(
                        fastctx_update::kernel::carries(&archive, &source).map_err(anyhow::Error::msg)?,
                        "repair program {} differs from the program its release carries",
                        source.display()
                    ),
                }
            }
            Ok(Some(archive))
        } else {
            Ok(None)
        }
    })();
    let archive = match checked {
        Ok(archive) => {
            journal.end(Ok(match (&archive, &brought) {
                (Some(_), Some(path)) => format!("brought {}; package signature and version checked", path.display()),
                (Some(_), None) => "package signature and version checked".into(),
                (None, _) => "development executable; no release acquisition".into(),
            }))?;
            archive
        }
        Err(error) => {
            journal.end(Err(error))?;
            journal.finish()?;
            bail!("{}", journal.report.error.unwrap_or_default());
        }
    };
    if version != build.version {
        let unpacked = root.join("updates").join(format!("repair-bootstrap-{}", std::process::id()));
        std::fs::create_dir_all(&unpacked)?;
        let result = (|| -> anyhow::Result<()> {
            let executable = fastctx_update::kernel::unpack(
                archive.as_deref().context("repair requires its version's package")?,
                &unpacked,
            )
            .map_err(anyhow::Error::msg)?;
            let candidate = fastctx_install::program::inspect(&executable)?;
            ensure!(candidate.version == version, "repair package contains {}, expected {version}", candidate.version);
            drop(uninstall);
            drop(gate);
            let mut command = std::process::Command::new(&executable);
            command.args(["renew", "repair", "--dir"]).arg(&root).args([
                "--version",
                &version,
                "--by",
                &args.by,
                "--id",
                &journal.report.id,
            ]);
            if args.application_only {
                command.arg("--application-only");
            }
            if let Some(reopen) = &args.reopen {
                command.arg("--reopen").arg(reopen);
            }
            command.env("FASTCTX_HOME", &root).stdin(std::process::Stdio::null());
            for name in ["FASTCTX_RESIDENT", "FASTCTX_CONVERSATION", "FASTCTX_SESSION"] {
                command.env_remove(name);
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt as _;
                command.creation_flags(0x08000000);
            }
            let status =
                command.status().with_context(|| format!("running repair program {}", executable.display()))?;
            ensure!(status.success(), "repair program {} exited {status}", executable.display());
            Ok(())
        })();
        std::fs::remove_dir_all(&unpacked).with_context(|| format!("removing {}", unpacked.display()))?;
        return result;
    }
    if !args.application_only {
        journal.begin("records")?;
        let cleaned = records::clean(&root, &source, &build, &mut journal);
        if !journal.end(cleaned)? {
            journal.finish()?;
            bail!("{}", journal.report.error.unwrap_or_default());
        }
        journal.begin("program")?;
        let program = program(&root, &source, &build, archive.as_deref(), &runtime, &mut journal);
        if !journal.end(program)? {
            journal.finish()?;
            bail!("{}", journal.report.error.unwrap_or_default());
        }
        journal.begin("service")?;
        let service = service::run(&root, &build, &runtime, &mut journal);
        journal.end(service)?;
    }
    journal.begin("application")?;
    let app = runtime.block_on(fastctx_install::repair::application::repair(&root, &build, &mut journal));
    journal.end(app)?;
    if !args.application_only {
        journal.begin("node")?;
        let node = node(&root, &build, &runtime, &mut journal);
        journal.end(node)?;
    }
    if journal.report.steps.iter().all(|step| {
        matches!(step.outcome, fastctx_types::repair::Outcome::Unchanged | fastctx_types::repair::Outcome::Repaired)
    }) && let Some(executable) = &args.reopen
    {
        journal.begin("reopen")?;
        journal.end(reopen(&root, executable).map(|_| "application launch accepted".into()))?;
    }
    journal.finish()?;
    println!("{}", serde_json::to_string(&journal.report)?);
    ensure!(journal.report.success, "{}", journal.report.error.as_deref().unwrap_or("repair failed"));
    Ok(())
}

fn requested_application(root: &Path, explicit: Option<&str>, running: &str) -> anyhow::Result<String> {
    let app = store::read_json::<fastctx_install::repair::application::Registration>(&root.join("updates/app.json"))?;
    let versions = [Some(running), explicit, app.as_ref().map(|a| a.app.version.as_str())];
    for version in versions.into_iter().flatten() {
        ensure!(fastctx_types::update::release(version).is_some(), "invalid application repair version {version}");
    }
    Ok(versions
        .into_iter()
        .flatten()
        .max_by_key(|v| fastctx_types::update::release(v))
        .context("no application repair version")?
        .into())
}

fn reopen(root: &Path, executable: &Path) -> anyhow::Result<()> {
    let app = store::read_json::<fastctx_install::repair::application::Registration>(&root.join("updates/app.json"))?
        .context("no registered application to reopen")?;
    ensure!(app.executable()? == executable, "application to reopen differs from its registration");
    #[cfg(windows)]
    fastctx_install::interactive::reopen(root, executable)?;
    #[cfg(not(windows))]
    {
        let environment: Vec<_> = std::env::vars_os()
            .filter(|(name, _)| {
                !["FASTCTX_RESIDENT", "FASTCTX_CONVERSATION", "FASTCTX_SESSION"].iter().any(|key| name == key)
            })
            .collect();
        fastctx_platform::spawn_detached(
            executable,
            &[],
            executable.parent().context("application has no parent")?,
            &environment,
        )
        .with_context(|| format!("reopening application {}", executable.display()))?;
    }

    Ok(())
}

fn program(
    root: &Path,
    source: &Path,
    build: &Build,
    archive: Option<&Path>,
    runtime: &tokio::runtime::Runtime,
    journal: &mut Journal,
) -> anyhow::Result<String> {
    let installed = store::installed(root);
    let current =
        installed.is_file() && fastctx_install::program::inspect(&installed).is_ok_and(|actual| actual == *build);
    let plan = store::read(root)?;
    if !current || plan.as_ref().is_some_and(|p| p.phase != Phase::Complete) {
        journal.action(format!("stage {} into {} and resume handover", source.display(), installed.display()))?;
        let staged = records::restage(root, source, journal)?;
        if staged.phase != Phase::Complete {
            store::request(root, true)?;
        }
        let mut began = Instant::now();
        loop {
            let plan = store::read(root)?.context("repair lost its renewal plan")?;
            ensure!(plan.candidate.build == *build, "renewal target changed during repair");
            if installed.is_file() && store::digest(&installed)? == build.digest {
                break;
            }
            if plan.phase == Phase::Failed {
                bail!("publishing {}: {}", installed.display(), plan.error.unwrap_or_default());
            }
            if plan.phase == Phase::WaitingForIdle {
                began = Instant::now();
            }
            ensure!(
                began.elapsed() < Duration::from_secs(600),
                "publishing {} stalled at {:?}",
                installed.display(),
                plan.phase
            );
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    // A Windows program keeps its own signature beside it, which the boot
    // guardian takes it by; a release elsewhere signs only its packages.
    if let Some(archive) = archive {
        let signed = store::signature_of(&installed);
        let valid = std::fs::read_to_string(&signed)
            .is_ok_and(|sig| fastctx_update::verify::file(&installed, &sig, &build.version).is_ok());
        match fastctx_update::kernel::signature(archive).map_err(anyhow::Error::msg)? {
            Some(signature) if !valid => {
                fastctx_update::verify::file(&installed, &signature, &build.version).map_err(anyhow::Error::msg)?;
                journal.action(format!("restore program signature {}", signed.display()))?;
                store::write_signature(&signed, &signature)?;
            }
            Some(_) => {}
            None => ensure!(!cfg!(windows), "{} carries no program signature", archive.display()),
        }
    }
    let _ = runtime;
    Ok(format!("{} matches {}", installed.display(), build.version))
}

fn connected(root: &Path) -> anyhow::Result<bool> {
    let net = WorldDir::under(root).net();
    if net.join("membership.json").try_exists()? {
        return Ok(true);
    }
    if !net.try_exists()? {
        return Ok(false);
    }
    for entry in std::fs::read_dir(&net)? {
        let path = entry?.path();
        if path.join("membership.json").try_exists()? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn node(
    root: &Path,
    build: &Build,
    runtime: &tokio::runtime::Runtime,
    journal: &mut Journal,
) -> anyhow::Result<String> {
    let world = WorldDir::under(root);
    if !connected(root)?
        && !world.node_service().try_exists()?
        && matches!(runtime.block_on(crate::renew::observing(&world))?, Probe::Absent)
    {
        return Ok("no connected node is required".into());
    }
    if let Some(plan) = store::read(root)?
        && plan.phase != Phase::Complete
    {
        journal.action(format!("resume {}", store::directory(root).join("plan.json").display()))?;
        store::request(root, true)?;
    }
    let began = Instant::now();
    loop {
        if let Probe::Running(node) = runtime.block_on(crate::renew::observing(&world))?
            && node.build == *build
            && !node.bridge
            && !node.yielding
        {
            if world.node_service().try_exists()? {
                let service = fastctx_install::service::Service::new(
                    fastctx_install::service::Kind::Node,
                    store::installed(root),
                    root,
                );
                ensure!(
                    service.guardian_current(&build.digest)?,
                    "boot guardian for {} still differs from {}",
                    root.display(),
                    build.version
                );
                #[cfg(target_os = "linux")]
                ensure!(
                    service.main_pid()? == u64::from(node.process.pid),
                    "node {} is not the boot service's process",
                    node.process.pid
                );
                #[cfg(windows)]
                ensure!(node.supervised, "node {} is outside its boot guardian", node.process.pid);
            }
            if connected(root)? {
                let links = runtime.block_on(async {
                    tokio::time::timeout(Duration::from_secs(30), async {
                        let mut client = fastctx_client::Client::connect(
                            &world,
                            fastctx_types::ipc::Peer::Command { resident: None, session: None },
                        )
                        .await?;
                        let answer = client.call(fastctx_types::Call::Nodes).await?;
                        Ok::<_, anyhow::Error>(answer)
                    })
                    .await
                    .with_context(|| {
                        format!(
                            "checking node {} network status under {} exceeded 30 seconds",
                            node.process.pid,
                            root.display()
                        )
                    })?
                })?;
                if let fastctx_types::Reply::Nodes(nodes) = links {
                    if !nodes.hubs.is_empty()
                        && nodes
                            .hubs
                            .iter()
                            .all(|hub| matches!(hub.link, fastctx_types::world::LinkState::Online { .. }))
                    {
                        return Ok(format!(
                            "node {} serves {} and its hubs are connected",
                            node.process.pid, build.version
                        ));
                    }
                } else {
                    bail!("node returned no network status during repair");
                }
            } else {
                return Ok(format!("node {} serves {}", node.process.pid, build.version));
            }
        }
        ensure!(
            began.elapsed() < Duration::from_secs(600),
            "node for {} did not serve {} within ten minutes",
            root.display(),
            build.version
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}
