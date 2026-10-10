//! Persistent node and hub registrations. Installation starts the service;
//! removal stops it. Managers restore exited processes without a retry limit;
//! an independent periodic check also restores a stopped service.

#[cfg(target_os = "linux")]
use std::ffi::OsString;
use std::path::PathBuf;
#[cfg(any(windows, target_os = "linux"))]
use std::process::{Command, Stdio};

#[cfg(any(windows, target_os = "linux"))]
use anyhow::Context as _;
use anyhow::bail;
use serde::{Deserialize, Serialize};
#[cfg(any(windows, target_os = "linux"))]
use sha2::{Digest as _, Sha256};

mod hub;
mod keep;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod linux_watchdog;
#[cfg(any(windows, target_os = "linux"))]
mod transaction;

pub use keep::{Kept, Released, keep_connected, stop_keeping};
pub use hub::keep_hub;
#[cfg(windows)]
#[path = "windows_guardian/mod.rs"]
mod windows;

/// A node owns a fastctx root; a hub owns its hub directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Node,
    Hub,
}

/// The complete desired registration. The executable must be the stable
/// installed copy. Directories and executable paths are absolute and are
/// passed as native strings without Unicode replacement or shell expansion.
/// On Windows, the calling executable must dispatch `service-host` and
/// `service-check` to this module's corresponding entry points. A protected
/// copy of that caller is the SYSTEM guardian; the node executable runs only
/// under the target user's token.
#[derive(Clone, Debug)]
pub struct Service {
    pub kind: Kind,
    pub executable: PathBuf,
    pub directory: PathBuf,
    /// Linux system services run as this UID (root installs on its behalf).
    #[cfg(target_os = "linux")]
    pub uid: u32,
    /// Windows node owner; defaults to the installing user's SID.
    /// Hubs always run as LocalSystem.
    #[cfg(windows)]
    pub user_sid: Option<String>,
}

/// Facts reported by the service manager, including failed/disabled states.
/// `last_exit` is the manager's last result, not an application health probe.
/// On Windows these are guardian facts; node exits are in its guardian log
/// and node stdout/stderr are in `service-node.log` under the node directory.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Status {
    pub name: String,
    pub installed: bool,
    pub enabled: bool,
    pub state: String,
    pub last_exit: Option<i64>,
    pub logs: String,
    #[serde(default)]
    pub recovery: Option<RecoveryStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoveryStatus {
    pub name: String,
    pub enabled: bool,
    pub state: String,
}

impl Service {
    pub fn new(kind: Kind, executable: impl Into<PathBuf>, directory: impl Into<PathBuf>) -> Self {
        Self {
            kind,
            executable: executable.into(),
            directory: directory.into(),
            #[cfg(windows)]
            user_sid: None,
            #[cfg(target_os = "linux")]
            // SAFETY: getuid has no preconditions.
            uid: if kind == Kind::Hub { 0 } else { unsafe { libc::getuid() } },
        }
    }

    /// Installs and starts this registration. Repeating the same installation
    /// leaves a running service alone. An update rolls back its registration
    /// on failure; rollback failures are included in the returned error.
    ///
    /// A node a service keeps is started by nobody else: while the service
    /// is in place, its World says so to the World's clients.
    pub fn install(&self) -> anyhow::Result<Status> {
        self.validate()?;
        #[cfg(windows)]
        let status = windows::install(self)?;
        #[cfg(target_os = "linux")]
        let status = linux::install(self)?;
        #[cfg(not(any(windows, target_os = "linux")))]
        bail!("fastctx services support Windows and Linux; this platform is not supported");
        #[cfg(any(windows, target_os = "linux"))]
        {
            if self.kind == Kind::Node {
                std::fs::write(keep::marker(&self.directory), b"")?;
            }
            Ok(status)
        }
    }

