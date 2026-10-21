//! Creating a World on this computer (`world-network.md`, 一个中枢，几个账号):
//! this computer becomes the World's hub, its first account the hub's super
//! administrator, and then logs in to it as any computer would. The hub runs
//! as a system service from the fastctx this application placed, asked for
//! administrator rights once; other computers reach it at the addresses the
//! person chose, which the drawing of this computer's network suggests.

use std::net::{IpAddr, SocketAddr};

use serde::Serialize;

use crate::Shell;
use crate::accounts::LoggedIn;

/// An address of this computer another one might reach it at.
#[derive(Serialize)]
pub struct Address {
    pub interface: String,
    pub address: String,
    /// Ethernet or Wi-Fi with a way out, not a TUN, VPN or virtual switch.
    pub physical: bool,
}

/// This computer's IPv4 addresses, the physical ones with a gateway first.
#[tauri::command]
pub(crate) fn here_addresses() -> Result<Vec<Address>, String> {
    let mut interfaces = fastctx_net::route::interfaces()?;
    interfaces.sort_by_key(|interface| (!interface.physical, !interface.gateway, interface.metric));
    Ok(interfaces
        .iter()
        .flat_map(|interface| {
            interface.ipv4.iter().filter(|address| !address.is_loopback() && !address.is_link_local()).map(|address| {
                Address {
                    interface: interface.name.clone(),
                    address: address.to_string(),
                    physical: interface.physical,
                }
            })
        })
        .collect())
}

