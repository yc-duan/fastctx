//! The one portable spelling accepted in a shared Codex configuration.

use super::*;

mod withdrawal;

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

pub(super) fn withdrawal(before: &[u8], after: &[u8], location: &str) -> Result<Option<String>, Error> {
    withdrawal::describe(before, after, location)
}
