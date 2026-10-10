//! Logging in and out, and what an account does (`world-network.md`, 登录与
//! 入网): each command is one call of the core's client. A computer that
//! logs in with its own node is kept connected by a system service
//! afterwards, as `fastctx login` keeps it; a device keeps nothing.

use fastctx_types::accounts::{AccountChange, AccountsReply};
use fastctx_types::{AccountName, HubName, MachineName};
use serde::Serialize;

use crate::Shell;

/// A login, and on a computer what came of keeping it connected.
#[derive(Serialize)]
pub struct LoggedIn {
    hub: HubName,
    name: MachineName,
    account: AccountName,
    #[cfg(desktop)]
    kept: Option<crate::setup::KeptNow>,
}

fn account(name: String) -> Result<AccountName, String> {
    AccountName::try_from(name).map_err(|error| error.to_string())
}

fn hub(name: Option<String>) -> Result<Option<HubName>, String> {
    name.map(|name| HubName::try_from(name).map_err(|error| error.to_string())).transpose()
}

pub(crate) async fn logged_in(shell: &Shell, (hub, name, account): (HubName, MachineName, AccountName)) -> Result<LoggedIn, String> {
    #[cfg(desktop)]
    let kept = match shell.connection.local_world() {
        Ok(world) => Some(crate::setup::keep(&world).await?),
        Err(_) => None,
    };
    #[cfg(not(desktop))]
    let _ = shell;
    Ok(LoggedIn {
        hub,
        name,
        account,
        #[cfg(desktop)]
        kept,
    })
}

/// Logs in to `account` at the hub written `hub` with its password, and
/// on a computer registers it as one of the account's machines when
/// `register`.
#[fastctx_gui_command::command]
pub(crate) async fn login(
    shell: tauri::State<'_, Shell>,
    hub: String,
    account_name: String,
    password: String,
    register: bool,
    name: Option<String>,
) -> Result<LoggedIn, String> {
    let addresses = fastctx_client::hub_addresses(&hub).await?;
    let world = shell.world();
    #[cfg(target_os = "android")]
    crate::android::joining(true)?;
    let joined = world
        .login(addresses, account(account_name)?, password, register, crate::machine(name)?)
        .await
        .map_err(|error| error.to_string());
    #[cfg(target_os = "android")]
    crate::android::joining(false)?;
    logged_in(&shell, joined?).await
}

/// Logs in with a one-time token another device made.
#[fastctx_gui_command::command]
pub(crate) async fn login_token(
    shell: tauri::State<'_, Shell>,
    token: String,
    name: Option<String>,
) -> Result<LoggedIn, String> {
    let world = shell.world();
    #[cfg(target_os = "android")]
    crate::android::joining(true)?;
    let joined = world.login_with_token(token.trim().to_string(), crate::machine(name)?).await.map_err(|error| error.to_string());
    #[cfg(target_os = "android")]
    crate::android::joining(false)?;
    logged_in(&shell, joined?).await
}

/// Opens `account` on a hub that lets anyone, and logs in to it.
#[fastctx_gui_command::command]
pub(crate) async fn open_account(
    shell: tauri::State<'_, Shell>,
    hub: String,
    account_name: String,
    password: String,
    register: bool,
    name: Option<String>,
) -> Result<LoggedIn, String> {
    let addresses = fastctx_client::hub_addresses(&hub).await?;
    let joined = shell
        .world()
        .open_account(addresses, account(account_name)?, password, register, crate::machine(name)?)
        .await
        .map_err(|error| error.to_string())?;
    logged_in(&shell, joined).await
}

/// Every hub this computer or device is on, with what it sees of each.
#[fastctx_gui_command::command]
pub(crate) async fn accounts(shell: tauri::State<'_, Shell>) -> Result<AccountsReply, String> {
    shell.world().accounts().await.map_err(|error| error.to_string())
}

/// Makes `change` as `account` at the hub named `hub`, or the one hub;
/// returns a token when it made one.
#[fastctx_gui_command::command]
pub(crate) async fn account_change(
    shell: tauri::State<'_, Shell>,
    hub_name: Option<String>,
    account_name: String,
    change: AccountChange,
) -> Result<Option<String>, String> {
    shell.world().account(hub(hub_name)?, account(account_name)?, change).await.map_err(|error| error.to_string())
}

/// The line that installs fastctx on a Linux server and registers it as a
/// machine of `account`, valid for `hours`.
#[fastctx_gui_command::command]
pub(crate) async fn server_command(
    shell: tauri::State<'_, Shell>,
    hub_name: Option<String>,
    account_name: String,
    name: Option<String>,
    hours: u32,
) -> Result<String, String> {
    let expires = jiff::Timestamp::now()
        .checked_add(jiff::SignedDuration::from_hours(i64::from(hours)))
        .map_err(|error| error.to_string())?;
    shell
        .world()
        .server_command(hub(hub_name)?, account(account_name)?, crate::machine(name)?, expires)
        .await
        .map_err(|error| error.to_string())
}

/// Sets where a conversation works: a machine as written, or the one it
/// lives on.
#[fastctx_gui_command::command]
pub(crate) async fn works_on(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    works_on: Option<String>,
) -> Result<(), String> {
    let works_on = works_on
        .map(|text| text.parse::<fastctx_types::access::Written>().map_err(|error| error.to_string()))
        .transpose()?;
    shell
        .world()
        .works_on(crate::resident(resident_id)?, crate::machine(machine_name)?, works_on)
        .await
        .map_err(|error| error.to_string())
}

/// Turns asking for colleagues on for a conversation, with where they
/// live and the Codex home they use there, or off.
#[fastctx_gui_command::command]
pub(crate) async fn colleagues(
    shell: tauri::State<'_, Shell>,
    resident_id: String,
    machine_name: Option<String>,
    lives_on: Option<String>,
    home: Option<std::path::PathBuf>,
) -> Result<(), String> {
    let lives_on = lives_on
        .map(|text| text.parse::<fastctx_types::access::Written>().map_err(|error| error.to_string()))
        .transpose()?;
    shell
        .world()
        .colleagues(crate::resident(resident_id)?, crate::machine(machine_name)?, lives_on, home)
        .await
        .map_err(|error| error.to_string())
}

/// Adds an agent's home on a machine, or hides one or shows it again.
#[fastctx_gui_command::command]
pub(crate) async fn agent_homes(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    change: fastctx_types::agents::HomeChange,
) -> Result<(), String> {
    let machine = machine_name.parse().map_err(|error| format!("{error}"))?;
    shell.world().homes(machine, change).await.map_err(|error| error.to_string())
}
