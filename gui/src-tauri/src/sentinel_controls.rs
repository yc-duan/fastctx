//! Conversation sentinel summaries, records and withdrawal, using the account-scoped node API.
use crate::Shell;
use fastctx_types::{
    Address, MachineName, SentinelId,
    sentinel_control::{Reply, Request},
};

#[fastctx_gui_command::command]
pub(crate) async fn sentinel_list(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    owner: Option<Address>,
) -> Result<Reply, String> {
    let (hub, account) = crate::sentinels::names(hub, account)?;
    match shell.world().sentinel(hub, account, Request::List { owner }).await.map_err(|e| e.to_string())? {
        reply @ Reply::Listed { .. } => Ok(reply),
        _ => Err("unexpected sentinel list reply".into()),
    }
}

#[fastctx_gui_command::command]
pub(crate) async fn sentinel_stop(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    id: SentinelId,
    home: Option<MachineName>,
) -> Result<Reply, String> {
    let (hub, account) = crate::sentinels::names(hub, account)?;
    match shell.world().sentinel(hub, account, Request::Stop { id, home }).await.map_err(|e| e.to_string())? {
        reply @ Reply::Stopped { .. } => Ok(reply),
        _ => Err("unexpected sentinel withdrawal reply".into()),
    }
}

/// Every sentinel of the accounts this device acts for, ended or not, with
/// when each woke and what each wake took.
#[fastctx_gui_command::command]
pub(crate) async fn sentinel_overview(shell: tauri::State<'_, Shell>) -> Result<Reply, String> {
    match shell.world().sentinel(None, None, Request::Overview).await.map_err(|e| e.to_string())? {
        reply @ Reply::Overview { .. } => Ok(reply),
        _ => Err("unexpected sentinel overview reply".into()),
    }
}

/// What a sentinel did, read from its home; `held` names the version the
/// window already shows.
#[fastctx_gui_command::command]
pub(crate) async fn sentinel_record(
    shell: tauri::State<'_, Shell>,
    hub: Option<String>,
    account: Option<String>,
    id: SentinelId,
    home: MachineName,
    generation: u64,
    held: Option<String>,
) -> Result<Reply, String> {
    let (hub, account) = crate::sentinels::names(hub, account)?;
    match shell
        .world()
        .sentinel(hub, account, Request::Record { id, home, generation, held })
        .await
        .map_err(|e| e.to_string())?
    {
        reply @ Reply::Record { .. } => Ok(reply),
        _ => Err("unexpected sentinel record reply".into()),
    }
}
