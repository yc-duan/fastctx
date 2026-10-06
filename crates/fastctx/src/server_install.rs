//! Installing a verified release as a persistent Linux World member.
//! Each operation is repeatable: atomic placement, the node's enrollment,
//! and the system service manager's registration. No enrollment receipt is needed.

#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};

use anyhow::bail;
#[cfg(target_os = "linux")]
use anyhow::{Context as _, ensure};
use clap::Subcommand;
use fastctx_types::MachineName;
#[cfg(target_os = "linux")]
use fastctx_types::{Call, Reply};

#[derive(Subcommand)]
pub enum ServerCommand {
    /// Install this verified release, join with the code, and enable the
    /// node at boot. Only service registration asks for sudo.
    Install {
        /// An opaque World invitation; interpreted by the node.
        code: String,
        #[arg(long)]
        name: Option<MachineName>,
    },
}

pub fn run(command: ServerCommand) -> anyhow::Result<()> {
    let ServerCommand::Install { code, name } = command;
    #[cfg(target_os = "linux")]
    return install(code, name);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (code, name);
        bail!("server installation requires Linux with systemd 249 or newer")
    }
}

#[cfg(target_os = "linux")]
fn install(code: String, name: Option<MachineName>) -> anyhow::Result<()> {
    use fastctx_install::service::{Kind, Service};

    let root = fastctx_platform::fastctx_root()?;
    fastctx_platform::ensure_private_dir(&root)?;
    let _lock = fastctx_platform::FileLock::acquire(&root.join("server-install.lock"))?;
    let installed = fastctx_install::server::place(&root, &std::env::current_exe()?)?;
    println!("Installed fastctx {} at {}", env!("CARGO_PKG_VERSION"), installed.display());
    match crate::ask::call(Call::Join { code, name })?.0 {
        Reply::Joined { world, name } => println!("Joined the World {world} as {name}"),
        other => bail!("the node answered something else: {other:?}"),
    }
    let service = Service::new(Kind::Node, &installed, &root);
    if service.uid == 0 {
        service.install()?;
    } else {
        let arguments = [
            "service".into(),
            "node".into(),
            "install".into(),
            "--dir".into(),
            root.as_os_str().to_owned(),
            "--executable".into(),
            installed.as_os_str().to_owned(),
            "--uid".into(),
            service.uid.to_string().into(),
        ];
        println!("Installing the boot service needs sudo. The node runs as your user (UID {}).", service.uid);
        println!(
            "sudo {} service node install --dir {} --executable {} --uid {}",
            quote(&installed),
            quote(&root),
            quote(&installed),
            service.uid
        );
        let status = Command::new("sudo")
            .arg(&installed)
            .args(&arguments)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("starting sudo for the node's boot service")?;
        ensure!(
            status.success(),
            "sudo service registration failed ({status}); run the same installation command to resume"
        );
    }
    hand_over_to_service(&root, &installed)?;
    println!("The system service is enabled at boot and keeps this World running without a login.");
    Ok(())
}

#[cfg(target_os = "linux")]
fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

#[cfg(target_os = "linux")]
fn hand_over_to_service(root: &Path, installed: &Path) -> anyhow::Result<()> {
    use fastctx_install::service::{Kind, Service};
    use fastctx_platform::ipc::{self, Endpoint};
    use fastctx_types::layout::WorldDir;

    let status = Service::new(Kind::Node, installed, root).status()?;
    ensure!(status.installed && status.enabled, "the boot service is not installed and enabled");
    let unit = status
        .name
        .bytes()
        .map(|byte| if byte.is_ascii_alphanumeric() { (byte as char).to_string() } else { format!("_{byte:02x}") })
        .collect::<String>();
    // Read only MainPID: systemctl show also requests unrelated properties,
    // which cannot be serialized when ExecStart contains non-UTF-8 paths.
    let output = Command::new("busctl")
        .args([
            "--system",
            "--json=short",
            "get-property",
            "org.freedesktop.systemd1",
            &format!("/org/freedesktop/systemd1/unit/{unit}"),
            "org.freedesktop.systemd1.Service",
            "MainPID",
        ])
        .output()?;
    ensure!(
        output.status.success(),
        "busctl could not identify the node service: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let service_pid = result.get("data").and_then(serde_json::Value::as_u64).context("systemd omitted its MainPID")?;
    let world = WorldDir::under(root);
    let connection = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(ipc::connect(&Endpoint::for_world(world.path())?))?;
    let pid = connection.server_process().context("the OS did not identify the serving node")?.pid;
    if u64::from(pid) != service_pid {
        crate::ask::call(Call::HandOver)?;
        println!("The temporary node is handing the World to its boot service once idle.");
    }
    Ok(())
}
