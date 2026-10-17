//! Installing a verified release as a machine of an account, on a Linux
//! server, a Windows computer or a Mac, from the line made for it.
//! Each operation is repeatable: atomic placement, the node's enrollment,
//! and the system service manager's registration. No enrollment receipt is needed.

#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};

#[cfg(target_os = "linux")]
use anyhow::ensure;
use anyhow::{Context as _, bail};
use clap::Subcommand;
use fastctx_types::MachineName;
use fastctx_types::{Call, Reply};

#[derive(Subcommand)]
pub enum ServerCommand {
    /// Install this verified release, log in with the token as a machine of
    /// its account, and enable the node at boot. Only service registration
    /// asks for sudo.
    Install {
        /// A one-time enrollment token; interpreted by the node.
        code: String,
        #[arg(long)]
        name: Option<MachineName>,
        /// On Windows, the part done as the account SYSTEM installs for:
        /// placing its program and joining, the service being SYSTEM's.
        #[arg(long, hide = true)]
        account_part: bool,
    },
}

pub fn run(command: ServerCommand) -> anyhow::Result<()> {
    let ServerCommand::Install { code, name, account_part } = command;
    #[cfg(target_os = "linux")]
    {
        let _ = account_part;
        install(code, name)
    }
    #[cfg(windows)]
    return windows::install(code, name, account_part);
    #[cfg(target_os = "macos")]
    {
        let _ = account_part;
        here(code, name, false)
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = (code, name, account_part);
        bail!("server installation requires Linux with systemd 249 or newer, Windows or macOS")
    }
}

/// On Windows and macOS: the program placed for whoever runs this, the
/// machine joined with the code, and its node kept running from boot by
/// the system's service manager, which hands the World over to it. Run as
/// root on a Mac, that node is a LaunchDaemon; run as a person, it is
/// theirs, as when the application installs it.
#[cfg(any(windows, target_os = "macos"))]
fn here(code: String, name: Option<MachineName>, account_part: bool) -> anyhow::Result<()> {
    let root = fastctx_platform::fastctx_root()?;
    // The program is placed for this account and runs from there; one
    // already there and newer stays (`world-upgrade.md`, 版本只往前走).
    if let Some(result) = crate::program::deliver(&root)? {
        return result;
    }
    // The same code again, from this computer, finds it logged in already.
    let joined = match crate::ask::call(Call::LoginWithToken { token: code, name }).context("joining with the code")?.0
    {
        Reply::LoggedIn { hub, name, account } => joined(&name, &account, &hub),
        other => bail!("the node answered something else: {other:?}"),
    };
    if account_part {
        // A line of 1.0.1-beta.3 run by SYSTEM: SYSTEM installs the
        // service; once it has, the node this account started hands the
        // World to the service's.
        if fastctx_types::layout::WorldDir::under(&root).node_service().exists() {
            crate::ask::call(Call::HandOver)?;
        }
        return Ok(());
    }
    crate::keep::keep_connected().context("keeping the node running at boot")?;
    println!("{joined}");
    Ok(())
}

/// A Windows computer comes in by its line (`world-network.md`, 登录与入网):
/// its World is whoever runs it. Run by a person in an administrator's
/// prompt, it is that person's account's, as on Linux; run by SYSTEM, as a
/// remote management tool runs it, it is SYSTEM's own, in the home Fastctx
/// gives SYSTEM (`fastctx_platform::system_home`).
#[cfg(windows)]
mod windows {
    use anyhow::{Context as _, bail, ensure};
    use fastctx_install::service::managed;
    use fastctx_types::MachineName;

    const SYSTEM: &str = "S-1-5-18";

    pub fn install(code: String, name: Option<MachineName>, account_part: bool) -> anyhow::Result<()> {
        if fastctx_platform::current_user_sid()? == SYSTEM {
            ensure!(!account_part, "the account's part is never SYSTEM's");
            beside_the_account()?;
            fastctx_platform::system_home::prepare().context("making SYSTEM's home for Fastctx")?;
        }
        super::here(code, name, account_part)
    }

    /// A computer 1.0.1-beta.3 installed from its line by SYSTEM keeps its
    /// World in an account of its own, and stays so; that account, left
    /// with no World kept in it, is taken away.
    fn beside_the_account() -> anyhow::Result<()> {
        let Some(sid) = managed::made()? else { return Ok(()) };
        if managed::keeps_a_world(&sid)? {
            bail!(
                "Fastctx already runs on this computer in the account {}, as 1.0.1-beta.3 installed it, and stays so; \
                 to have it run as SYSTEM, remove this machine from its network first",
                managed::ACCOUNT
            );
        }
        for left in managed::remove(&sid)? {
            println!("could not clear: {left}");
        }
        println!("Removed the account {} an earlier Fastctx left on this computer", managed::ACCOUNT);
        Ok(())
    }
}

/// The last line an installation prints: whose machine this is now.
fn joined(name: &impl std::fmt::Display, account: &impl std::fmt::Display, hub: &impl std::fmt::Display) -> String {
    format!("Installed: this machine is {name}, a machine of {account} on the hub {hub}")
}

#[cfg(target_os = "linux")]
fn install(code: String, name: Option<MachineName>) -> anyhow::Result<()> {
    use fastctx_install::service::{Kind, Service};

    let root = fastctx_platform::fastctx_root()?;
    if let Some(result) = crate::program::deliver(&root)? {
        return result;
    }
    fastctx_platform::ensure_private_dir(&root)?;
    let _lock = fastctx_platform::FileLock::acquire(&root.join("server-install.lock"))?;
    // Run again, the line says what was there already rather than what it did.
    let was_installed = fastctx_install::executable::installed_path(&root).try_exists()?;
    let installed = fastctx_install::server::place(&root, &std::env::current_exe()?)?;
    if was_installed {
        println!("fastctx {} is installed at {} already", env!("CARGO_PKG_VERSION"), installed.display());
    } else {
        println!("Installed fastctx {} at {}", env!("CARGO_PKG_VERSION"), installed.display());
    }
    // The same token again, from this computer, finds it logged in already.
    let joined = match crate::ask::call(Call::LoginWithToken { token: code, name }).context("joining with the code")?.0
    {
        Reply::LoggedIn { hub, name, account } => joined(&name, &account, &hub),
        other => bail!("the node answered something else: {other:?}"),
    };
    let service = Service::new(Kind::Node, &installed, &root);
    let was_enabled = service.status().is_ok_and(|status| status.installed && status.enabled);
    if was_enabled {
        // Registering again changes nothing that runs.
    } else if service.uid == 0 {
        service.install().context("installing the boot service")?;
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
        let mut sudo = Command::new("sudo");
        // Without a terminal, as over SSH from the app, sudo's password is the
        // line on standard input.
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            sudo.args(["-S", "-p", ""]);
        }
        let status = sudo
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
    if was_enabled {
        println!("Its boot service was enabled already and keeps this World running without a login.");
    } else {
        println!("The system service is enabled at boot and keeps this World running without a login.");
    }
    println!("{joined}");
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
