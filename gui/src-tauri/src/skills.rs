//! The account's skills, as the settings manage them (`fastctx_client::skills`).

use fastctx_types::skills::{Listed, Skill};

use crate::Shell;
use crate::sentinels::names;

/// Atomic file-folder reads and edits; writes carry the revision from the editor's read.
#[fastctx_gui_command::command]
pub(crate) async fn skill_files(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    request: fastctx_types::skill_files::Request,
) -> Result<fastctx_types::skill_files::Reply, String> {
    let (hub, account) = names(hub, account)?;
    shell.world().skill_files(hub, account, request).await.map_err(|error| error.to_string())
}

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

/// A folder picked on this computer, read as a skill's files.
#[derive(serde::Serialize)]
pub(crate) struct Picked {
    folder: String,
    files: Vec<fastctx_types::skill_files::File>,
}

/// The most files a folder taken in may hold; the hub checks its own.
const FILES_MOST: usize = 2000;

/// Picks a folder with the system's own picker and reads it, every file
/// under it by its path there; none when the user changed their mind. Links
/// are passed over, as they may lead out of the folder.
#[fastctx_gui_command::command]
pub(crate) async fn skill_folder_pick(app: tauri::AppHandle, window: tauri::WebviewWindow) -> Result<Option<Picked>, String> {
    let Some(root) = crate::folder_picker::pick(app, window).await? else { return Ok(None) };
    tauri::async_runtime::spawn_blocking(move || read_folder(&root).map(Some))
        .await
        .map_err(|error| error.to_string())?
}

fn read_folder(root: &std::path::Path) -> Result<Picked, String> {
    use base64::Engine as _;
    let folder = root.file_name().and_then(|name| name.to_str()).unwrap_or("skill").to_string();
    let mut files = Vec::new();
    let mut total = 0usize;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let mut entries = std::fs::read_dir(&directory)
            .map_err(|error| format!("{}: {error}", directory.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            let path = entry.path();
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                pending.push(path);
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let bytes = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            total += bytes.len();
            if files.len() >= FILES_MOST || total > fastctx_types::skill_files::ENCODED_LIMIT {
                return Err("the folder is larger than a skill may be".into());
            }
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt as _;
                entry.metadata().map(|metadata| metadata.permissions().mode() & 0o111 != 0).unwrap_or(false)
            };
            #[cfg(not(unix))]
            let executable = false;
            files.push(fastctx_types::skill_files::File {
                path: relative,
                data: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
                executable,
            });
        }
    }
    Ok(Picked { folder, files })
}