/// Makes this computer the hub of a new World named `name`, with `admin`
/// its super administrator, reached at `public` (each IP:PORT); runs the hub
/// as a service; and logs in to it as `admin`, registering this computer as
/// one of its machines when `register`.
#[fastctx_gui_command::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_world(
    shell: tauri::State<'_, Shell>,
    name: String,
    admin: String,
    password: String,
    public: Vec<String>,
    register: bool,
    machine: Option<String>,
) -> Result<LoggedIn, String> {
    shell.connection.local_program()?;
    let public = public
        .iter()
        .map(|text| {
            text.trim()
                .parse::<SocketAddr>()
                .map_err(|_| format!("{text} is not an address with a port, as 192.168.1.20:7443"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let first = *public.first().ok_or("give at least one address other computers reach this one at")?;
    let dir = {
        let (name, admin, password, public) = (name.clone(), admin.clone(), password.clone(), public.clone());
        tauri::async_runtime::spawn_blocking(move || found(&name, &admin, &password, &public))
            .await
            .map_err(|error| error.to_string())??
    };
    let places = crate::setup::places()?;
    let kept = tauri::async_runtime::spawn_blocking(move || {
        #[cfg(windows)]
        let (kept, node_kept) =
            fastctx_install::service::keep_local_network(&places, &dir).map_err(|error| error.to_string())?;
        #[cfg(not(windows))]
        let kept = fastctx_install::service::keep_hub(&places, &dir).map_err(|error| error.to_string())?;
        #[cfg(target_os = "linux")]
        let kept = if matches!(kept, fastctx_install::service::Kept::Command(_)) {
            crate::linux_service::run(fastctx_install::service::Kind::Hub, "install", &dir, &places.installed())?
        } else {
            kept
        };
        #[cfg(target_os = "macos")]
        let kept = if matches!(kept, fastctx_install::service::Kept::Command(_)) {
            hub_as_administrator(&dir, &places.installed())?
        } else {
            kept
        };
        #[cfg(windows)]
        {
            Ok::<_, String>((kept, node_kept))
        }
        #[cfg(not(windows))]
        {
            Ok::<_, String>(kept)
        }
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    use fastctx_install::service::Kept;
    #[cfg(windows)]
    let (kept, node_kept) = kept;
    match kept {
        Kept::Installed | Kept::Already => {}
        Kept::Declined => {
            return Err("the hub is made, but administrator rights were declined, so it is not running".into());
        }
        Kept::Unsupported | Kept::Command(_) => return Err("this system cannot run the hub as a service".into()),
    }
    // This computer reaches its own hub without leaving itself.
    let local = SocketAddr::new(IpAddr::from([127, 0, 0, 1]), first.port());
    let addresses = fastctx_client::hub_addresses(&local.to_string()).await?;
    let admin = fastctx_types::AccountName::try_from(admin).map_err(|error| error.to_string())?;
    let joined = shell
        .world()
        .login(addresses, admin, password, register, crate::new_machine_name(machine)?)
        .await
        .map_err(|error| error.to_string())?;
    #[cfg(windows)]
    {
        let kept = crate::setup::finish_keep(&shell.world(), node_kept).await?;
        Ok(crate::accounts::logged_in_kept(joined, kept))
    }
    #[cfg(not(windows))]
    crate::accounts::logged_in(&shell, joined).await
}

/// macOS asks for an administrator in its own dialog: the hub is a
/// LaunchDaemon, installed by the World program run as root.
#[cfg(target_os = "macos")]
fn hub_as_administrator(
    dir: &std::path::Path,
    executable: &std::path::Path,
) -> Result<fastctx_install::service::Kept, String> {
    let args: Vec<std::ffi::OsString> = vec![
        "service".into(),
        "hub".into(),
        "install".into(),
        "--dir".into(),
        dir.into(),
        "--executable".into(),
        executable.into(),
    ];
    match fastctx_platform::elevation::run_elevated(executable, &args) {
        Ok(0) => Ok(fastctx_install::service::Kept::Installed),
        Ok(code) => Err(format!("installing the hub service failed (exit {code})")),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            Ok(fastctx_install::service::Kept::Declined)
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Places fastctx where it runs from, and has it make the hub; the hub's
/// directory.
fn found(name: &str, admin: &str, password: &str, public: &[SocketAddr]) -> Result<std::path::PathBuf, String> {
    use std::io::Write as _;
    let places = crate::setup::places()?;
    if !places.installed().is_file() {
        crate::setup::place()?;
    }
    let dir = places.root.join("hub");
    // A hub made here before, by an attempt whose service was declined, is
    // taken up again when it has this name; the login that follows checks
    // the password. One of a network dissolved here, still telling the
    // machines that were away, is ended first (`world-network.md`, 解散网络).
    match made_here(&places.installed(), &dir)? {
        Some(Made { dissolved: true, .. }) => retire(&places.installed(), &dir)?,
        Some(Made { name: made, .. }) if made == name => return Ok(dir),
        Some(Made { name: made, .. }) => {
            return Err(format!("this computer is already the hub of the World {made}, in {}", dir.display()));
        }
        None => {}
    }
    let mut command = std::process::Command::new(places.installed());
    command.args(["hub", "init", "--name", name, "--admin", admin, "--dir"]).arg(&dir);
    for address in public {
        command.arg("--public").arg(address.to_string());
    }
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    // Without a terminal, fastctx reads the password from its first line.
    child
        .stdin
        .take()
        .ok_or("no way to give fastctx the password")?
        .write_all(format!("{password}\n").as_bytes())
        .map_err(|error| error.to_string())?;
    let output = child.wait_with_output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(dir)
}

/// A hub kept on this computer: its name, and whether its network was
/// dissolved.
struct Made {
    name: String,
    dissolved: bool,
}

/// Ends the hub of a network dissolved here with the rights it takes, asked
/// for once: its service is removed and its data deleted, all but its log.
fn retire(fastctx: &std::path::Path, dir: &std::path::Path) -> Result<(), String> {
    let args: Vec<std::ffi::OsString> = vec!["hub".into(), "retire".into(), "--dir".into(), dir.into()];
    #[cfg(any(windows, target_os = "macos"))]
    let ended = if fastctx_platform::elevation::is_elevated().map_err(|error| error.to_string())? {
        let mut command = std::process::Command::new(fastctx);
        command.args(&args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(0x08000000);
        }
        let output = command.output().map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        Ok(())
    } else {
        match fastctx_platform::elevation::run_elevated(fastctx, &args) {
            Ok(0) => Ok(()),
            Ok(code) => Err(format!("ending the dissolved network's hub here failed (exit {code})")),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                Err("administrator rights were declined; the dissolved network's hub still runs here".to_string())
            }
            Err(error) => Err(error.to_string()),
        }
    };
    #[cfg(target_os = "linux")]
    let ended = {
        let output = std::process::Command::new("pkexec")
            .arg(fastctx)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| format!("starting the administrator prompt: {error}"))?;
        match output.status.code() {
            Some(0) => Ok(()),
            Some(126) => {
                Err("administrator rights were declined; the dissolved network's hub still runs here".to_string())
            }
            _ => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
        }
    };
    ended
}

/// The hub kept in `dir`, if there is one.
fn made_here(fastctx: &std::path::Path, dir: &std::path::Path) -> Result<Option<Made>, String> {
    if !dir.is_dir() {
        return Ok(None);
    }
    let mut command = std::process::Command::new(fastctx);
    command.args(["hub", "show", "--dir"]).arg(dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x08000000);
    }
    let output = command.output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Ok(None);
    }
    // "hub NAME (ID)" on the first line; "dissolved by …" on one of its own.
    let text = String::from_utf8_lossy(&output.stdout);
    let dissolved = text.lines().any(|line| line.starts_with("dissolved "));
    Ok(text
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("hub "))
        .and_then(|rest| rest.split_whitespace().next())
        .map(|name| Made { name: name.to_string(), dissolved }))
}
