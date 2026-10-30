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
    /// Why keeping the node running from boot failed, the machine being in
    /// all the same.
    #[cfg(desktop)]
    unkept: Option<String>,
}

fn account(name: String) -> Result<AccountName, String> {
    AccountName::try_from(name).map_err(|error| error.to_string())
}

fn hub(name: Option<String>) -> Result<Option<HubName>, String> {
    name.map(|name| HubName::try_from(name).map_err(|error| error.to_string())).transpose()
}

pub(crate) async fn logged_in(
    shell: &Shell,
    (hub, name, account): (HubName, MachineName, AccountName),
) -> Result<LoggedIn, String> {
    // In already: what goes wrong keeping the node running from boot is
    // said beside the login, never instead of it.
    #[cfg(desktop)]
    let (kept, unkept) = match shell.connection.local_world() {
        Ok(world) => match crate::setup::keep(&world).await {
            Ok(kept) => (Some(kept), None),
            Err(error) => (None, Some(error)),
        },
        Err(_) => (None, None),
    };
    #[cfg(not(desktop))]
    let _ = shell;
    Ok(LoggedIn {
        hub,
        name,
        account,
        #[cfg(desktop)]
        kept,
        #[cfg(desktop)]
        unkept,
    })
}

#[cfg(windows)]
pub(crate) fn logged_in_kept(
    (hub, name, account): (HubName, MachineName, AccountName),
    kept: crate::setup::KeptNow,
) -> LoggedIn {
    LoggedIn { hub, name, account, kept: Some(kept), unkept: None }
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
        .login(addresses, account(account_name)?, password, register, crate::new_machine_name(name)?)
        .await
        .map_err(|error| error.to_string());
    #[cfg(target_os = "android")]
    crate::android::joining_ended();
    logged_in(&shell, joined?).await
}

/// What a token lets this computer into, before it is used: whose, which
/// network, whether as a managed machine (`world-network.md`, 受管机器).
#[fastctx_gui_command::command]
pub(crate) async fn peek_token(
    shell: tauri::State<'_, Shell>,
    token: String,
) -> Result<Option<fastctx_types::accounts::TokenPeek>, String> {
    shell.world().peek_token(token.trim().to_string()).await.map_err(|error| error.to_string())
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
    let joined = world
        .login_with_token(token.trim().to_string(), crate::new_machine_name(name)?)
        .await
        .map_err(|error| error.to_string());
    #[cfg(target_os = "android")]
    crate::android::joining_ended();
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
        .open_account(addresses, account(account_name)?, password, register, crate::new_machine_name(name)?)
        .await
        .map_err(|error| error.to_string())?;
    logged_in(&shell, joined).await
}

/// Every hub this computer or device is on, with what it sees of each.
#[fastctx_gui_command::command]
pub(crate) async fn accounts(shell: tauri::State<'_, Shell>) -> Result<AccountsReply, String> {
    shell.world().accounts().await.map_err(|error| error.to_string())
}

/// The networks said dissolved were seen: they are said no more.
#[fastctx_gui_command::command]
pub(crate) async fn seen_dissolved(shell: tauri::State<'_, Shell>) -> Result<(), String> {
    shell.world().seen_dissolved().await.map_err(|error| error.to_string())
}

/// Machine changes visible in the selected account, newest first.
#[fastctx_gui_command::command]
pub(crate) async fn machine_log(
    shell: tauri::State<'_, Shell>,
    hub_name: String,
    account_name: String,
    before: Option<u64>,
) -> Result<fastctx_types::machine_log::Page, String> {
    let hub = HubName::try_from(hub_name).map_err(|error| error.to_string())?;
    shell.world().machine_log(hub, account(account_name)?, before).await.map_err(|error| error.to_string())
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
    let logout = matches!(change, AccountChange::Logout);
    let made = shell
        .world()
        .account(hub(hub_name)?, account(account_name)?, change)
        .await
        .map_err(|error| error.to_string())?;
    // What this device kept of the conversations it showed was shown to the
    // account it leaves as well: none of it is left for whoever logs in next.
    if logout && let Some(cache) = shell.world().cache().cloned() {
        tauri::async_runtime::spawn_blocking(move || cache.clear())
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("logged out; letting go of what this device kept: {error}"))?;
    }
    Ok(made)
}

/// Renames this computer `name`: on no hub here, on hubs through the one it
/// is registered on, which the others follow.
#[fastctx_gui_command::command]
pub(crate) async fn rename_here(shell: tauri::State<'_, Shell>, name: String) -> Result<(), String> {
    let to = name.parse::<fastctx_types::MachineName>().map_err(|error| error.to_string())?;
    shell.world().rename_here(to).await.map_err(|error| error.to_string())
}

/// The HTTP proxy this computer reaches its hubs through: read, or with
/// `change`, set to `url`, none clearing it.
#[fastctx_gui_command::command]
pub(crate) async fn hub_proxy(
    shell: tauri::State<'_, Shell>,
    url: Option<String>,
    change: bool,
) -> Result<Option<String>, String> {
    shell.world().hub_proxy(url, change).await.map_err(|error| error.to_string())
}

/// The line that installs fastctx on a Linux server, or on a Windows computer
/// when `windows`, and registers it as a machine of `account`, valid until
/// `expires`: for `uses` of them, one when none, any number at zero, its code
/// called `label`.
#[fastctx_gui_command::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn server_command(
    shell: tauri::State<'_, Shell>,
    hub_name: Option<String>,
    account_name: String,
    name: Option<String>,
    expires: jiff::Timestamp,
    managed: Option<bool>,
    uses: Option<u32>,
    label: Option<String>,
    windows: Option<bool>,
    mac: Option<bool>,
) -> Result<String, String> {
    shell
        .world()
        .server_command(
            hub(hub_name)?,
            account(account_name)?,
            crate::new_machine_name(name)?,
            expires,
            managed.unwrap_or(false),
            uses,
            label,
            windows.unwrap_or(false),
            mac.unwrap_or(false),
        )
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
