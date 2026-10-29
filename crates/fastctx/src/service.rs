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
    /// Stops the service and keeps it stopped, for maintenance, until it is
    /// released: nothing that keeps it running starts it meanwhile.
    Hold,
    /// Lets a held service run again, and starts it.
    Release,
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
    // What keeps a managed machine connected stays while its owner keeps it,
    // unless an administrator of this computer takes it off.
    if kind == Kind::Node
        && matches!(args.action, Action::Uninstall)
        && let Some(managed) = fastctx_uninstall::managed(&fastctx_types::layout::WorldDir::under(&directory))?
        && !fastctx_platform::elevation::is_administrator()?
    {
        anyhow::bail!("{}", managed.refusal());
    }
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
                Action::Uninstall | Action::Status | Action::Hold | Action::Release => {
                    fastctx_install::executable::installed_path(&root)
                }
            }
        }
    };
    let kept_before = fastctx_types::layout::WorldDir::under(&directory).node_service().exists();
    let service = Service::new(kind, executable, directory.clone());
    #[cfg(windows)]
    let service = Service { user_sid: args.user_sid, ..service };
    #[cfg(target_os = "linux")]
    let service = Service { uid: args.uid.unwrap_or(service.uid), ..service };
    let status = match args.action {
        Action::Install => service.install()?,
        Action::Uninstall => service.uninstall()?,
        Action::Status => service.status()?,
        Action::Hold => service.hold()?,
        Action::Release => service.release()?,
    };
    // As when this computer joins a World (`crate::keep`): the node that
    // served until now hands the World to the service's.
    if kind == Kind::Node && matches!(args.action, Action::Install) && !kept_before {
        hand_over(&directory);
    }
    #[cfg(target_os = "linux")]
    if matches!(args.action, Action::Install)
        && let Some(owner) = args.package_owner
    {
        fastctx_uninstall::package::remember(serde_json::from_str(&owner)?)?;
    }
    println!("{}", serde_json::to_string_pretty(&status)?);
    Ok(())
}

/// Asks the node serving the World under `root`, when one does, to hand it
/// over once idle. None is started for it.
fn hand_over(root: &std::path::Path) {
    let world = fastctx_types::layout::WorldDir::under(root);
    let asked =
        tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(anyhow::Error::from).and_then(
            |runtime| {
                runtime.block_on(async {
                    match fastctx_client::Client::connect(&world, fastctx_client::command_peer()?).await {
                        Ok(mut client) => match client.call(fastctx_types::Call::HandOver).await? {
                            fastctx_types::Reply::Done => Ok(()),
                            other => Err(anyhow::anyhow!("the node answered {other:?}")),
                        },
                        Err(fastctx_client::ConnectError::NoNode { .. }) => Ok(()),
                        Err(error) => Err(error.into()),
                    }
                })
            },
        );
    if let Err(error) = asked {
        eprintln!("fastctx: the node serving until now was not asked to hand the World over: {error:#}");
    }
}