    /// Stops and deletes only this registration. A missing registration is
    /// already uninstalled. Data directories and logs are retained.
    pub fn uninstall(&self) -> anyhow::Result<Status> {
        self.validate()?;
        #[cfg(windows)]
        let status = windows::uninstall(self)?;
        #[cfg(target_os = "linux")]
        let status = linux::uninstall(self)?;
        #[cfg(not(any(windows, target_os = "linux")))]
        bail!("fastctx services support Windows and Linux; this platform is not supported");
        #[cfg(any(windows, target_os = "linux"))]
        {
            if self.kind == Kind::Node {
                match std::fs::remove_file(keep::marker(&self.directory)) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
                    _ => {}
                }
            }
            Ok(status)
        }
    }

    pub fn status(&self) -> anyhow::Result<Status> {
        self.validate()?;
        #[cfg(windows)]
        return windows::status(self);
        #[cfg(target_os = "linux")]
        return linux::status(self);
        #[cfg(not(any(windows, target_os = "linux")))]
        bail!("fastctx services support Windows and Linux; this platform is not supported")
    }

    /// Whether the protected guardian already runs this image. Linux's
    /// guardian is systemd itself, so a node image needs no privileged update.
    pub fn guardian_current(&self, digest: &str) -> anyhow::Result<bool> {
        self.validate()?;
        #[cfg(windows)]
        return windows::guardian_current(self, digest);
        #[cfg(not(windows))]
        {
            let _ = digest;
            Ok(true)
        }
    }

    /// Stages this approved process's image in an existing protected
    /// registration. It does not stop anything or open the user's World.
    pub fn renew_guardian(&self, digest: &str) -> anyhow::Result<()> {
        self.validate()?;
        #[cfg(windows)]
        return windows::renew_guardian(self, digest);
        #[cfg(not(windows))]
        {
            let _ = digest;
            bail!("this platform has no bundled guardian")
        }
    }

    pub fn guardian_staged(&self, digest: &str) -> anyhow::Result<bool> {
        self.validate()?;
        #[cfg(windows)]
        return windows::guardian_staged(self, digest);
        #[cfg(not(windows))]
        {
            let _ = digest;
            Ok(true)
        }
    }

    /// The target user has SCM stop/start rights. Restarting after staging
    /// needs no second elevation, including after the approving helper died.
    /// The caller holds the startup gate and verifies a serving bridge first.
    pub fn restart_guardian(&self, digest: &str) -> anyhow::Result<()> {
        self.validate()?;
        #[cfg(windows)]
        return windows::restart_guardian(self, digest);
        #[cfg(not(windows))]
        {
            let _ = digest;
            Ok(())
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        #[cfg(target_os = "linux")]
        if self.kind == Kind::Hub && self.uid != 0 {
            bail!("a hub system service runs as root");
        }
        #[cfg(windows)]
        if self.kind == Kind::Hub && self.user_sid.is_some() {
            bail!("a hub system service runs as LocalSystem");
        }
        for path in [&self.executable, &self.directory] {
            if !path.is_absolute() {
                bail!("a service path must be absolute: {}", path.display());
            }
        }
        Ok(())
    }

    #[cfg(any(windows, target_os = "linux"))]
    fn role(&self) -> &'static str {
        match self.kind {
            Kind::Node => "node",
            Kind::Hub => "hub",
        }
    }

    #[cfg(target_os = "linux")]
    fn arguments(&self) -> Vec<OsString> {
        let mut args = vec![self.role().into()];
        if self.kind == Kind::Hub {
            args.push("serve".into());
        }
        args.extend(["--dir".into(), self.directory.as_os_str().into()]);
        args
    }

    /// Hashes the native directory spelling and account, independent of the
    /// binary version. Keeping the same spelling identifies the same service.
    #[cfg(any(windows, target_os = "linux"))]
    fn id(&self, account: &[u8]) -> String {
        let mut hash = Sha256::new();
        hash.update(self.role());
        hash.update([0]);
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt as _;
            hash.update(self.directory.as_os_str().as_bytes());
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt as _;
            for unit in self.directory.as_os_str().encode_wide() {
                hash.update(unit.to_le_bytes());
            }
        }
        hash.update([0]);
        hash.update(account);
        format!("fastctx-service-{}-{:x}", self.role(), hash.finalize())
    }
}

#[cfg(windows)]
pub fn run_host(name: &str) -> anyhow::Result<()> {
    windows::run_host(name)
}

#[cfg(windows)]
pub fn check_guardian(name: &str) -> anyhow::Result<()> {
    windows::check(name)
}

#[cfg(any(windows, target_os = "linux"))]
fn checked(command: &mut Command) -> anyhow::Result<String> {
    command.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let output = command.output().with_context(|| format!("starting {:?}", command.get_program()))?;
    if !output.status.success() {
        bail!(
            "{:?} exited {}: {}{}",
            command.get_program(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout).context("service manager returned non-UTF-8 output")
}
