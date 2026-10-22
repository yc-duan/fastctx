//! A project's pages, as the page screen asks for them
//! (`gui/src/world/pages.ts`; their list comes with the project's map):
//! reading a version, its history, the user's consent, an act an open page
//! asks for, taking a page down and bringing a version back. The core
//! decides every one of them; a device reaches it through a machine's node.

use fastctx_types::pages::{PageAct, PageConsent, PageInvocation, PageResult, PageVersion, PageView};
use fastctx_types::{PageName, TownName};

use crate::Shell;

fn town(name: String) -> Result<Option<TownName>, String> {
    TownName::try_from(name).map(Some).map_err(|error| error.to_string())
}

fn page(name: String) -> Result<PageName, String> {
    PageName::try_from(name).map_err(|error| error.to_string())
}

#[fastctx_gui_command::command]
pub async fn page_read(
    shell: tauri::State<'_, Shell>,
    town: String,
    page: String,
    version: Option<u32>,
    account: Option<String>,
) -> Result<PageView, String> {
    let account = account.map(fastctx_types::AccountName::try_from).transpose().map_err(|error| error.to_string())?;
    shell
        .world()
        .page_as(self::town(town)?, self::page(page)?, version, account)
        .await
        .map_err(|error| error.to_string())
}

#[fastctx_gui_command::command]
pub async fn page_history(
    shell: tauri::State<'_, Shell>,
    town: String,
    page: String,
) -> Result<Vec<PageVersion>, String> {
    shell.world().page_history(self::town(town)?, self::page(page)?).await.map_err(|error| error.to_string())
}

#[fastctx_gui_command::command]
pub async fn page_consent(
    shell: tauri::State<'_, Shell>,
    town: String,
    page: String,
    version: u32,
    base: Option<u32>,
    acts: Vec<PageAct>,
    account: Option<String>,
) -> Result<PageConsent, String> {
    let account = account.map(fastctx_types::AccountName::try_from).transpose().map_err(|error| error.to_string())?;
    shell
        .world()
        .consent_page_as(self::town(town)?, self::page(page)?, version, base, acts, account)
        .await
        .map_err(|error| error.to_string())
}

#[fastctx_gui_command::command]
pub async fn page_act(
    shell: tauri::State<'_, Shell>,
    town: String,
    page: String,
    version: u32,
    action: PageInvocation,
    account: Option<String>,
) -> Result<PageResult, String> {
    let account = account.map(fastctx_types::AccountName::try_from).transpose().map_err(|error| error.to_string())?;
    shell
        .world()
        .act_page_as(self::town(town)?, self::page(page)?, version, action, account)
        .await
        .map_err(|error| error.to_string())
}

#[fastctx_gui_command::command]
pub async fn page_withdraw(
    shell: tauri::State<'_, Shell>,
    town: String,
    page: String,
    base: u32,
) -> Result<PageView, String> {
    shell.world().withdraw_page(self::town(town)?, self::page(page)?, base).await.map_err(|error| error.to_string())
}

#[fastctx_gui_command::command]
pub async fn page_restore(
    shell: tauri::State<'_, Shell>,
    town: String,
    page: String,
    base: u32,
    version: u32,
) -> Result<PageView, String> {
    shell
        .world()
        .restore_page(self::town(town)?, self::page(page)?, base, version)
        .await
        .map_err(|error| error.to_string())
}
