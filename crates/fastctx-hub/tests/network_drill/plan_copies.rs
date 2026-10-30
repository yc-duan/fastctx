//! Real hub persistence and concurrent edits from distinct network members.

use crate::{report::Report, world::World};
use fastctx_types::{
    Address, ResidentId,
    plan::{Author, Page, Result as PlanResult},
    plan_copies::{Reply, Request},
};
use std::time::Duration;

fn text(page: &Page) -> &str {
    page.body.as_ref().map_or("", |body| body.markdown.as_str())
}
fn page(reply: Reply) -> anyhow::Result<Page> {
    match reply {
        Reply::Page { page } => Ok(*page),
        _ => anyhow::bail!("expected a plan page"),
    }
}

pub async fn exercise(world: &World, report: &mut Report) -> anyhow::Result<()> {
    let alpha = world.machine("alpha").link();
    let beta = world.machine("beta").link();
    let owner = Address {
        machine: world.machine("alpha").name.clone(),
        resident: ResidentId::try_from("codex-a1b2".to_string()).map_err(anyhow::Error::msg)?,
    };
    report.check(alpha.view().plan_copies, "the hub advertises revision-aware plan copies", "");
    let base = "# Work\n- [ ] first\n- [ ] second\n";
    let first = page(
        alpha
            .plan_copy(
                &world.account,
                Request::Merge { submission: None, owner: owner.clone(), base: None, markdown: base.into() },
            )
            .await
            .map_err(anyhow::Error::msg)?,
    )?;
    let user = "# Work\n- [x] first\n- [ ] second\n";
    let user_page = beta
        .plan_write(&world.account, owner.resident.clone(), first.version(), user.into(), Author::User, Some(&owner))
        .await
        .map_err(anyhow::Error::msg)?;
    let PlanResult::Page { page: user_page } = user_page else { anyhow::bail!("the user save conflicted") };
    let file = "# Work\n- [ ] first\n- [x] second\n";
    let merged = page(
        alpha
            .plan_copy(
                &world.account,
                Request::Merge {
                    submission: Some(fastctx_net::plan_merge::submission(&owner, first.version(), file)),
                    owner: owner.clone(),
                    base: first.version(),
                    markdown: file.into(),
                },
            )
            .await
            .map_err(anyhow::Error::msg)?,
    )?;
    report.check(
        text(&merged) == "# Work\n- [x] first\n- [x] second\n",
        "independent file and old-client panel edits merge by line",
        text(&merged),
    );
    let concurrent = beta
        .plan_write(
            &world.account,
            owner.resident.clone(),
            merged.version(),
            "hub conflict\n".into(),
            Author::User,
            Some(&owner),
        )
        .await
        .map_err(anyhow::Error::msg)?;
    let PlanResult::Page { page: concurrent } = concurrent else { anyhow::bail!("the conflict fixture did not save") };
    let chosen = page(
        alpha
            .plan_copy(
                &world.account,
                Request::Merge {
                    submission: None,
                    owner: owner.clone(),
                    base: merged.version(),
                    markdown: "file conflict\n".into(),
                },
            )
            .await
            .map_err(anyhow::Error::msg)?,
    )?;
    report.check(text(&chosen) == "file conflict\n", "an overlapping edit selects the file", text(&chosen));
    for (version, expected) in
        [(first.version(), base), (user_page.version(), user), (concurrent.version(), "hub conflict\n")]
    {
        let historical = page(
            beta.plan_copy(
                &world.account,
                Request::Revision { owner: owner.clone(), version: version.expect("written version") },
            )
            .await
            .map_err(anyhow::Error::msg)?,
        )?;
        report.check(text(&historical) == expected, "the superseded revision is preserved exactly", version.unwrap());
    }
    let replayed = page(
        alpha
            .plan_copy(
                &world.account,
                Request::Merge {
                    submission: Some(fastctx_net::plan_merge::submission(&owner, first.version(), file)),
                    owner: owner.clone(),
                    base: first.version(),
                    markdown: file.into(),
                },
            )
            .await
            .map_err(anyhow::Error::msg)?,
    )?;
    report.check(
        replayed.version() == merged.version() && text(&replayed) == text(&merged),
        "a lost receipt replays the original accepted result",
        "",
    );
    let current = alpha.plan_read_of(&world.account, &owner).await.map_err(anyhow::Error::msg)?;
    report.check(
        current.version() == chosen.version() && text(&current) == text(&chosen),
        "replaying an accepted edit cannot overwrite a later edit",
        "",
    );
    let alien = Address { machine: world.machine("beta").name.clone(), ..owner.clone() };
    let refused = beta
        .plan_copy(
            &world.account,
            Request::Merge { submission: None, owner: alien, base: chosen.version(), markdown: "collision".into() },
        )
        .await;
    report.check(refused.is_err(), "a same-short-name conversation cannot replace another's plan", "");
    let missing = beta
        .plan_copy(
            &world.account,
            Request::Merge {
                submission: None,
                owner: owner.clone(),
                base: Some(99999),
                markdown: "unknown base".into(),
            },
        )
        .await;
    report.check(missing.is_err(), "an unavailable base is not silently treated as an empty plan", "");
    world.hub.restart(Duration::from_millis(100)).await?;
    anyhow::ensure!(world.all_online(Duration::from_secs(60)).await, "machines did not reconnect");
    let retained = beta
        .plan_copy(&world.account, Request::Contains { owner: owner.clone(), markdown: user.into() })
        .await
        .map_err(anyhow::Error::msg)?;
    report.check(
        matches!(retained, Reply::Contains { found: true }),
        "historical content remains verifiable after hub restart",
        "",
    );
    let unchanged = alpha.plan_read_of(&world.account, &owner).await.map_err(anyhow::Error::msg)?;
    report.check(text(&unchanged) == "file conflict\n", "old readers still read the same current page", "");
    use sha2::{Digest, Sha256};
    let opened = alpha.view().account(&world.account).expect("account").opened;
    let digest = format!("{:x}", Sha256::digest(format!("plan\0{}", owner.resident).as_bytes()));
    let history = world
        .hub
        .dir
        .join("account-records")
        .join(world.account.as_str())
        .join(opened.as_nanosecond().to_string())
        .join(format!("{digest}.versions"));
    anyhow::ensure!(history.exists(), "the history fixture never existed");
    alpha.plan_delete(&world.account, &owner.resident).await.map_err(anyhow::Error::msg)?;
    report.check(!history.exists(), "deleting a plan removes all physical revisions and submission receipts", "");
    alpha.plan_delete(&world.account, &owner.resident).await.map_err(anyhow::Error::msg)?;
    report.check(!history.exists(), "retrying a plan deletion leaves no history", "");
    crate::record_following::exercise(world, report).await?;
    Ok(())
}
