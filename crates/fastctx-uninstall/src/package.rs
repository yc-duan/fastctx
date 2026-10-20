//! Debian maintainer hooks retain each user's data. A root-owned registry
//! supplies original user coordinates; host withdrawal still runs as that user.

use anyhow::Context as _;
use fastctx_install::uninstall::Installation;
use fastctx_types::uninstall::{Event, Result as Outcome};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};

const REGISTRY: &str = "/var/lib/fastctx/installations";
/// Tauri names the Debian package after the product, `Fastctx`, in kebab case.
const PACKAGE: &str = "fastctx";

#[derive(Serialize, Deserialize)]
struct Registration {
    installation: Installation,
    completed: bool,
}

fn record(installation: &Installation) -> PathBuf {
    Path::new(REGISTRY).join(format!("{}.json", installation.uid))
}

/// Called by the same privileged command that installs the graphical service.
pub fn remember(installation: Installation) -> anyhow::Result<()> {
    anyhow::ensure!(unsafe { libc::geteuid() } == 0, "recording a Debian installation requires root");
    if installation.application.as_deref() != Some(Path::new("/usr/bin/fastctx-app")) {
        return Ok(());
    }
    installation.validate()?;
    anyhow::ensure!(
        std::fs::metadata(&installation.root)?.uid() == installation.uid,
        "the package registration names another user's World"
    );
    std::fs::create_dir_all(REGISTRY)?;
    std::fs::set_permissions(REGISTRY, std::fs::Permissions::from_mode(0o700))?;
    fastctx_platform::publish(
        &record(&installation),
        &serde_json::to_vec(&Registration { installation, completed: false })?,
        fastctx_platform::Permissions::Private,
    )?;
    Ok(())
}

fn complete(installation: &Installation) -> anyhow::Result<()> {
    let path = record(installation);
    let mut registration: Registration =
        crate::read(&path)?.context("the Debian installation has no original-user registration")?;
    anyhow::ensure!(
        registration.installation.root == installation.root,
        "the Debian registration belongs to another World"
    );
    registration.completed = true;
    fastctx_platform::publish(&path, &serde_json::to_vec(&registration)?, fastctx_platform::Permissions::Private)?;
    Ok(())
}

pub fn before_remove() -> anyhow::Result<()> {
    anyhow::ensure!(unsafe { libc::geteuid() } == 0, "the Debian removal hook requires root");
    for entry in crate::entries(Path::new(REGISTRY))? {
        let registration: Registration = serde_json::from_slice(&std::fs::read(entry.path())?)?;
        if !registration.completed {
            withdraw(registration.installation)?;
        }
        fastctx_install::uninstall::delete_file(&entry.path())?;
    }
    if Path::new(REGISTRY).try_exists()? && crate::entries(Path::new(REGISTRY))?.is_empty() {
        std::fs::remove_dir(REGISTRY)?;
        if crate::entries(Path::new("/var/lib/fastctx"))?.is_empty() {
            std::fs::remove_dir("/var/lib/fastctx")?;
        }
    }
    Ok(())
}

