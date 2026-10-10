//! Local maintenance facts stay visible even when business IPC cannot answer.

use fastctx_types::renewal::{PathCommand, Status};

use crate::app::Ctx;
use crate::design::Role;
use crate::paint::Row;

pub fn rows(ctx: &Ctx, status: &Result<Status, String>, width: usize) -> Vec<Row> {
    let notes: Vec<String> = match status {
        Err(error) => vec![format!("[program status] {error}")],
        Ok(status) => status
            .commands
            .iter()
            .filter(|command| command.is_world != Some(true))
            .flat_map(|command| ctx.words.program_path(command))
            .collect(),
    };
    notes
        .into_iter()
        .flat_map(|note| crate::text::wrap_plain(&note, ctx.look.text(Role::Caution), width))
        .map(Row::new)
        .collect()
}

impl crate::words::Words {
    pub fn program_path(&self, command: &PathCommand) -> Vec<String> {
        let version = command.build.as_ref().map(|build| build.version.as_str()).unwrap_or("?");
        let program = command.executable.as_deref().or(command.error.as_deref()).unwrap_or("?");
        let lookup = match command.lookup {
            fastctx_types::renewal::PathLookup::Windows => "Windows",
            fastctx_types::renewal::PathLookup::Bash => "Bash",
            fastctx_types::renewal::PathLookup::Unix => "Unix",
            fastctx_types::renewal::PathLookup::Codex => "Codex (command = fastctx)",
        };
        if command.is_world.is_none() {
            return vec![match self.lang {
                crate::words::Lang::Zh => format!(
                    "[文案占位] {lookup} 首先找到 {}；不能确定它实际启动哪一份内核：{}",
                    command.path,
                    command.error.as_deref().unwrap_or("?")
                ),
                crate::words::Lang::En => format!(
                    "[Copy placeholder] {lookup} finds {}; its actual kernel could not be determined: {}",
                    command.path,
                    command.error.as_deref().unwrap_or("?")
                ),
            }];
        }
        let mut notes = vec![match self.lang {
            crate::words::Lang::Zh => format!(
                "[文案占位] {lookup} 在当前继承的 PATH 中首先启动 {}，版本 {version}，实际程序 {program}；入口不是世界的稳定副本。终端与 Codex 的共享 command = fastctx 各按自己的查找规则启动它；新版入口会转交稳定副本，旧版不会",
                command.path
            ),
            crate::words::Lang::En => format!(
                "[Copy placeholder] {lookup} first starts {} in the currently inherited PATH, version {version}, executable {program}; this entry is not the stable World program. Terminals and Codex's shared command = fastctx use their respective lookups; new deliveries forward to the stable program, legacy versions do not",
                command.path
            ),
        }];
        if command.legacy_unapply {
            notes.push(match self.lang {
                crate::words::Lang::Zh => "[文案占位] 这份旧程序的 unapply 会删除整个 .fastctx；不要用它撤销新版世界的接入".into(),
                crate::words::Lang::En => "[Copy placeholder] This legacy program's unapply deletes the entire .fastctx; do not use it to withdraw the new World integration".into(),
            });
        }
        notes
    }
}
