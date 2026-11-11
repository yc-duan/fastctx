//! One login's display, with no external read on the list's answering path.
use super::build::{Aliases, failure, read};
use crate::{
    Node,
    logins::{self, Context, kept},
};

use fastctx_types::{
    agents::{AgentHome, Fact, Integration},
    ipc::Failure,
    logins::{Found, Login},
};
use std::{path::Path, sync::Arc, time::Instant};

#[allow(clippy::too_many_arguments)]
pub(super) async fn one(
    node: &Arc<Node>,
    context: &Arc<Context>,
    record: &Arc<kept::Kept>,
    homes: Arc<Vec<AgentHome>>,
    aliases: Option<Arc<Aliases>>,
    agent: &'static str,
    dir: &Path,
    found: Found,
    deadline: Instant,
) -> Result<Login, Failure> {
    let dir_owned = dir.to_path_buf();
    let (ctx, location, kept) = (context.clone(), dir_owned.clone(), record.clone());
    let (integration, known) = read(node, deadline, "reading the login's connection state", move || {
        let integration = logins::standing(&ctx, logins::host(agent)?, &location).map_err(Failure::new)?;
        let known = kept.of(agent).logins.iter().find(|login| crate::homes::same_dir(&login.dir, &location)).cloned();
        Ok((integration, known))
    })
    .await?;
    let (reading, kept, dir) = (node.clone(), record.clone(), dir_owned.clone());
    let limits = read(node, deadline, "reading the login's usage", move || {
        Ok(if agent == "codex" {
            logins::limits::of(&reading, &kept, &dir)
        } else {
            logins::claude_limits(&reading, &dir)
        })
    })
    .await
    .unwrap_or_else(|error| Fact::failed(error.message));
    let (reading, dir, saved) = (node.clone(), dir_owned.clone(), known.clone());
    let launcher = match read(node, deadline, "finding the login's program", move || {
        Ok(logins::program::now(&reading, agent, &dir, saved.as_ref()))
    })
    .await
    {
        Ok(launcher) => launcher,
        Err(error) => {
            failure(node, "program", &error);
            None
        }
    };
    let (models, model_edits) = if agent == "codex" {
        (known.as_ref().map(|login| login.models.clone()).unwrap_or_default(), None)
    } else {
        let (dir, kept) = (dir_owned.clone(), record.clone());
        match read(node, deadline, "assembling Claude models", move || {
            let edits = logins::models::edits(&kept, &dir);
            let models = aliases
                .map(|aliases| {
                    fastctx_hosts::claude::models::agent_models(
                        logins::models::aliases(&aliases, &homes, &dir).as_deref(),
                        &edits,
                    )
                })
                .unwrap_or_default();
            Ok((models, Some(edits)))
        })
        .await
        {
            Ok(models) => models,
            Err(error) => {
                failure(node, "Claude models", &error);
                (Vec::new(), None)
            }
        }
    };
    let unattended = if agent == "claude" && integration == Integration::In {
        let (ctx, dir) = (context.clone(), dir_owned.clone());
        match read(node, deadline, "reading Claude consent", move || {
            fastctx_install::apply::claude_consent_at(&ctx.places, &dir)
                .map_err(|error| Failure::new(error.to_string()))
        })
        .await
        {
            Ok(accepted) => Some(accepted),
            Err(error) => {
                failure(node, "Claude consent", &error);
                None
            }
        }
    } else {
        None
    };
    let (reading, dir, kept) = (node.clone(), dir_owned.clone(), record.clone());
    let (reset_in_place, workbench, drift, auto_title) = read(node, deadline, "reading login preferences", move || {
        Ok((
            crate::usage::resets_in_place(&reading, agent, &dir),
            logins::workbench::is_open(&reading, &fastctx_types::access::Acting::Here, agent, Some(&dir)),
            logins::refresh::of(&reading, agent, &dir),
            agent == "codex" && !kept.codex.untitled.iter().any(|known| crate::homes::same_dir(known, &dir)),
        ))
    })
    .await
    .unwrap_or_else(|error| {
        (
            false,
            false,
            Some(fastctx_types::logins::Drift::Changed {
                location: dir_owned.display().to_string(),
                reason: error.message,
            }),
            false,
        )
    });
    Ok(Login {
        dir: dir_owned.clone(),
        default: found == Found::Default,
        found,
        identity: known.map(|login| login.identified),
        limits,
        programs: if found == Found::Default {
            (0..record.of(agent).programs.len() as u32).collect()
        } else {
            Vec::new()
        },
        integration,
        unattended,
        auto_title,
        reset_in_place,
        launcher,
        models,
        model_edits,
        change: None,
        drift: if integration == Integration::In { drift } else { None },
        workbench: integration == Integration::In && workbench,
    })
}
