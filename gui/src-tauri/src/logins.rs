//! The agents' logins of each machine, as their nodes keep them, and what
//! the user asks of them: a scan, choosing a folder or forgetting it, taking
//! a directory up, a change's preview and the change itself, a fresh read of
//! limits. Each is done by the node of the login's machine
//! (`fastctx_types::logins`). Limits are read only for a device whose
//! workbench is on, which the interface says with `limits`.

use std::path::PathBuf;

use fastctx_types::logins::{ChangePreview, FolderChange, LoginChange, MachineLogins, WorldLogins};

use crate::Shell;

fn machine(name: String) -> Result<fastctx_types::access::Written, String> {
    name.parse().map_err(|error| format!("{error}"))
}

fn agent(name: String) -> Result<fastctx_types::AgentName, String> {
    fastctx_types::AgentName::try_from(name).map_err(|error| error.to_string())
}

/// What one machine, or every machine the user may look at, keeps of its
/// agents' logins; nothing is started to read it.
#[fastctx_gui_command::command]
pub(crate) async fn agent_logins(
    shell: tauri::State<'_, Shell>,
    machine_name: Option<String>,
) -> Result<WorldLogins, String> {
    let machine = machine_name.map(machine).transpose()?;
    shell.world().agent_logins(machine).await.map_err(|error| error.to_string())
}

/// Scans a machine for its agents, answered once the usual places are done
/// while its chosen folders are searched again; with `stop`, stops those
/// searches.
#[fastctx_gui_command::command]
pub(crate) async fn scan_agents(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    stop: bool,
) -> Result<MachineLogins, String> {
    shell.world().scan_agents(machine(machine_name)?, stop).await.map_err(|error| error.to_string())
}

/// Takes up a directory on a machine as a candidate login.
#[fastctx_gui_command::command]
pub(crate) async fn add_agent_login(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    agent_name: String,
    dir: PathBuf,
) -> Result<MachineLogins, String> {
    shell
        .world()
        .add_agent_login(machine(machine_name)?, agent(agent_name)?, dir)
        .await
        .map_err(|error| error.to_string())
}

/// Chooses a folder on a machine for logins, stops its search, or no longer
/// searches it.
#[fastctx_gui_command::command]
pub(crate) async fn agent_folder(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    dir: PathBuf,
    change: FolderChange,
) -> Result<MachineLogins, String> {
    shell.world().agent_folder(machine(machine_name)?, dir, change).await.map_err(|error| error.to_string())
}

/// What putting the World's tools into a login, or taking them out, would
/// change; with `take_back`, rewriting what the user changed of them too.
#[fastctx_gui_command::command]
pub(crate) async fn preview_agent_change(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    agent_name: String,
    dir: PathBuf,
    into: bool,
    take_back: Option<bool>,
    workbench: Option<bool>,
) -> Result<ChangePreview, String> {
    shell
        .world()
        .preview_agent_change(
            machine(machine_name)?,
            agent(agent_name)?,
            dir,
            into,
            take_back.unwrap_or(false),
            workbench,
        )
        .await
        .map_err(|error| error.to_string())
}

/// Puts the World's tools into a login, or takes them out, by its machine's
/// node; answered when the change has ended. With `expected`, the change is
/// made only while its files are as they were when it was shown.
#[fastctx_gui_command::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn change_agent_login(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    agent_name: String,
    dir: PathBuf,
    into: bool,
    raise_output_limit: bool,
    take_back: Option<bool>,
    expected: Option<String>,
    workbench: Option<bool>,
) -> Result<LoginChange, String> {
    shell
        .world()
        .change_agent_login(
            machine(machine_name)?,
            agent(agent_name)?,
            dir,
            into,
            raise_output_limit,
            take_back.unwrap_or(false),
            expected,
            workbench,
        )
        .await
        .map_err(|error| error.to_string())
}

/// What a login on a machine was used for.
#[fastctx_gui_command::command]
pub(crate) async fn agent_usage(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    agent_name: String,
    dir: PathBuf,
) -> Result<fastctx_types::usage::UsageReport, String> {
    shell.world().agent_usage(machine(machine_name)?, agent(agent_name)?, dir).await.map_err(|error| error.to_string())
}

