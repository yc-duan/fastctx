//! The agents' logins of each machine, as their nodes keep them, and what
//! the user asks of them: a scan, taking a directory up, a change's preview
//! and the change itself, a fresh read of limits. Each is done by the node
//! of the login's machine (`fastctx_types::logins`).

use std::path::PathBuf;

use fastctx_types::logins::{ChangePreview, LoginChange, MachineLogins, WorldLogins};

use crate::Shell;

fn machine(name: String) -> Result<fastctx_types::MachineName, String> {
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

/// Scans a machine for its agents, answered once the usual places are done;
/// with `stop`, stops the walk of its disks.
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

/// What putting the World's tools into a login, or taking them out, would
/// change.
#[fastctx_gui_command::command]
pub(crate) async fn preview_agent_change(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    agent_name: String,
    dir: PathBuf,
    into: bool,
) -> Result<ChangePreview, String> {
    shell
        .world()
        .preview_agent_change(machine(machine_name)?, agent(agent_name)?, dir, into)
        .await
        .map_err(|error| error.to_string())
}

/// Puts the World's tools into a login, or takes them out, by its machine's
/// node; answered when the change has ended.
#[fastctx_gui_command::command]
pub(crate) async fn change_agent_login(
    shell: tauri::State<'_, Shell>,
    machine_name: String,
    agent_name: String,
    dir: PathBuf,
    into: bool,
    raise_output_limit: bool,
) -> Result<LoginChange, String> {
    shell
        .world()
        .change_agent_login(machine(machine_name)?, agent(agent_name)?, dir, into, raise_output_limit)
        .await
        .map_err(|error| error.to_string())
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
