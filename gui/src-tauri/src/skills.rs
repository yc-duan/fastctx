//! The account's skills, as the settings manage them (`fastctx_client::skills`).

use fastctx_types::skills::{Listed, Skill};

use crate::Shell;
use crate::sentinels::names;

/// The names and sentences of the account's skills.
#[fastctx_gui_command::command]
pub(crate) async fn skills_list(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
) -> Result<Vec<Listed>, String> {
    let (hub, account) = names(hub, account)?;
    shell.world().skills(hub, account).await.map_err(|error| error.to_string())
}

/// One skill whole, or none by that name.
#[fastctx_gui_command::command]
pub(crate) async fn skill_read(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    name: String,
) -> Result<Option<Skill>, String> {
    let (hub, account) = names(hub, account)?;
    shell.world().skill(hub, account, name).await.map_err(|error| error.to_string())
}

/// Keeps `skill`, in place of the one named `previous` where there was one.
#[fastctx_gui_command::command]
pub(crate) async fn skill_save(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    previous: Option<String>,
    skill: Skill,
) -> Result<(), String> {
    let (hub, account) = names(hub, account)?;
    shell.world().skill_save(hub, account, previous, skill).await.map_err(|error| error.to_string())
}

/// Lets go of the skill `name`.
#[fastctx_gui_command::command]
pub(crate) async fn skill_delete(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    name: String,
) -> Result<(), String> {
    let (hub, account) = names(hub, account)?;
    shell.world().skill_delete(hub, account, name).await.map_err(|error| error.to_string())
}
