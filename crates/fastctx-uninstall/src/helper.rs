//! The executable doing the removal lives outside the installation. On
//! Windows a second link keeps the invoking image alive until its owner
//! exits; a native OS helper then retires the private temporary directory.

use crate::Request;
#[cfg(windows)]
use anyhow::Context as _;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn run_outside(request: &Request) -> anyhow::Result<()> {
    // Nothing is copied or started for a removal a managed machine refuses
    // (`crate::execute::run`).
    if let Some(managed) = &request.plan.managed
        && !fastctx_platform::elevation::is_administrator()?
    {
        anyhow::bail!("{}", managed.refusal());
    }
    let temporary = tempfile::Builder::new().prefix("fastctx-uninstall-").tempdir()?;
    fastctx_platform::ensure_private_dir(temporary.path())?;
    let original = std::env::current_exe()?;
    let program = temporary.path().join(if cfg!(windows) { "fastctx.exe" } else { "fastctx" });
    std::fs::copy(&original, &program)?;
    let input = temporary.path().join("request.json");
    std::fs::write(&input, serde_json::to_vec(request)?)?;
    #[cfg(windows)]
    if original.starts_with(&request.installation.root) {
        std::fs::hard_link(&original, temporary.path().join("invoking-image.exe"))?;
    }
    let mut command = Command::new(&program);
    command
        .args(["uninstall", "worker", "--request"])
        .arg(&input)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn()?;
    #[cfg(windows)]
    {
        let worker = fastctx_platform::identity_of(child.id())?
            .context("the uninstall worker exited before its identity was captured")?;
        let caller = fastctx_platform::current_identity()?;
        schedule_cleanup(temporary.path(), &[caller, worker])?;
    }
    let status = child.wait()?;
    #[cfg(windows)]
    {
        let _ = temporary.keep();
    }
    #[cfg(not(windows))]
    temporary.close()?;
    anyhow::ensure!(status.success(), "the uninstall worker exited {status}");
    Ok(())
}

/// Starts only an OS-provided waiting process; it never runs from the files
/// it will remove. Every PID is compared with its recorded creation token.
#[cfg(windows)]
pub fn schedule_cleanup(directory: &Path, identities: &[fastctx_types::ProcessIdentity]) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt as _;
    let quote = |text: &str| format!("'{}'", text.replace('\'', "''"));
    let mut script = "$ErrorActionPreference='Stop';".to_owned();
    for identity in identities {
        script.push_str(&format!(" $p=Get-Process -Id {} -ErrorAction SilentlyContinue; if ($null -ne $p -and $p.StartTime.ToUniversalTime().ToFileTimeUtc().ToString() -eq {}) {{ $p.WaitForExit() }};", identity.pid, quote(&identity.token)));
    }
    let directory = directory.to_str().context("uninstall helper directory is not Unicode")?;
    script.push_str(&format!(" Remove-Item -LiteralPath {} -Recurse -Force", quote(directory)));
    Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .creation_flags(0x08000000)
        .spawn()?;
    Ok(())
}

/// Returns a checked original-user request; elevation uses a separate entry
/// point that is permitted to operate only on the listed system objects.
pub fn read_request(path: &Path) -> anyhow::Result<Request> {
    let request: Request = serde_json::from_slice(&std::fs::read(path)?)?;
    request.installation.validate()?;
    #[cfg(windows)]
    anyhow::ensure!(
        request.installation.user_sid == fastctx_platform::current_user_sid()?,
        "the uninstall worker belongs to a different user"
    );
    #[cfg(target_os = "linux")]
    anyhow::ensure!(
        request.installation.uid == unsafe { libc::getuid() },
        "the uninstall worker belongs to a different user"
    );
    Ok(request)
}
