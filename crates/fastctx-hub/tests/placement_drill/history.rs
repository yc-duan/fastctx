//! The phone edits records while the assisted model reads its own computer's projection.
use super::*;
use fastctx_types::{
    ResidentId, TownName,
    history::{HistoryCall, HistoryReply, Scope},
    project::RecordFields,
};

pub(super) async fn exercise(
    world: &World,
    host: &Path,
    working: &Path,
    id: &ResidentId,
    actor: &fastctx_types::Address,
) -> Result<()> {
    let town: TownName = "placement-history".parse()?;
    world
        .open_town(
            town.clone(),
            RecordFields {
                name: Some("Placement history".into()),
                what: Some("Verify account history placement".into()),
                machines: Some(vec![machine("alice-two")]),
                ..Default::default()
            },
            Some(name("alice")),
            None,
        )
        .await?;
    world.move_to_project(id.clone(), Some("host".parse()?), Some(town.clone())).await?;
    for (doc, canary) in
        [("placement-observation", "ALICE_OBSERVATION_CANARY"), ("placement-contract", "ALICE_CONTRACT_CANARY")]
    {
        let text = format!("---\nname: {doc}\ndescription: Placement fixture\n---\n{canary}\n");
        anyhow::ensure!(
            matches!(
                world
                    .history(HistoryCall::Save {
                        scope: Scope::Project { project: town.clone() },
                        name: doc.into(),
                        text,
                        expected: None
                    })
                    .await?,
                HistoryReply::Saved { .. }
            ),
            "history save did not finish"
        );
        let HistoryReply::Document { document } = world
            .history(HistoryCall::Read {
                scope: Scope::Project { project: town.clone() },
                name: doc.into(),
                version: None,
            })
            .await?
        else {
            anyhow::bail!("no history document")
        };
        record("the phone reads its authoritative project history", document.text.contains(canary), json!(doc));
    }
    let folder = WorldDir::under(working).town(&town);
    let journal = folder.path().join("history").join("placement-observation.md");
    let spec = folder.path().join("history").join("placement-contract.md");
    for _ in 0..240 {
        if journal.exists() && spec.exists() {
            break;
        }
        pause().await;
    }
    anyhow::ensure!(journal.exists() && spec.exists(), "project history never reached the working computer");
    let command =
        format!("ALICE_HISTORY_CASE [[run: on alice-two fastctx view '{}' '{}']]", journal.display(), spec.display());
    let before = last_turn(host, id.as_str());
    world.talk(id.clone(), Some("host".parse()?), command).await?;
    anyhow::ensure!(idle_after(host, id.as_str(), before).await, "history turn did not settle");
    let host_folder = WorldDir::under(host).town(&town);
    record(
        "the assisting host has no project-history document replicas",
        !host_folder.path().join("history").join("placement-observation.md").exists()
            && !host_folder.path().join("history").join("placement-contract.md").exists(),
        json!(null),
    );
    let scope = Scope::Project { project: town.clone() };
    let mut large = "---\nname: large-history\ndescription: Full transport boundary\n---\n".to_string();
    large.push_str(&"x".repeat(fastctx_types::history::MAX_DOCUMENT_BYTES - large.len()));
    world
        .history(HistoryCall::Save {
            scope: scope.clone(),
            name: "large-history".into(),
            text: large.clone(),
            expected: None,
        })
        .await?;
    let body = current(world, &scope, "large-history").await?;
    record(
        "a full 512 KiB history roundtrips through the hub and reaches a computer",
        body.text == large && file_is(&folder.path().join("history/large-history.md"), &large).await,
        json!(large.len()),
    );
    large.push('x');
    record(
        "history over 512 KiB is refused without creating another version",
        world
            .history(HistoryCall::Save {
                scope: scope.clone(),
                name: "large-history".into(),
                text: large,
                expected: Some(1),
            })
            .await
            .is_err()
            && current(world, &scope, "large-history").await?.versions.len() == 1,
        json!(null),
    );
    let alice_pc = host.parent().unwrap().join("alice-pc");
    let target = WorldDir::under(&alice_pc).town(&town).path().join("history/placement-contract.md");
    let patch = format!(
        "*** Begin Patch\n*** Update File: {}\n@@\n-ALICE_CONTRACT_CANARY\n+PATCH_BYPASS\n*** End Patch",
        target.display()
    );
    let rejected = operation_as(&alice_pc, actor, json!({"tool":"apply_patch","input":patch})).await;
    record(
        "apply_patch has no history submission bypass",
        rejected.is_err() && current(world, &scope, "placement-contract").await?.text.contains("ALICE_CONTRACT_CANARY"),
        json!(format!("{rejected:?}")),
    );
    let invalid = target.parent().unwrap().join("invalid-copy.md");
    let refused = write_as(&alice_pc, actor, &invalid, "not a history header").await;
    ask(&alice_pc, json!({"call":"prepare_copies","paths":[invalid]})).await?;
    record("a rejected new history never becomes an empty replica", refused.is_err() && !invalid.exists(), json!(null));
    concurrent(world, host, working, id, &town).await?;
    let state_path = host.join("world/conversations").join(id.as_str()).join("context-state.json");
    let before: Value = serde_json::from_slice(&std::fs::read(&state_path)?)?;
    world.control(id.clone(), Some("host".parse()?), fastctx_types::session::control::Control::Compact).await?;
    let mut compacted = false;
    for _ in 0..240 {
        let state: Value = serde_json::from_slice(&std::fs::read(&state_path)?)?;
        if state["compacted"].is_string()
            && state["compacted"] != before["compacted"]
            && state["epoch"].as_u64() > before["epoch"].as_u64()
        {
            compacted = true;
            break;
        }
        pause().await;
    }
    anyhow::ensure!(compacted, "the real host did not complete compaction");
    turn(world, host, id, "HISTORY_AFTER_COMPACT".into()).await?;
    record("the real host completed compaction before the next history-bearing turn", true, json!(null));
    Ok(())
}

