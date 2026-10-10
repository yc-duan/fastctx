//! Describe the shared settings actually changed, including partial withdrawal.

use super::*;

pub(super) fn describe(before: &[u8], after: &[u8], location: &str) -> Result<Option<String>, Error> {
    let before = Source::parse(before, location)?;
    let after = Source::parse(after, location)?;
    let mut changes = Vec::new();
    if before.item(SERVER).is_some() && after.item(SERVER).is_none() {
        changes.push("removed mcp_servers.fastctx".to_owned());
    }
    for path in [LIMIT, &["features", "shell_tool"], &["features", "view_image"]] {
        match (before.item(path), after.item(path)) {
            (Some(left), Some(right)) if !same_value(left, right) => {
                let value = right
                    .as_integer()
                    .map(|value| value.to_string())
                    .or_else(|| right.as_bool().map(|value| value.to_string()))
                    .ok_or_else(|| Error::at(path.join("."), "expected a restored integer or boolean"))?;
                changes.push(format!("restored {} to {value}", path.join(".")));
            }
            (Some(_), None) => changes.push(format!("removed {}", path.join("."))),
            _ => {}
        }
    }
    if changes.is_empty() {
        return Ok(None);
    }
    let retained = if after.item(SERVER).is_some() { " The fastctx server is left in place." } else { "" };
    Ok(Some(format!(
        "{location}: unapply changed this shared configuration file: {}.{retained} Synchronizing this file applies these same changes on the other machines.",
        changes.join("; "),
    )))
}
