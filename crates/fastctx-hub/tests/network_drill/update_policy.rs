//! A rollout and its withdrawals survive a machine restarting without its hub.
use crate::{
    report::Report,
    world::{World, until},
};
use fastctx_net::wire::Change;
use fastctx_types::update::Rollout;
use std::{collections::BTreeSet, time::Duration};

pub async fn exercise(world: &World, report: &mut Report) -> anyhow::Result<()> {
    let alpha = world.machine("alpha");
    let version = "1.0.1-beta.99";
    let withdrawn = "1.0.1-beta.98";
    alpha
        .link()
        .manage(
            &world.account,
            Change::Rollout {
                rollout: Some(Rollout {
                    version: version.into(),
                    first: BTreeSet::new(),
                    share: 0,
                    paused: false,
                    stopped: None,
                }),
                withdrawn: BTreeSet::from([withdrawn.into()]),
            },
        )
        .await
        .map_err(anyhow::Error::msg)?;
    let file = world.root.join("alpha/net/update-policy.json");
    let durable = until(Duration::from_secs(30), || {
        std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|value| {
                value["policy"]["rollout"]["version"] == version
                    && value["policy"]["withdrawn"]
                        .as_array()
                        .is_some_and(|values| values.iter().any(|v| v == withdrawn))
            })
    })
    .await;
    anyhow::ensure!(durable.is_some(), "the rollout policy was not durably received");
    world.hub.kill()?;
    alpha.stop();
    alpha.start()?;
    let link = alpha.link();
    report.check(
        link.versions().is_none(),
        "the restarted machine has received no live version message",
        "hub stopped",
    );
    let network = link.update_network().ok_or_else(|| anyhow::anyhow!("no durable update policy"))?;
    report.check(
        network.rolled.as_ref().is_some_and(|rolled| rolled.version == version && !rolled.takes),
        "the saved zero-share rollout still refuses automatic updating",
        version,
    );
    report.check(
        network.policy.check_release(withdrawn).is_err(),
        "the saved withdrawal still refuses an explicit release",
        withdrawn,
    );
    report.check(
        network.due.is_empty() && network.machines.is_empty(),
        "offline policy does not recreate queued work or machine reports",
        "",
    );
    world.hub.restart(Duration::ZERO).await?;
    Ok(())
}
