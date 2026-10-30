//! Ordered markers prove the absence of unwanted hints without timeout races.
use crate::{report::Report, world::World};
use fastctx_types::{
    Address,
    account_records::{Request, Response, Watched},
};
use std::time::Duration;

pub async fn exercise(world: &World, report: &mut Report) -> anyhow::Result<()> {
    let writer = world.machine("alpha").link();
    let reader = world.machine("gamma").link();
    let owner = Address { machine: world.machine("alpha").name.clone(), resident: "codex-f011".parse()? };
    let mut changes = reader.record_changes();
    let mut base = None;
    for (step, expected) in [(0, false), (1, true), (2, false)] {
        if step == 1 {
            reader.plan_read_of(&world.account, &owner).await.map_err(anyhow::Error::msg)?;
        }
        if step == 2 {
            reader
                .follow_record(&world.account, "plan", owner.resident.as_str(), false)
                .await
                .map_err(anyhow::Error::msg)?;
        }
        let reply = writer
            .plan_copy(
                &world.account,
                fastctx_types::plan_copies::Request::Merge {
                    owner: owner.clone(),
                    base,
                    markdown: format!("step {step}"),
                    submission: None,
                },
            )
            .await
            .map_err(anyhow::Error::msg)?;
        let fastctx_types::plan_copies::Reply::Page { page } = reply else { anyhow::bail!("no page") };
        base = page.version();
        let marker = format!("marker-{step}");
        writer
            .account_records(
                &world.account,
                Request::Write {
                    kind: "placement-probe".into(),
                    key: marker.clone(),
                    expected: None,
                    body: serde_json::json!(step),
                },
            )
            .await
            .map_err(anyhow::Error::msg)?;
        let observed = tokio::time::timeout(Duration::from_secs(20), async {
            let mut observed = false;
            loop {
                if let Some(change) = changes.recv().await? {
                    if change.metadata.kind == "placement-probe" && change.metadata.key == marker {
                        return Ok::<_, anyhow::Error>(observed);
                    }
                    if change.metadata.kind == "plan" && change.metadata.key == owner.resident.as_str() {
                        observed = true;
                    }
                }
            }
        })
        .await??;
        report.check(
            observed == expected,
            [
                "an unread plan sends no copy hints",
                "reading a plan subscribes its copy",
                "withdrawing a copy stops its plan hints",
            ][step],
            "",
        );
    }
    let mut watch = reader.watch_account_records(world.account.clone(), "plan".into()).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        matches!(watch.next().await.map_err(anyhow::Error::msg)?, Watched::Current { .. }),
        "watch did not list"
    );
    let new_owner = Address { resident: "codex-f012".parse()?, ..owner.clone() };
    writer
        .plan_copy(
            &world.account,
            fastctx_types::plan_copies::Request::Merge {
                owner: new_owner.clone(),
                base: None,
                markdown: "new key".into(),
                submission: None,
            },
        )
        .await
        .map_err(anyhow::Error::msg)?;
    let changed = tokio::time::timeout(Duration::from_secs(20), watch.next()).await?.map_err(anyhow::Error::msg)?;
    report.check(
        matches!(changed, Watched::Changed { metadata } if metadata.key == new_owner.resident.as_str()),
        "an explicit kind watch receives newly created keys",
        "",
    );
    writer.plan_delete(&world.account, &owner.resident).await.map_err(anyhow::Error::msg)?;
    let tombstone = reader
        .account_records(&world.account, Request::Read { kind: "plan".into(), key: owner.resident.to_string() })
        .await
        .map_err(anyhow::Error::msg)?;
    report.check(
        matches!(tombstone, Response::Read { record: Some(record) } if record.metadata.deleted),
        "a deleted plan retains its fence",
        "",
    );
    Ok(())
}
