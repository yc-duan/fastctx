//! Co-owners manage a conversation's goals and sentinels without its account settings.
use super::*;
use fastctx_types::{
    Address,
    goals::{Request as GoalRequest, Updated, UserChange},
    sentinel_control::{Create, Reply as SentinelReply, Request as SentinelRequest},
    sentinel_settings::{Settings, SettingsReply, WriteSettings},
};

async fn change(world: &World, address: &Address, change: UserChange, prefix: &str) -> Result<()> {
    for attempt in 0..60 {
        let Updated::Goal { view } =
            world.goal(address.resident.clone(), Some(address.machine.clone().into()), None, GoalRequest::Read).await?
        else {
            anyhow::bail!("goal read did not return its view")
        };
        let reply = world
            .goal(
                address.resident.clone(),
                Some(address.machine.clone().into()),
                None,
                GoalRequest::Change {
                    operation: format!("{prefix}-{attempt}"),
                    expected: view.version,
                    change: change.clone(),
                },
            )
            .await?;
        match reply {
            Updated::Goal { .. } => return Ok(()),
            Updated::Conflict { .. } => pause().await,
            other => anyhow::bail!("unexpected goal change: {other:?}"),
        }
    }
    anyhow::bail!("the shared goal kept changing")
}

pub(super) async fn exercise(owner: &World, bob: &World, local: &World, address: &Address) -> Result<()> {
    change(bob, address, UserChange::Open { objective: "SHARED_GOAL_BOB".into(), standing: false }, "shared-goal-open")
        .await?;
    let shared =
        owner.goal(address.resident.clone(), Some(address.machine.clone().into()), None, GoalRequest::Read).await?;
    record(
        "a co-owner reads the goal from its actual account",
        matches!(shared,Updated::Goal { view } if view.account.account == name("bob") && view.goal.as_ref().is_some_and(|goal| goal.objective=="SHARED_GOAL_BOB")),
        json!(null),
    );
    change(owner, address, UserChange::Edit { objective: "SHARED_GOAL_OWNER_EDIT".into() }, "shared-goal-edit").await?;
    let read =
        bob.goal(address.resident.clone(), Some(address.machine.clone().into()), None, GoalRequest::Read).await?;
    record(
        "a co-owner changes the same goal without impersonating its opener",
        matches!(read,Updated::Goal { view } if view.account.account == name("bob") && view.goal.as_ref().is_some_and(|goal| goal.objective=="SHARED_GOAL_OWNER_EDIT")),
        json!(null),
    );
    change(owner, address, UserChange::Pause, "shared-goal-pause").await?;
    let SettingsReply::View { view } = bob.sentinel_settings(None, Some(name("bob")), None).await? else {
        anyhow::bail!("sentinel settings read conflicted")
    };
    bob.sentinel_settings(
        None,
        Some(name("bob")),
        Some(WriteSettings {
            expected: view.version,
            settings: Settings { home: Some(machine("bob-pc")), model: None, search: None },
        }),
    )
    .await?;
    record(
        "shared ownership grants no access to the opener's sentinel settings",
        owner.sentinel_settings(None, Some(name("bob")), None).await.is_err(),
        json!(null),
    );
    let created = local
        .sentinel(
            None,
            Some(name("bob")),
            SentinelRequest::Create {
                request: Create {
                    condition: String::new(),
                    at: vec![jiff::Timestamp::now().as_millisecond() + 3_600_000],
                    web: false,
                    tell: "SHARED_SENTINEL_CANARY".into(),
                },
            },
        )
        .await?;
    let SentinelReply::Created { sentinel } = created else { anyhow::bail!("the sentinel was not created") };
    let definition = sentinel.definition;
    let list = owner.sentinel(None, None, SentinelRequest::List { owner: Some(address.clone()) }).await?;
    record(
        "a co-owner lists the other account's conversation sentinel",
        matches!(list,SentinelReply::Listed { sentinels,unavailable } if unavailable.is_empty() && sentinels.iter().any(|sentinel| sentinel.definition.id==definition.id)),
        json!(null),
    );
    let record_read = owner
        .sentinel(
            None,
            None,
            SentinelRequest::Record {
                id: definition.id.clone(),
                home: definition.home.clone(),
                generation: definition.generation,
                held: None,
            },
        )
        .await?;
    record(
        "a co-owner reads sentinel details through the conversation's host",
        matches!(record_read,SentinelReply::Record { record } if record.summary.is_some()),
        json!(null),
    );
    let stopped = owner
        .sentinel(None, None, SentinelRequest::Stop { id: definition.id.clone(), home: Some(definition.home) })
        .await?;
    record(
        "a co-owner stops a sentinel on the opener's private computer",
        matches!(stopped, SentinelReply::Stopped { home_confirmed: true, reason: None, .. }),
        json!(null),
    );
    let list = bob.sentinel(None, None, SentinelRequest::List { owner: Some(address.clone()) }).await?;
    record(
        "the opener sees the shared sentinel stop",
        matches!(list,SentinelReply::Listed { sentinels,.. } if sentinels.iter().all(|sentinel| sentinel.definition.id!=definition.id)),
        json!(null),
    );
    Ok(())
}
