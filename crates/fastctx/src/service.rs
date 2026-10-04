//! The local service-manager commands; they do not enroll a machine or
//! migrate a registration from an older fastctx.

use std::path::PathBuf;

use clap::{Args, ValueEnum};
use fastctx_install::service::{Kind, Service};

#[derive(Clone, Copy, ValueEnum)]
enum Role {
    Node,
    Hub,
}

#[derive(Clone, Copy, ValueEnum)]
enum Action {
    Install,
    Uninstall,
    Status,
}

#[derive(Args)]
pub struct ServiceArgs {
    #[arg(value_enum)]
    role: Role,
    #[arg(value_enum)]
    action: Action,
    /// The node's fastctx root or the hub's data directory.
    #[arg(long)]
    dir: Option<PathBuf>,
    /// The stable installed fastctx executable; defaults to this executable.
    #[arg(long)]
    executable: Option<PathBuf>,
    /// Linux: run the system service as this UID. Install/uninstall as root.
    #[cfg(target_os = "linux")]
    #[arg(long)]
    uid: Option<u32>,
    /// Windows: own the node as this user's SID. Install/uninstall as administrator.
    #[cfg(windows)]
    #[arg(long)]
    user_sid: Option<String>,
}

pub fn run(args: ServiceArgs) -> anyhow::Result<()> {
    let kind = match args.role {
        Role::Node => Kind::Node,
        Role::Hub => Kind::Hub,
    };
    let directory = match args.dir {
        Some(directory) => std::path::absolute(directory)?,
        None => {
            let root = fastctx_platform::fastctx_root()?;
            if kind == Kind::Hub { fastctx_hub::default_dir(&root) } else { root }
        }
    };
    let executable = match args.executable {
        Some(path) => std::path::absolute(path)?,
        None => std::env::current_exe()?,
    };
    let service = Service::new(kind, executable, directory.clone());
    #[cfg(windows)]
    let service = Service { user_sid: args.user_sid, ..service };
    #[cfg(target_os = "linux")]
    let service = Service { uid: args.uid.unwrap_or(service.uid), ..service };
    // A node a service keeps is started by nobody else; the World says so
    // to its clients while the service is in place.
    let marker = (kind == Kind::Node).then(|| fastctx_types::layout::WorldDir::under(&directory).node_service());
    let status = match args.action {
        Action::Install => {
            let status = service.install()?;
            if let Some(marker) = &marker {
                std::fs::write(marker, b"")?;
            }
            status
        }
        Action::Uninstall => {
            let status = service.uninstall()?;
            if let Some(marker) = &marker {
                match std::fs::remove_file(marker) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
                    _ => {}
                }
            }
            status
        }
        Action::Status => service.status()?,
    };
    println!("{}", serde_json::to_string_pretty(&status)?);
    Ok(())
}
