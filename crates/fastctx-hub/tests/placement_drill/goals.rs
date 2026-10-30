//! Exercise the actual sentinel read path with a loopback internal-agent model.
use super::*;
use fastctx_types::{
    ResidentId,
    agent::{ModelProtocol, ModelSettings, OutputLimit},
    goals::{Request, Updated, UserChange},
    sentinel_settings::{KeyName, Secret, Settings, SettingsReply, WriteSettings},
};
pub(super) async fn exercise(
    world: &World,
    host: &Path,
    working_plan: &Path,
    id: &ResidentId,
    endpoint: &str,
) -> Result<()> {
    let account = Some(name("alice"));
    let SettingsReply::View { view } = world.sentinel_settings(None, account.clone(), None).await? else {
        anyhow::bail!("sentinel settings conflicted")
    };
    world
        .sentinel_settings(
            None,
            account.clone(),
            Some(WriteSettings {
                expected: view.version,
                settings: Settings {
                    home: Some(machine("alice-two")),
                    model: Some(ModelSettings {
                        protocol: ModelProtocol::OpenAi,
                        endpoint: endpoint.into(),
                        model: "placement-mock".into(),
                        output_limit: OutputLimit::MaxCompletionTokens,
                    }),
                    search: None,
                },
            }),
        )
        .await?;
    world
        .sentinel_key(None, account, KeyName::Model, Some(Secret("placement-fixture-not-a-credential".into())))
        .await?;
    let Updated::Goal { view } = world.goal(id.clone(), Some("host".parse()?), None, Request::Read).await? else {
        anyhow::bail!("goal read conflicted")
    };
    world
        .goal(
            id.clone(),
            Some("host".parse()?),
            None,
            Request::Change {
                operation: "placement-open-goal".into(),
                expected: view.version,
                change: UserChange::Open { objective: "PLACEMENT_GOAL_CANARY".into(), standing: false },
            },
        )
        .await?;
    world.talk(id.clone(),Some("host".parse()?),"ALICE_GOAL_REVIEW [[run: on alice-two <<'GOAL'\nset -e\nprintf 'PLACEMENT_BLOCKED_CANARY' > \"$FASTCTX_SCRATCH/blocked.md\"\nfastctx goal blocked --file \"$FASTCTX_SCRATCH/blocked.md\"\nGOAL]]".into()).await?;
    let mut reviewed = false;
    for _ in 0..480 {
        let updated = world.goal(id.clone(), Some("host".parse()?), None, Request::Read).await?;
        if serde_json::to_string(&updated)?.contains("PLACEMENT_REVIEW_CANARY") {
            reviewed = true;
            break;
        }
        pause().await;
    }
    record("the real goal sentinel read the working materials and reported its judgment", reviewed, json!(null));
    let artifacts = working_plan.parent().unwrap().join("artifacts");
    let material = std::fs::read_dir(artifacts)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.join("full-materials.json").is_file())
        .context("working review artifacts")?;
    record(
        "complete review materials and evidence share the working computer",
        material.join("world-facts.json").is_file(),
        json!(material),
    );
    let original = host.join("world/conversations").join(id.as_str()).join("books/goal-reviews");
    let leaked = std::fs::read_dir(original)?.filter_map(Result::ok).any(|entry| {
        entry.path().join("full-materials.json").exists()
            || entry.path().join("world-facts.json").exists()
            || entry.path().join("previous-judgments.json").exists()
    });
    record("the assisting host keeps no extra complete goal-material files", !leaked, json!(null));
    Ok(())
}
