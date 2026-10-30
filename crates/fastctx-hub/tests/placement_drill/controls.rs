//! Native fork and rewind retain account placement and do not rewind files.
use super::*;
use fastctx_types::{
    ResidentId,
    residents::Landed,
    session::control::{Control, Controlled},
};
pub(super) async fn exercise(
    world: &World,
    host: &Path,
    working: &Path,
    parent: &ResidentId,
    parent_plan: &Path,
) -> Result<()> {
    let (_, result) =
        world.control(parent.clone(), Some("host".parse()?), Control::Fork { through: None, side: false }).await?;
    let Controlled::Forked { resident: child } = result else {
        anyhow::bail!("the native host did not fork the thread")
    };
    let events = shown(world, &child.resident).await?;
    let words: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.happened {
            fastctx_types::session::Happened::Heard { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    record(
        "a fork displays original user words without injected context",
        words.contains(&"ALICE_READ_AFTER_UI") && !words.iter().any(|text| text.contains("=== plan ===")),
        json!(words),
    );
    record(
        "a fork restores attachment cards when context precedes the envelope",
        events.iter().any(|event| {
            matches!(&event.happened,
            fastctx_types::session::Happened::Heard { text, message: Some(message), .. }
                if text.starts_with("ALICE_ATTACHMENT") && message.attachments.len() == 1)
        }),
        json!(null),
    );
    let said = world.talk(child.resident.clone(), Some("host".parse()?), "ALICE_FORK_CASE".into()).await?;
    anyhow::ensure!(idle(host, child.resident.as_str()).await, "the fork did not settle");
    let child_plan = plan_file(host, child.resident.as_str())?;
    record(
        "a fork keeps the account's working computer and gets its own plan path",
        child_plan.starts_with(working) && child_plan != parent_plan,
        json!(child_plan),
    );
    std::fs::write(&child_plan, "FORK_FILES_STAY")?;
    anyhow::ensure!(plan_is(world, &child.resident, "FORK_FILES_STAY").await, "the fork's file did not sync");
    let turn = match said.landed {
        Landed::NewTurn { turn } | Landed::JoinedTurn { turn } => turn,
        other => anyhow::bail!("the fork's input was not accepted: {other:?}"),
    };
    let (_, result) =
        world.control(child.resident.clone(), Some("host".parse()?), Control::Rewind { from: turn }).await?;
    record(
        "native rewind leaves the account's working files unchanged",
        matches!(result, Controlled::Done { .. } | Controlled::Rewound { .. })
            && std::fs::read_to_string(&child_plan)? == "FORK_FILES_STAY",
        json!(null),
    );
    record(
        "fork and rewind do not overwrite the parent's plan",
        std::fs::read_to_string(parent_plan)? == "# Plan\nALICE_UI_CANARY\nOFFLINE_EDIT\n",
        json!(null),
    );
    Ok(())
}

pub(super) async fn shown(world: &World, id: &ResidentId) -> Result<Vec<fastctx_types::session::SessionEvent>> {
    let mut watch = world.watch(id.clone(), Some("host".parse()?)).await?;
    tokio::time::timeout(Duration::from_secs(40), async {
        while let Some(event) = watch.next().await? {
            if let fastctx_types::session::Watched::History { events } = event {
                return Ok(events);
            }
        }
        anyhow::bail!("the history stream ended before its snapshot")
    })
    .await?
}
