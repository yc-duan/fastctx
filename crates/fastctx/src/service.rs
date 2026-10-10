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
    #[arg(long, value_parser = crate::paths::parser())]
    dir: Option<PathBuf>,
    /// The executable to register; defaults to the stable World program.
    #[arg(long, value_parser = crate::paths::parser())]
    executable: Option<PathBuf>,
    /// Linux: run the system service as this UID. Install/uninstall as root.
    #[cfg(target_os = "linux")]
    #[arg(long)]
    uid: Option<u32>,
    #[cfg(target_os = "linux")]
    #[arg(long, hide = true)]
    package_owner: Option<String>,
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
        None => {
            let root = if kind == Kind::Node { directory.clone() } else { fastctx_platform::fastctx_root()? };
            match args.action {
                Action::Install => {
                    if let Some(result) = crate::program::deliver(&root)? {
                        return result;
                    }
                    fastctx_install::executable::installed_path(&root)
                }
                Action::Uninstall | Action::Status => fastctx_install::executable::installed_path(&root),
            }
        }
    };
    let service = Service::new(kind, executable, directory);
    #[cfg(windows)]
    let service = Service { user_sid: args.user_sid, ..service };
    #[cfg(target_os = "linux")]
    let service = Service { uid: args.uid.unwrap_or(service.uid), ..service };
    let status = match args.action {
        Action::Install => service.install()?,
        Action::Uninstall => service.uninstall()?,
        Action::Status => service.status()?,
    };
    #[cfg(target_os = "linux")]
    if matches!(args.action, Action::Install) && let Some(owner) = args.package_owner {
        fastctx_uninstall::package::remember(serde_json::from_str(&owner)?)?;
    }
    println!("{}", serde_json::to_string_pretty(&status)?);
    Ok(())
}