async fn current(world: &World, scope: &Scope, name: &str) -> Result<fastctx_types::history::Document> {
    match world.history(HistoryCall::Read { scope: scope.clone(), name: name.into(), version: None }).await? {
        HistoryReply::Document { document } => Ok(*document),
        other => anyhow::bail!("expected a history document: {other:?}"),
    }
}

async fn turn(world: &World, host: &Path, id: &ResidentId, command: String) -> Result<()> {
    let before = last_turn(host, id.as_str());
    world.talk(id.clone(), Some("host".parse()?), command).await?;
    anyhow::ensure!(idle_after(host, id.as_str(), before).await, "the concurrent history turn did not settle");
    Ok(())
}

async fn concurrent(world: &World, host: &Path, working: &Path, id: &ResidentId, town: &TownName) -> Result<()> {
    let scope = Scope::Project { project: town.clone() };
    let name = "concurrent-history";
    let original = format!("---\nname: {name}\ndescription: Concurrent history fixture\n---\nBASE_ALPHA\nBASE_BETA\n");
    world
        .history(HistoryCall::Save { scope: scope.clone(), name: name.into(), text: original.clone(), expected: None })
        .await?;
    let left =
        WorldDir::under(&host.parent().unwrap().join("alice-pc")).town(town).path().join(format!("history/{name}.md"));
    let right = WorldDir::under(working).town(town).path().join(format!("history/{name}.md"));
    anyhow::ensure!(
        file_is(&left, &original).await && file_is(&right, &original).await,
        "both history copies must exist before concurrent writes"
    );
    let mut watch = world.watch_history(scope.clone()).await?;
    tokio::time::timeout(Duration::from_secs(20), watch.next()).await??;
    let completed = host.parent().unwrap().join("alice-pc/history-append-completed");
    let command = format!(
        "HISTORY_APPEND_PAIR [[run: on alice-pc <<'PAIR'\nset -e\nfastctx view '{left}'\non alice-two fastctx view '{right}'\nwrite '{left}' <<'LEFT' &\n{original}LEFT_APPEND\nLEFT\nl=$!\non alice-two write '{right}' <<'RIGHT' &\n{original}RIGHT_APPEND\nRIGHT\nr=$!\nwait \"$l\"\nwait \"$r\"\nprintf done > '{completed}'\nPAIR]]",
        left = left.display(),
        right = right.display(),
        completed = completed.display()
    );
    turn(world, host, id, command).await?;
    // A notice can end run's wait and the mock turn before its job finishes.
    anyhow::ensure!(file_is(&completed, "done").await, "the concurrent history job did not finish");
    let appended = current(world, &scope, name).await?;
    record(
        "concurrent whole-file appends retain both tails exactly once",
        appended.revision.version == 3
            && appended.text.matches("LEFT_APPEND").count() == 1
            && appended.text.matches("RIGHT_APPEND").count() == 1,
        json!(appended.revision),
    );
    let catalog = tokio::time::timeout(Duration::from_secs(20), watch.next()).await??;
    record(
        "the phone history watch receives committed changes",
        catalog.documents.iter().any(|revision| revision.name == name && revision.version > 1),
        json!(null),
    );
    record(
        "a stale UI version is rejected with the current version",
        world
            .history(HistoryCall::Save {
                scope: scope.clone(),
                name: name.into(),
                text: original.clone(),
                expected: Some(1),
            })
            .await
            .err()
            .is_some_and(|error| error.to_string().contains('3')),
        json!(null),
    );
    let completed = host.parent().unwrap().join("alice-pc/history-edit-completed");
    let command = format!(
        "HISTORY_EDIT_PAIR [[run: on alice-pc <<'PAIR'\nset -e\nedit '{left}' <<'LEFT' &\n<<<<<<< SEARCH\nBASE_ALPHA\n=======\nEDIT_ALPHA\n>>>>>>> REPLACE\nLEFT\nl=$!\non alice-two edit '{right}' <<'RIGHT' &\n<<<<<<< SEARCH\nBASE_BETA\n=======\nEDIT_BETA\n>>>>>>> REPLACE\nRIGHT\nr=$!\nwait \"$l\"\nwait \"$r\"\nprintf done > '{completed}'\nPAIR]]",
        left = left.display(),
        right = right.display(),
        completed = completed.display()
    );
    turn(world, host, id, command).await?;
    // A notice can end run's wait and the mock turn before its job finishes.
    anyhow::ensure!(file_is(&completed, "done").await, "the concurrent history job did not finish");
    let edited = current(world, &scope, name).await?;
    record(
        "concurrent exact edits rebase on the latest document",
        edited.revision.version == 5
            && edited.text.contains("EDIT_ALPHA")
            && edited.text.contains("EDIT_BETA")
            && edited.text.contains("LEFT_APPEND")
            && edited.text.contains("RIGHT_APPEND"),
        json!(edited.revision),
    );
    anyhow::ensure!(
        file_is(&left, &edited.text).await && file_is(&right, &edited.text).await,
        "edited histories did not converge"
    );
    let a = edited.text.replace("EDIT_ALPHA", "CONFLICT_LEFT");
    let b = edited.text.replace("EDIT_ALPHA", "CONFLICT_RIGHT");
    let completed = host.parent().unwrap().join("alice-pc/history-conflict-completed");
    let command = format!(
        "HISTORY_CONFLICT_PAIR [[run: on alice-pc <<'PAIR'\nfastctx view '{left}'\non alice-two fastctx view '{right}'\nwrite '{left}' <<'LEFT' &\n{a}LEFT\nl=$!\non alice-two write '{right}' <<'RIGHT' &\n{b}RIGHT\nr=$!\nwait \"$l\"; l=$?\nwait \"$r\"; r=$?\nprintf 'HISTORY_CONFLICT_STATUS %s %s\\n' \"$l\" \"$r\"\nprintf done > '{completed}'\nPAIR]]",
        left = left.display(),
        right = right.display(),
        completed = completed.display()
    );
    turn(world, host, id, command).await?;
    // A notice can end run's wait and the mock turn before its job finishes.
    anyhow::ensure!(file_is(&completed, "done").await, "the concurrent history job did not finish");
    let conflict = current(world, &scope, name).await?;
    record(
        "a true concurrent conflict commits one revision",
        conflict.revision.version == 6 && (conflict.text == a || conflict.text == b),
        json!(conflict.revision),
    );
    let old = world.history(HistoryCall::Read { scope, name: name.into(), version: Some(1) }).await?;
    record(
        "old revisions remain byte-for-byte readable after concurrent changes",
        matches!(old, HistoryReply::Document { document } if document.text == original && document.versions.len() == 6),
        json!(null),
    );
    Ok(())
}
