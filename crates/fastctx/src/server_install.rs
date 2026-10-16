//! Installing a verified release as a machine of an account, on a Linux
//! server or a Windows computer, from the line made for it.
//! Each operation is repeatable: atomic placement, the node's enrollment,
//! and the system service manager's registration. No enrollment receipt is needed.

#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};

#[cfg(not(windows))]
use anyhow::bail;
#[cfg(target_os = "linux")]
use anyhow::{Context as _, ensure};
use clap::Subcommand;
use fastctx_types::MachineName;
#[cfg(target_os = "linux")]
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
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (code, name, account_part);
        bail!("server installation requires Linux with systemd 249 or newer, or Windows")
    }
}

/// A Windows computer comes in by its line (`world-network.md`, 受管机器):
/// run by a person in an administrator's prompt, its World is that person's
/// account's, as on Linux; run by SYSTEM, as a remote management tool runs
/// it, it is an account's of its own (`fastctx_install::service::managed`).
#[cfg(windows)]
mod windows {
    use std::ffi::OsString;

    use anyhow::{Context as _, bail, ensure};
    use fastctx_install::service::{Kind, Service, managed};
    use fastctx_types::{Call, MachineName, Reply};

    const SYSTEM: &str = "S-1-5-18";

    pub fn install(code: String, name: Option<MachineName>, account_part: bool) -> anyhow::Result<()> {
        if fastctx_platform::current_user_sid()? == SYSTEM {
            ensure!(!account_part, "the account's part is never SYSTEM's");
            return as_system(code, name);
        }
        let root = fastctx_platform::fastctx_root()?;
        // The program is placed for this account and runs from there; one
        // already there and newer stays (`world-upgrade.md`, 版本只往前走).
        if let Some(result) = crate::program::deliver(&root)? {
            return result;
        }
        // The same code again, from this computer, finds it logged in already.
        match crate::ask::call(Call::LoginWithToken { token: code, name })?.0 {
            Reply::LoggedIn { hub, name, account } => {
                println!("This machine is {name}, a machine of {account} on the hub {hub}")
            }
            other => bail!("the node answered something else: {other:?}"),
        }
        if account_part {
            // SYSTEM installs the service; once it has, the node this
            // account started hands the World to the service's.
            if fastctx_types::layout::WorldDir::under(&root).node_service().exists() {
                crate::ask::call(Call::HandOver)?;
            }
            return Ok(());
        }
        crate::keep::keep_connected()
    }

    /// As SYSTEM: the account is made or found, what belongs to its World
    /// is done as it, and the service that keeps its node is installed for
    /// it; then the node the account started hands the World over.
    fn as_system(code: String, name: Option<MachineName>) -> anyhow::Result<()> {
        let sid = managed::account()?;
        println!("Fastctx lives in the account {} on this computer", managed::ACCOUNT);
        let program = std::env::current_exe()?;
        let mut args: Vec<OsString> = vec!["server".into(), "install".into(), code.into()];
        if let Some(name) = name {
            args.extend(["--name".into(), name.as_str().into()]);
        }
        args.push("--account-part".into());
        let (code, home) = managed::run(&sid, &program, &args)?;
        ensure!(
            code == 0,
            "placing and joining as {} failed (exit {code}); run the line again to resume",
            managed::ACCOUNT
        );
        let root = fastctx_platform::fastctx_root_in(&home);
        let installed = fastctx_install::renewal::installed(&root);
        let mut service = Service::new(Kind::Node, &installed, &root);
        service.user_sid = Some(sid.clone());
        service.install().context("installing the boot service")?;
        println!("The system service is enabled at boot and keeps this World running without a login.");
        // The second time only hands over; the code finds it logged in.
        let (code, _) = managed::run(&sid, &installed, &args)?;
        ensure!(
            code == 0,
            "handing the World to its service failed (exit {code}); it hands over when the computer starts again"
        );
        Ok(())
    }
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
    match crate::ask::call(Call::LoginWithToken { token: code, name })?.0 {
        Reply::LoggedIn { hub, name, account } => {
            println!("This machine is {name}, a machine of {account} on the hub {hub}")
        }
        other => bail!("the node answered something else: {other:?}"),
    }
    let service = Service::new(Kind::Node, &installed, &root);
    let was_enabled = service.status().is_ok_and(|status| status.installed && status.enabled);
    if was_enabled {
        // Registering again changes nothing that runs.
    } else if service.uid == 0 {
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