fn withdraw(mut installation: Installation) -> anyhow::Result<()> {
    installation.graphical = false;
    installation.application_control = None;
    installation.application_process = None;
    let mut request = crate::preview(installation.clone(), false)?;
    let mut results = Vec::new();
    crate::run_system(&request, |event| results.push(event));
    for event in &results {
        if let Event::Step { result: Outcome::Failed, why, .. } = event {
            anyhow::bail!("the Debian system cleanup failed: {}", why.as_deref().unwrap_or("no reason was provided"));
        }
    }
    request.system_results = results;
    let temporary = tempfile::Builder::new().prefix("fastctx-package-user-").tempdir()?;
    let program = temporary.path().join("fastctx");
    std::fs::copy(std::env::current_exe()?, &program)?;
    let input = temporary.path().join("request.json");
    std::fs::write(&input, serde_json::to_vec(&request)?)?;
    let (name, gid) = account(installation.uid)?;
    let directory = std::ffi::CString::new(temporary.path().as_os_str().as_encoded_bytes())?;
    anyhow::ensure!(
        unsafe { libc::chown(directory.as_ptr(), installation.uid, gid) } == 0,
        "assigning the user handoff directory: {}",
        std::io::Error::last_os_error()
    );
    let mut command = std::process::Command::new(&program);
    command.args(["uninstall", "worker", "--request"]).arg(input).current_dir(&installation.home);
    command
        .env_clear()
        .env("HOME", &installation.home)
        .env("FASTCTX_HOME", &installation.root)
        .env("CODEX_HOME", &installation.codex_home)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin");
    if let Some(home) = &installation.claude_override {
        command.env("CLAUDE_CONFIG_DIR", home);
    }
    // SAFETY: only async-signal-safe credential calls run in the child. The
    // account and group list are resolved before fork, without login files.
    let groups = groups(&name, gid)?;
    unsafe {
        command.pre_exec(move || {
            if libc::setgroups(groups.len(), groups.as_ptr()) != 0
                || libc::setgid(gid) != 0
                || libc::setuid(installation.uid) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let status = command.status()?;
    temporary.close()?;
    anyhow::ensure!(status.success(), "the Debian user's removal exited {status}");
    Ok(())
}

fn account(uid: u32) -> anyhow::Result<(std::ffi::CString, u32)> {
    let mut record = unsafe { std::mem::zeroed::<libc::passwd>() };
    let mut found = std::ptr::null_mut();
    let mut buffer = Vec::new();
    // sysconf reports the platform's initial passwd-query buffer size.
    let hint = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let size = if hint > 0 { hint as usize } else { 16 * 1024 };
    buffer.resize(size, 0u8);
    loop {
        let code = unsafe { libc::getpwuid_r(uid, &mut record, buffer.as_mut_ptr().cast(), buffer.len(), &mut found) };
        if code == libc::ERANGE {
            buffer.resize(buffer.len().checked_mul(2).context("passwd buffer size overflow")?, 0);
            continue;
        }
        anyhow::ensure!(code == 0 && !found.is_null(), "the original UID {uid} has no readable account: {code}");
        return Ok((unsafe { std::ffi::CStr::from_ptr(record.pw_name) }.to_owned(), record.pw_gid));
    }
}

fn groups(name: &std::ffi::CStr, gid: u32) -> anyhow::Result<Vec<u32>> {
    let mut size = 0;
    unsafe { libc::getgrouplist(name.as_ptr(), gid, std::ptr::null_mut(), &mut size) };
    anyhow::ensure!(size > 0, "the account has no primary group");
    let mut groups = vec![gid; size as usize];
    anyhow::ensure!(
        unsafe { libc::getgrouplist(name.as_ptr(), gid, groups.as_mut_ptr(), &mut size) } >= 0,
        "the account's group list changed during resolution"
    );
    groups.truncate(size as usize);
    Ok(groups)
}

/// The approval obtained for kernel cleanup remains in this helper until
/// Fastctx requests package removal and its exact process lifetime exits.
pub(crate) async fn finish(request: &crate::Request) -> anyhow::Result<()> {
    let installation = &request.installation;
    let control = installation.application_control.as_ref().context("the graphical package handoff is absent")?;
    let identity = installation.application_process.as_ref().context("the graphical process identity is absent")?;
    let (send, mut changes) = tokio::sync::mpsc::unbounded_channel();
    use notify::Watcher as _;
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = send.send(event);
    })?;
    watcher
        .watch(control.parent().context("the package handoff has no parent")?, notify::RecursiveMode::NonRecursive)?;
    loop {
        if control.try_exists()? {
            anyhow::ensure!(std::fs::read(control)? == b"finish\n", "the package handoff is invalid");
            break;
        }
        tokio::select! {
            change = changes.recv() => { change.context("watching package removal stopped")??; },
            ended = fastctx_platform::host_process::exited(identity) => { ended?; anyhow::bail!("Fastctx exited without requesting package removal"); },
        }
    }
    fastctx_platform::host_process::exited(identity).await?;
    complete(installation)?;
    let status = std::process::Command::new("dpkg").args(["--remove", PACKAGE]).status()?;
    anyhow::ensure!(status.success(), "the Debian package uninstaller exited {status}");
    if request.plan.everything {
        crate::finish_application(&installation.root)?;
    }
    fastctx_install::uninstall::delete_tree(control.parent().context("the package handoff has no parent")?)?;
    Ok(())
}

pub async fn remove_image(
    image: &Path,
    process: &fastctx_types::ProcessIdentity,
    temporary: &Path,
    data: Vec<PathBuf>,
) -> anyhow::Result<()> {
    let mut installation = Installation::here()?;
    installation.application_dirs = data;
    installation.validate()?;
    let metadata = std::fs::symlink_metadata(image)?;
    anyhow::ensure!(
        image.is_absolute()
            && metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == installation.uid,
        "the AppImage is not an owned regular package file"
    );
    anyhow::ensure!(
        std::env::current_exe()?.parent() == Some(temporary),
        "the AppImage helper does not own this temporary directory"
    );
    fastctx_platform::host_process::exited(process).await?;
    let current = std::fs::symlink_metadata(image)?;
    anyhow::ensure!(
        current.dev() == metadata.dev() && current.ino() == metadata.ino(),
        "the AppImage changed before retirement"
    );
    fastctx_install::uninstall::delete_file(image)?;
    let left = crate::execute::application_data(&installation.application_dirs)?;
    if !left.is_empty() {
        println!("{}", serde_json::to_string(&Event::Finished { left })?);
    }
    schedule_cleanup(temporary)?;
    Ok(())
}

pub(crate) fn schedule_cleanup(directory: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::current_exe()?.parent() == Some(directory)
            && directory
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("fastctx-appimage-uninstall-")
                    || name.to_string_lossy().starts_with("fastctx-uninstall-admin-")),
        "the cleanup directory is not this helper's private directory"
    );
    let identity = fastctx_platform::current_identity()?;
    let token = identity.token.rsplit_once(':').context("the Linux process identity has no start time")?.1;
    let script = directory.join("cleanup.sh");
    std::fs::write(&script, b"#!/bin/bash\nset -eu\nwhile [[ -r /proc/$1/stat ]]; do\n  IFS= read -r stat < /proc/$1/stat || break\n  rest=${stat##*) }; read -ra fields <<< \"$rest\"\n  [[ ${fields[19]} == \"$2\" ]] || break\n  sleep 0.02\ndone\nrm -rf -- \"$3\"\n")?;
    std::process::Command::new("/bin/bash")
        .arg(script)
        .arg(identity.pid.to_string())
        .arg(token)
        .arg(directory)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .spawn()?;
    Ok(())
}
