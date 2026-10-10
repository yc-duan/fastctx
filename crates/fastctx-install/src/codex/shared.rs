//! The one portable spelling accepted in a shared Codex configuration.

use super::*;

pub(super) const COMMAND: &str = "fastctx";

pub(super) fn portable(source: &Source<'_>) -> bool {
    source.item(&["mcp_servers", "fastctx", "command"]).and_then(Item::as_str) == Some(COMMAND)
}

pub(super) fn matches(current: &Item, expected: &Item) -> bool {
    if same_value(current, expected) {
        return true;
    }
    if current.as_table_like().and_then(|table| table.get("command")).and_then(Item::as_str) != Some(COMMAND) {
        return false;
    }
    let mut expected = expected.clone();
    let Some(table) = expected.as_table_like_mut() else { return false };
    table.insert("command", toml_edit::value(COMMAND));
    same_value(current, &expected)
}

pub(super) fn adopt(source: &Source<'_>, plan: &CodexPlan) -> Result<Server, Error> {
    let server = Server { body: server_body(plan, COMMAND, false, "\n"), prefix: vec![], inline: false };
    let expected = server_document(&server);
    let expected = Source::parse(&expected, "shared server")?;
    if source.item(SERVER).is_some_and(|current| same_value(current, expected.item(SERVER).unwrap())) {
        Ok(server)
    } else {
        Err(Error::at(
            "mcp_servers.fastctx",
            "the shared server's other keys or values differ from the World's; left it in place",
        ))
    }
}

pub(super) fn conflict(reason: &str) -> String {
    format!(
        "{reason}; for a configuration shared across machines, use command = \"fastctx\" with the World's other server values; a machine-local path is not replaced"
    )
}

pub(super) fn withdrawal(location: &str) -> String {
    format!(
        "{location}: unapply removes the fastctx server from this shared configuration file; after synchronization, the other machines also lose that server"
    )
}

pub(super) fn on_path() -> bool {
    let Some(path) = std::env::var_os("PATH") else { return false };
    let mut names = vec![COMMAND.to_owned()];
    if cfg!(windows) {
        let extensions = std::env::var_os("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
        names.extend(
            extensions
                .to_string_lossy()
                .split(';')
                .filter(|extension| !extension.is_empty())
                .map(|extension| format!("{COMMAND}{extension}")),
        );
    }
    std::env::split_paths(&path).any(|directory| {
        names.iter().any(|name| {
            let file = directory.join(name);
            if !file.is_file() {
                return false;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.metadata().is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
            }
            #[cfg(not(unix))]
            {
                true
            }
        })
    })
}
