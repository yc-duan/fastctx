//! Persistent node and hub registrations. Installation starts the service;
//! removal stops it. Managers restore exited processes without a retry limit;
//! an independent periodic check also restores a stopped service.

#[cfg(target_os = "linux")]
use std::ffi::OsString;
use std::path::PathBuf;
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
use std::process::{Command, Stdio};

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
use anyhow::Context as _;
use anyhow::bail;
use serde::{Deserialize, Serialize};
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
use sha2::{Digest as _, Sha256};

#[cfg(windows)]
mod delegate;
mod hub;
mod hub_registry;
mod inventory;
mod keep;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod linux_directory;
#[cfg(target_os = "linux")]
mod linux_restart;
#[cfg(target_os = "linux")]
mod linux_watchdog;
#[cfg(windows)]
mod local_network;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(windows, target_os = "linux"))]
mod transaction;

pub use hub::{keep_hub, keep_hub_at};
pub use hub_registry::registered_hubs;
pub use inventory::{Registration, registrations};
pub use keep::{Kept, Released, keep_connected, stop_keeping};
#[cfg(windows)]
pub use local_network::{install_local_network, keep_local_network};
#[cfg(windows)]
#[path = "windows_guardian/mod.rs"]
mod windows;
#[cfg(windows)]
pub use windows::managed;

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
    /// Linux system services run as this UID (root installs on its behalf);
    /// a macOS node is a LaunchAgent of this user.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
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
            #[cfg(any(target_os = "linux", target_os = "macos"))]
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
        let root = if self.kind == Kind::Node {
            Some(self.directory.as_path())
        } else {
            self.executable
                .parent()
                .filter(|parent| parent.file_name().is_some_and(|name| name == "bin"))
                .and_then(|parent| parent.parent())
        };
        if let Some(root) = root {
            anyhow::ensure!(!root.join("uninstalled.json").try_exists()?, "this World is being uninstalled");
        }
        let normalized = self.normalized()?;
        #[cfg(windows)]
        if let Some(status) = delegate::install(&normalized)? {
            return Ok(status);
        }
        #[cfg(windows)]
        let status = windows::install(&normalized)?;
        #[cfg(target_os = "linux")]
        let status = linux::install(&normalized)?;
        #[cfg(target_os = "macos")]
        let status = macos::install(&normalized)?;
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        bail!("fastctx services support Windows, Linux and macOS; this platform is not supported");
        #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
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
        let status = self.uninstall_registration()?;
        if self.kind == Kind::Node {
            match std::fs::remove_file(keep::marker(&self.directory)) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
                _ => {}
            }
        }
        Ok(status)
    }

    /// The elevated phase retires system objects only. The original user
    /// retires the marker in their private World after this returns.
    pub fn uninstall_registration(&self) -> anyhow::Result<Status> {
        self.validate()?;
        let normalized = self.normalized()?;
        #[cfg(windows)]
        let status = windows::uninstall(&normalized)?;
        #[cfg(target_os = "linux")]
        let status = linux::uninstall(&normalized)?;
        #[cfg(target_os = "macos")]
        let status = macos::uninstall(&normalized)?;
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        bail!("fastctx services support Windows, Linux and macOS; this platform is not supported");
        #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
        {
            Ok(status)
        }
    }

    pub fn status(&self) -> anyhow::Result<Status> {
        self.validate()?;
        let normalized = self.normalized()?;
        #[cfg(windows)]
        return windows::status(&normalized);
        #[cfg(target_os = "linux")]
        return linux::status(&normalized);
        #[cfg(target_os = "macos")]
        return macos::status(&normalized);
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        bail!("fastctx services support Windows, Linux and macOS; this platform is not supported")
    }

    /// Whether the manager's persisted definition still names this installation.
    pub fn definition_current(&self) -> anyhow::Result<bool> {
        let service = self.normalized()?;
        #[cfg(windows)]
        return windows::definition_current(&service);
        #[cfg(target_os = "linux")]
        return linux::definition_current(&service);
        #[cfg(target_os = "macos")]
        return macos::definition_current(&service);
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        { let _ = service; Ok(false) }
    }

    /// The process systemd runs the service as, 0 when it runs none.
    #[cfg(target_os = "linux")]
    pub fn main_pid(&self) -> anyhow::Result<u64> {
        self.validate()?;
        linux::main_pid(&self.normalized()?)
    }

    /// Whether the protected guardian already runs this image. Linux's
    /// guardian is systemd itself, so a node image needs no privileged update.
    pub fn guardian_current(&self, digest: &str) -> anyhow::Result<bool> {
        self.validate()?;
        let normalized = self.normalized()?;
        #[cfg(windows)]
        return windows::guardian_current(&normalized, digest);
        #[cfg(not(windows))]
        {
            let _ = (digest, normalized);
            Ok(true)
        }
    }

    /// Stages this approved process's image in an existing protected
    /// registration. It does not stop anything or open the user's World.
    pub fn renew_guardian(&self, digest: &str) -> anyhow::Result<()> {
        self.validate()?;
        let normalized = self.normalized()?;
        #[cfg(windows)]
        return windows::renew_guardian(&normalized, digest);
        #[cfg(not(windows))]
        {
            let _ = (digest, normalized);
            bail!("this platform has no bundled guardian")
        }
    }

    /// Checks whether this process can restart without prompting. On Linux,
    /// a refusal includes the sudo policy or execution error.
    pub fn may_restart(&self) -> anyhow::Result<()> {
        self.validate()?;
        #[cfg(target_os = "linux")]
        {
            let normalized = self.normalized()?;
            linux_restart::may(&normalized.id(&normalized.uid.to_le_bytes()))
        }
        #[cfg(target_os = "macos")]
        return macos::may_restart(&self.normalized()?);
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        Ok(())
    }

    /// As root on Linux: lets the account with `uid` restart this service
    /// from now on, as an install of this release leaves it; elsewhere there
    /// is nothing to allow.
    pub fn allow_restart(&self, uid: u32) -> anyhow::Result<()> {
        self.validate()?;
        #[cfg(target_os = "linux")]
        {
            let normalized = self.normalized()?;
            linux_restart::allow(&normalized.id(&normalized.uid.to_le_bytes()), uid)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = uid;
            Ok(())
        }
    }

    /// Asks the protected guardian to take the World program its
    /// registration names, which it does only for an officially signed
    /// release newer than its own; whether it then holds `digest`. A
    /// guardian of an earlier release, or a platform with none, does not.
    pub fn adopt_guardian(&self, digest: &str) -> anyhow::Result<bool> {
        self.validate()?;
        let normalized = self.normalized()?;
        #[cfg(windows)]
        return windows::adopt_guardian(&normalized, digest);
        #[cfg(not(windows))]
        {
            let _ = (digest, normalized);
            Ok(false)
        }
    }

    pub fn guardian_staged(&self, digest: &str) -> anyhow::Result<bool> {
        self.validate()?;
        let normalized = self.normalized()?;
        #[cfg(windows)]
        return windows::guardian_staged(&normalized, digest);
        #[cfg(not(windows))]
        {
            let _ = (digest, normalized);
            Ok(true)
        }
    }

    /// The target user has SCM stop/start rights. Restarting after staging
    /// needs no second elevation, including after the approving helper died.
    /// The caller holds the startup gate and verifies a serving bridge first.
    pub fn restart_guardian(&self, digest: &str) -> anyhow::Result<()> {
        self.validate()?;
        let normalized = self.normalized()?;
        #[cfg(windows)]
        return windows::restart_guardian(&normalized, digest);
        #[cfg(target_os = "linux")]
        {
            let _ = digest;
            linux_restart::restart(&normalized.id(&normalized.uid.to_le_bytes()))
        }
        #[cfg(target_os = "macos")]
        {
            let _ = digest;
            macos::restart(&normalized)
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            let _ = digest;
            Ok(())
        }
    }

    /// Physical directory spelling is fixed before deriving a registration.
    /// Windows' verbatim prefix is transport syntax, not part of its identity.
    fn normalized(&self) -> anyhow::Result<Self> {
        let directory = physical_path(&self.directory)?;
        // Inspection and removal must remain possible after a delivery image
        // disappears. An existing image also has one registration spelling.
        let executable =
            if self.executable.try_exists()? { physical_path(&self.executable)? } else { self.executable.clone() };
        Ok(Self { directory, executable, ..self.clone() })
    }

    fn validate(&self) -> anyhow::Result<()> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
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

    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
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
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
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

fn physical_path(path: &std::path::Path) -> anyhow::Result<PathBuf> {
    let directory = std::fs::canonicalize(path)?;
    #[cfg(windows)]
    let directory = {
        use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
        let wide: Vec<u16> = directory.as_os_str().encode_wide().collect();
        let normalized = if wide.starts_with(&[92, 92, 63, 92, 85, 78, 67, 92]) {
            [vec![92, 92], wide[8..].to_vec()].concat()
        } else if wide.starts_with(&[92, 92, 63, 92]) {
            wide[4..].to_vec()
        } else {
            wide
        };
        PathBuf::from(std::ffi::OsString::from_wide(&normalized))
    };
    Ok(directory)
}

#[cfg(windows)]
pub fn run_host(name: &str) -> anyhow::Result<()> {
    windows::run_host(name)
}

#[cfg(windows)]
pub fn check_guardian(name: &str) -> anyhow::Result<()> {
    windows::check(name)
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
pub(crate) fn checked(command: &mut Command) -> anyhow::Result<String> {
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
