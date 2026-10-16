//! Uninstalling as a hub that took this machine off asked
//! (`world-network.md`, 移出网络与卸载): the same removal the machine's own
//! settings make, started by its node and outliving it, which tells that
//! hub what it could not do.

use std::path::Path;

use fastctx_types::uninstall::{Event, Left, Result as Outcome};

use crate::Request;

/// Runs `request` outside the installation, as `run_outside` does; when
/// anything is left undone, or the removal cannot start here, tells the
/// hub of the membership kept in `membership` what and why, and fails
/// saying so.
pub fn after_removal(request: &Request, membership: &Path) -> anyhow::Result<()> {
    let left = match refusal(request)? {
        Some(why) => Some(why),
        None => {
            let mut finished: Vec<Left> = Vec::new();
            let mut failed: Option<String> = None;
            let outcome =
                crate::helper::run_outside_reading(request, &mut |line| match serde_json::from_str::<Event>(line) {
                    Ok(Event::Finished { left }) => finished = left,
                    Ok(Event::Step { step, result: Outcome::Failed, why }) => {
                        failed = Some(match why {
                            Some(why) => format!("{step}: {why}"),
                            None => step,
                        });
                    }
                    _ => {}
                });
            match outcome {
                Ok(()) if finished.is_empty() => None,
                Ok(()) => Some(described(&finished)),
                Err(error) => Some(failed.unwrap_or_else(|| error.to_string())),
            }
        }
    };
    let Some(left) = left else { return Ok(()) };
    let world = fastctx_types::layout::WorldDir::under(&request.installation.root);
    let told = tokio::runtime::Runtime::new()?.block_on(crate::logout::tell_left(&world, membership, &left));
    match told {
        Ok(()) => anyhow::bail!("{left}"),
        Err(error) => anyhow::bail!("{left}; telling the hub: {error}"),
    }
}

/// Why the removal does not start here: the machine keeps a hub, or a step
/// needs an administrator this process is not.
fn refusal(request: &Request) -> anyhow::Result<Option<String>> {
    if let Some(hub) = &request.plan.hub {
        return Ok(Some(format!(
            "this machine runs the hub {}: move it elsewhere or dissolve its network, then uninstall here",
            hub.name
        )));
    }
    let administered: Vec<&str> =
        request.plan.steps.iter().filter(|step| step.admin).map(|step| step.what.as_str()).collect();
    if !administered.is_empty() && !fastctx_platform::elevation::is_administrator()? {
        let everything = if request.plan.everything { " --everything" } else { "" };
        return Ok(Some(format!(
            "{} needs an administrator: run `fastctx uninstall run{everything}` on this machine as one",
            administered.join(", ")
        )));
    }
    Ok(None)
}

fn described(left: &[Left]) -> String {
    left.iter()
        .map(|item| match &item.location {
            Some(location) => format!("{} ({location}): {}", item.what, item.why),
            None => format!("{}: {}", item.what, item.why),
        })
        .collect::<Vec<_>>()
        .join("; ")
}
