//! Portable login identity for an already owned MCP server.

use super::*;

pub(crate) fn forward_home(config: File<'_>, receipt: &Receipt) -> Result<Option<(Vec<u8>, Receipt)>, Error> {
    receipt.check(config.path)?;
    let Some(server) = &receipt.server else { return Ok(None) };
    let Some(bytes) = config.bytes else { return Ok(None) };
    let source = Source::parse(bytes, config.path)?;
    if source.item(SERVER).is_none() {
        return Ok(None);
    }
    check_server(&source, server)?;
    let recorded = server_document(server);
    let recorded = std::str::from_utf8(&recorded).map_err(|_| Error::at("receipt", "server is not UTF-8"))?;
    let mut document: toml_edit::DocumentMut = recorded.parse().map_err(|_| Error::at("receipt", "invalid server"))?;
    let current = source.item(SERVER).expect("checked server");
    let mut next = receipt.clone();
    let target = document["mcp_servers"]["fastctx"].as_table_like_mut().expect("checked server table");
    if shared::portable(&source) {
        target.insert("command", toml_edit::value(shared::COMMAND));
        next.portable = true;
    }
    target.insert("env_vars", toml_edit::value(toml_edit::Array::from_iter(["CODEX_HOME"])));
    let mut env = toml_edit::InlineTable::new();
    env.insert("FASTCTX_LOGIN_IDENTITY", "1".into());
    target.insert("env", toml_edit::value(env));
    let desired = &document["mcp_servers"]["fastctx"];
    if same_value(current, desired) {
        return Ok(None);
    }
    let config = edit::synchronize(bytes, config.path, SERVER, desired)?;
    let body = document.to_string();
    let body = if server.inline { body.strip_prefix("[mcp_servers]\n").expect("inline wrapper") } else { &body };
    next.server = Some(Server { body: body.as_bytes().to_vec(), ..server.clone() });
    check_server(&Source::parse(&config, "updated server")?, next.server.as_ref().expect("updated receipt"))?;
    Ok(Some((config, next)))
}
