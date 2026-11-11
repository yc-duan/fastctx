//! A user's project deletion reaches the management job and its ordinary result.

use super::*;

pub(super) async fn exercise(base: &Path, world: &World) -> Result<()> {
    let town: fastctx_types::TownName = "submitted-project-deletion".parse()?;
    world
        .open_town(
            town.clone(),
            fastctx_types::project::RecordFields {
                name: Some("Submitted deletion fixture".into()),
                what: Some("A project removed through the user's management path".into()),
                ..Default::default()
            },
            Some(name("owner")),
            None,
        )
        .await?;
    anyhow::ensure!(world.town(Some(town.clone())).await.is_ok(), "the project fixture was not created");
    let before = ran_after(base, jiff::Timestamp::now(), 0, "job.ran.ask/manage").await?;
    let reply = world.account(None, name("owner"), AccountChange::DeleteProject { town: town.clone() }).await?;
    let after = ran_after(base, jiff::Timestamp::now(), before + 1, "job.ran.ask/manage").await?;
    let listed = world.towns().await?;
    let missing = world.town(Some(town.clone())).await.is_err();
    record(
        "product project deletion runs one management job and returns the ordinary empty result",
        reply.is_none() && missing && !listed.iter().any(|row| row.town == town) && after == before + 1,
        json!({"project":town,"ran_before":before,"ran_after":after,"no_token":reply.is_none(),"project_missing":missing}),
    );
    Ok(())
}
