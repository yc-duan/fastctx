//! Built-in command policies shared with the mod, before a session reports its catalog.

use fastctx_types::session::control::{CommandArguments, CommandOffer};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Catalog {
    policies: BTreeMap<String, Policy>,
    prompts: BTreeMap<String, Option<String>>,
}

#[derive(Deserialize)]
struct Policy {
    arguments: CommandArguments,
    args: Option<String>,
}

/// Known background commands for an older conversation without a saved host catalog.
/// Execution still asks the resumed host to validate the command and its arguments.
pub fn commands() -> Vec<CommandOffer> {
    let catalog: Catalog = serde_json::from_str(include_str!("command_catalog.json")).expect("built-in catalog");
    let mut policies = catalog.policies;
    for (name, args) in catalog.prompts {
        let arguments = if name == "batch" { CommandArguments::Required } else { CommandArguments::Optional };
        policies.entry(name).or_insert(Policy { arguments, args });
    }
    policies
        .into_iter()
        .map(|(name, policy)| CommandOffer {
            description: format!("Claude Code /{name}"),
            name,
            argument_forms: policy.args.iter().cloned().collect(),
            args: policy.args,
            arguments: policy.arguments,
            source: Some("builtin".into()),
            path: None,
        })
        .collect()
}
