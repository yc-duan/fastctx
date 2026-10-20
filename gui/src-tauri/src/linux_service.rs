//! Graphical service installation uses polkit; the command-line installer
//! retains its explicit sudo command. Arguments never pass through a shell.

use std::path::Path;
use std::process::{Command, Stdio};

use fastctx_install::service::{Kept, Kind};

pub fn run(kind: Kind, action: &str, directory: &Path, executable: &Path) -> Result<Kept, String> {
    use std::os::unix::fs::MetadataExt as _;
    let role = match kind {
        Kind::Node => "node",
        Kind::Hub => "hub",
    };
    let mut command = Command::new("pkexec");
    command.arg(executable).args(["service", role, action, "--dir"]).arg(directory).arg("--executable").arg(executable);
    if kind == Kind::Node {
        command.arg("--uid").arg(std::fs::metadata(directory).map_err(|error| error.to_string())?.uid().to_string());
    }
    if action == "install" {
        let mut owner = fastctx_install::uninstall::Installation::here().map_err(|error| error.to_string())?;
        owner.application = Some(std::env::current_exe().map_err(|error| error.to_string())?);
        owner.application_dirs =
            fastctx_install::uninstall::application_directories(&owner.home).map_err(|error| error.to_string())?;
        command.arg("--package-owner").arg(serde_json::to_string(&owner).map_err(|error| error.to_string())?);
    }
    let output =
        command.stdin(Stdio::null()).output().map_err(|error| format!("starting the administrator prompt: {error}"))?;
    match output.status.code() {
        Some(0) => Ok(Kept::Installed),
        Some(126) => Ok(Kept::Declined),
        _ => Err(format!(
            "the {role} service {action} failed ({}): {}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )),
    }
}

/// Waits for the service's process to own the IPC endpoint after the old
/// node hands over. No World call is sent while the serving identity changes.
pub fn wait_for_node(root: &Path, executable: &Path) -> Result<(), String> {
    use fastctx_install::service::Service;
    use fastctx_platform::ipc::{self, Endpoint};
    use fastctx_types::layout::WorldDir;
    let service = Service::new(Kind::Node, executable, root);
    let runtime =
        tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|error| error.to_string())?;
    let world = WorldDir::under(root);
    loop {
        let status = service.status().map_err(|error| error.to_string())?;
        if !status.installed || !status.enabled || status.state != "active/running" {
            return Err(format!("the node service is {}", status.state));
        }
        let pid = service.main_pid().map_err(|error| error.to_string())?;
        let endpoint = Endpoint::for_world(world.path()).map_err(|error| error.to_string())?;
        match runtime.block_on(ipc::connect(&endpoint)) {
            Ok(connection) => {
                // Linux loses the live identity if the old peer exits
                // between connect and reading its process start time.
                if let Some(peer) = connection.server_process()
                    && u64::from(peer.pid) == pid
                {
                    return Ok(());
                }
            }
            Err(error)
                if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) => {}
            Err(error) => return Err(error.to_string()),
        }
        // Match the core client's startup polling interval; readiness is
        // the OS-reported serving process, never an elapsed startup delay.
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
