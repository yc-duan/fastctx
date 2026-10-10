//! Local maintenance facts stay visible even when business IPC cannot answer.

use fastctx_types::renewal::{PathCommand, Status};

use crate::app::Ctx;
use crate::design::Role;
use crate::paint::Row;

pub fn rows(ctx: &Ctx, status: &Result<Status, String>, width: usize) -> Vec<Row> {
    let notes: Vec<String> = match status {
        Err(error) => vec![error.clone()],
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
    /// One PATH entry that is not this machine's World program: which
    /// terminal finds it, its version and where it is; and for the old
    /// release, that its unapply deletes the whole .fastctx.
    pub fn program_path(&self, command: &PathCommand) -> Vec<String> {
        use crate::words::Lang;
        let version = command.build.as_ref().map(|build| build.version.as_str()).unwrap_or("?");
        let place = command.executable.as_deref().unwrap_or(&command.path);
        let lookup = match (self.lang, &command.lookup) {
            (Lang::Zh, fastctx_types::renewal::PathLookup::Windows) => "命令提示符",
            (Lang::En, fastctx_types::renewal::PathLookup::Windows) => "Command Prompt",
            (_, fastctx_types::renewal::PathLookup::Bash) => "Git Bash",
            (Lang::Zh, fastctx_types::renewal::PathLookup::Unix) => "终端",
            (Lang::En, fastctx_types::renewal::PathLookup::Unix) => "Terminal",
            (_, fastctx_types::renewal::PathLookup::Codex) => "Codex",
        };
        if command.is_world.is_none() {
            let why = command.error.as_deref().unwrap_or("?");
            return vec![match self.lang {
                Lang::Zh => format!("无法确认 {lookup} 使用的 fastctx：{}（{why}）", command.path),
                Lang::En => format!("Can't tell which fastctx {lookup} uses: {} ({why})", command.path),
            }];
        }
        let mut notes = vec![match self.lang {
            Lang::Zh => format!("{lookup} 使用的是另一份 fastctx（{version}）：{place}"),
            Lang::En => format!("{lookup} uses another fastctx ({version}): {place}"),
        }];
        if command.legacy_unapply {
            notes.push(match self.lang {
                Lang::Zh => "请勿运行旧版的 fastctx unapply：它会删除整个 .fastctx，包括世界的数据".into(),
                Lang::En => "Don't run the old fastctx unapply: it deletes the whole .fastctx, World data included".into(),
            });
        }
        notes
    }
}