/// Request records as this caller may see them.
#[fastctx_gui_command::command]
pub(crate) async fn agent_calls(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    agent_name: String,
    dir: PathBuf,
    query: fastctx_types::usage_calls::UsageCallsQuery,
) -> Result<fastctx_types::usage_calls::UsageCallsPage, String> {
    shell.world().agent_calls(machine(machine_name)?, agent(agent_name)?, dir, query).await.map_err(|e| e.to_string())
}

/// Reads a login's limits afresh on its machine.
#[fastctx_gui_command::command]
pub(crate) async fn read_agent_limits(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    agent_name: String,
    dir: PathBuf,
) -> Result<MachineLogins, String> {
    shell
        .world()
        .read_agent_limits(machine(machine_name)?, agent(agent_name)?, dir)
        .await
        .map_err(|error| error.to_string())
}

/// Turns on or off whether a week cleared before its reset counts anew in a
/// login.
#[fastctx_gui_command::command]
pub(crate) async fn set_agent_reset_in_place(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    agent_name: String,
    dir: PathBuf,
    on: bool,
) -> Result<MachineLogins, String> {
    shell
        .world()
        .set_agent_reset_in_place(machine(machine_name)?, agent(agent_name)?, dir, on)
        .await
        .map_err(|error| error.to_string())
}

/// Sets what a login's week is expected to hold, in quota dollars
/// (millionths), or the default again; and, where given, how far a week's
/// figure may stand from it, in percent either way.
#[fastctx_gui_command::command]
pub(crate) async fn set_agent_week(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    agent_name: String,
    dir: PathBuf,
    week: Option<u64>,
    swing: Option<u8>,
) -> Result<MachineLogins, String> {
    shell
        .world()
        .set_agent_week(machine(machine_name)?, agent(agent_name)?, dir, week.map(fastctx_types::usage::Usd), swing)
        .await
        .map_err(|error| error.to_string())
}

/// Resets what everyone has used of this week of a login to `to_tenths` of
/// the week's figure, in tenths of a percent; or with none takes the reset
/// `id` back.
#[fastctx_gui_command::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn reset_agent_usage(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    agent_name: String,
    dir: PathBuf,
    id: String,
    to_tenths: Option<u32>,
    week_end: Option<String>,
    holder: Option<String>,
) -> Result<MachineLogins, String> {
    let week_end = week_end.map(|at| at.parse::<jiff::Timestamp>()).transpose().map_err(|error| error.to_string())?;
    shell
        .world()
        .reset_agent_usage(machine(machine_name)?, agent(agent_name)?, dir, id, to_tenths, week_end, holder)
        .await
        .map_err(|error| error.to_string())
}

/// Changes the models a Claude Code login offers, for everyone who uses it.
#[fastctx_gui_command::command]
pub(crate) async fn change_agent_models(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    agent_name: String,
    dir: PathBuf,
    change: fastctx_types::logins::models::ModelChange,
) -> Result<MachineLogins, String> {
    shell
        .world()
        .change_agent_models(machine(machine_name)?, agent(agent_name)?, dir, change)
        .await
        .map_err(|error| error.to_string())
}

/// Sets compacting conversations idle a while in a login, for the account
/// asking alone.
#[fastctx_gui_command::command]
pub(crate) async fn set_idle_compaction(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    agent_name: String,
    dir: PathBuf,
    setting: fastctx_types::logins::idle::IdleCompaction,
) -> Result<MachineLogins, String> {
    shell
        .world()
        .set_idle_compaction(machine(machine_name)?, agent(agent_name)?, dir, setting)
        .await
        .map_err(|error| error.to_string())
}

/// Turns naming new Codex conversations in a login on or off.
#[fastctx_gui_command::command]
pub(crate) async fn set_agent_title(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    agent_name: String,
    dir: PathBuf,
    on: bool,
) -> Result<MachineLogins, String> {
    shell
        .world()
        .set_agent_title(machine(machine_name)?, agent(agent_name)?, dir, on)
        .await
        .map_err(|error| error.to_string())
}
