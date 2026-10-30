//! The phone edits records while the assisted model reads its own computer's projection.
use super::*;
use fastctx_types::{
    ResidentId, TownName,
    memory::{Kind, MemoryCall, MemoryReply},
    project::RecordFields,
};

pub(super) async fn exercise(world: &World, host: &Path, working: &Path, id: &ResidentId) -> Result<()> {
    let town: TownName = "placement-memory".parse()?;
    world
        .open_town(
            town.clone(),
            RecordFields {
                name: Some("Placement memory".into()),
                what: Some("Verify account memory placement".into()),
                machines: Some(vec![machine("alice-two")]),
                ..Default::default()
            },
            Some(name("alice")),
            None,
        )
        .await?;
    world.memory_settings(None, Some(name("alice")), Some(true)).await?;
    world.move_to_project(id.clone(), Some("host".parse()?), Some(town.clone())).await?;
    for (kind, doc, canary) in [
        (Kind::Journal, "placement-observation", "ALICE_JOURNAL_CANARY"),
        (Kind::Spec, "placement-contract", "ALICE_SPEC_CANARY"),
    ] {
        let text = format!("---\nname: {doc}\ndescription: Placement fixture\n---\n{canary}\n");
        anyhow::ensure!(
            matches!(
                world
                    .memory(MemoryCall::Save { town: town.clone(), kind, name: doc.into(), text, expected: None })
                    .await?,
                MemoryReply::Saved { .. }
            ),
            "memory save did not finish"
        );
        let MemoryReply::Document { document } =
            world.memory(MemoryCall::Read { town: town.clone(), name: doc.into(), version: None }).await?
        else {
            anyhow::bail!("no memory document")
        };
        record("the phone reads its authoritative project memory", document.text.contains(canary), json!(doc));
    }
    let folder = WorldDir::under(working).town(&town);
    let journal = folder.journals().join("placement-observation.md");
    let spec = folder.specs().join("placement-contract.md");
    for _ in 0..240 {
        if journal.exists() && spec.exists() {
            break;
        }
        pause().await;
    }
    anyhow::ensure!(journal.exists() && spec.exists(), "project memory never reached the working computer");
    let command =
        format!("ALICE_MEMORY_CASE [[run: on alice-two fastctx view '{}' '{}']]", journal.display(), spec.display());
    world.talk(id.clone(), Some("host".parse()?), command).await?;
    anyhow::ensure!(idle(host, id.as_str()).await, "memory turn did not settle");
    let host_folder = WorldDir::under(host).town(&town);
    record(
        "the assisting host has no project-memory document replicas",
        !host_folder.journals().join("placement-observation.md").exists()
            && !host_folder.specs().join("placement-contract.md").exists(),
        json!(null),
    );
    Ok(())
}
