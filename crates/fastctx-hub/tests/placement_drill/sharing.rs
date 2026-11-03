//! Whole-machine sharing grants records, while assistance and administration do not.
use super::*;
use fastctx_types::{
    Address, ResidentId, TownName,
    history::{HistoryCall, HistoryReply, Scope},
    project::RecordFields,
};

fn history(owner: &Address) -> Scope {
    Scope::Conversation { resident: owner.resident.clone(), machine: owner.machine.clone() }
}
fn copy(root: &Path, machine_name: &str, owner: &Address) -> PathBuf {
    WorldDir::under(root)
        .owner(&fastctx_types::layout::Names { me: machine(machine_name), was: Vec::new(), renamed: Vec::new() }, owner)
        .path()
        .to_path_buf()
}
fn text(name: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: Shared record fixture\n---\n2026-10-31: {body}\n")
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn exercise(
    base: &Path,
    host: &Path,
    owner_pc: &Path,
    owner: &World,
    bob: &World,
    owner_id: &ResidentId,
    private_bob: &Address,
    home: &Path,
) -> Result<()> {
    let private = history(private_bob);
    bob.history(HistoryCall::Save {
        scope: private.clone(),
        name: "private-record".into(),
        text: text("private-record", "PRIVATE_BOB_HISTORY"),
        expected: None,
    })
    .await?;
    record(
        "administration alone cannot read another account's history",
        owner.history(HistoryCall::List { scope: private.clone() }).await.is_err(),
        json!(null),
    );
    let terminal = World::new(WorldDir::under(host), Peer::Command { resident: None, session: None });
    record(
        "the host's local user cannot delete an assisted account's private conversation",
        terminal.forget_conversation(private_bob.resident.clone(), "host".parse()?).await.is_err(),
        json!(null),
    );
    record(
        "administration alone receives no private conversation name",
        !serde_json::to_string(&owner.who(None, true).await?)?.contains(private_bob.resident.as_str()),
        json!(null),
    );
    owner
        .account(
            None,
            name("owner"),
            AccountChange::Share { machine: machine("host"), to: name("bob"), shared: true, jobs: None },
        )
        .await?;
    let shared = bob
        .open_with(
            "codex".parse()?,
            None,
            Some("host".parse()?),
            "BOB_SHARED_BIRTH".into(),
            Default::default(),
            fastctx_types::agents::Choices { home: Some(home.to_path_buf()), ..Default::default() },
        )
        .await?;
    anyhow::ensure!(idle_after(host, shared.to.resident.as_str(), None).await, "shared birth did not settle");
    let scope = history(&shared.to);
    let mut watch = owner.watch_plan(None, None, shared.to.resident.clone()).await?;
    tokio::time::timeout(Duration::from_secs(20), watch.next()).await??;
    bob.plan_write(None, None, shared.to.resident.clone(), None, "SHARED_PLAN".into(), Some(machine("host"))).await?;
    let changed = tokio::time::timeout(Duration::from_secs(20), watch.next()).await??;
    record(
        "a co-owner's plan watch follows the actual owning account",
        changed.is_some_and(|metadata| metadata.version == 1),
        json!(null),
    );
    let first = bob
        .history(HistoryCall::Save {
            scope: scope.clone(),
            name: "shared-record".into(),
            text: text("shared-record", "SHARED_HISTORY"),
            expected: None,
        })
        .await?;
    let HistoryReply::Saved { revision } = first else { anyhow::bail!("history save had no revision") };
    owner
        .history(HistoryCall::Save {
            scope: scope.clone(),
            name: "shared-record".into(),
            text: text("shared-record", "COOWNER_EDIT"),
            expected: Some(revision.version),
        })
        .await?;
    let owner_copy = copy(owner_pc, "owner-pc", &shared.to).join("history/shared-record.md");
    let bob_copy = copy(&base.join("bob-pc"), "bob-pc", &shared.to).join("history/shared-record.md");
    record(
        "shared histories reach both owners' computers",
        file_is(&owner_copy, &text("shared-record", "COOWNER_EDIT")).await
            && file_is(&bob_copy, &text("shared-record", "COOWNER_EDIT")).await,
        json!(null),
    );
    record(
        "sharing later does not expose an assisted birth",
        owner.history(HistoryCall::List { scope: private.clone() }).await.is_err()
            && !host
                .join("world/conversations")
                .join(private_bob.resident.as_str())
                .join("history/private-record.md")
                .exists(),
        json!(null),
    );
    let who = serde_json::to_string(&owner.who(None, true).await?)?;
    record(
        "a shared conversation carries its opener marker",
        who.contains(shared.to.resident.as_str()) && who.contains("\"openedBy\":{\"account\":\"bob\"}"),
        json!(null),
    );
    let local = World::new(WorldDir::under(host), Peer::Command { resident: Some(shared.to.clone()), session: None });
    super::shared_controls::exercise(owner, bob, &local, &shared.to).await?;
    let town: TownName = "shared-machine-history".parse()?;
    local
        .open_town(
            town.clone(),
            RecordFields {
                name: Some("Shared machine history".into()),
                what: Some("Ownership drill".into()),
                machines: Some(vec![machine("host")]),
                ..Default::default()
            },
            Some(name("bob")),
            None,
        )
        .await?;
    let project = Scope::Project { project: town.clone() };
    bob.history(HistoryCall::Save {
        scope: project.clone(),
        name: "shared-project".into(),
        text: text("shared-project", "PROJECT_COOWNED"),
        expected: None,
    })
    .await?;
    record(
        "a project opened on a shared machine belongs to its holders",
        owner.history(HistoryCall::Read { scope: project, name: "shared-project".into(), version: None }).await.is_ok(),
        json!(null),
    );
    owner.history(HistoryCall::Retire { scope: scope.clone(), name: "shared-record".into(), expected: 2 }).await?;
    let HistoryReply::Document { document } =
        bob.history(HistoryCall::Read { scope: scope.clone(), name: "shared-record".into(), version: None }).await?
    else {
        anyhow::bail!("history document missing")
    };
    record(
        "retirement preserves all revisions",
        document.revision.retired && document.versions.len() == 3,
        json!(null),
    );
    bob.history(HistoryCall::Restore { scope: scope.clone(), name: "shared-record".into(), version: 1 }).await?;
    record(
        "restoration republishes the chosen text",
        file_is(&bob_copy, &text("shared-record", "SHARED_HISTORY")).await,
        json!(null),
    );
    let doomed = bob
        .open_with(
            "codex".parse()?,
            None,
            Some("host".parse()?),
            "SHARED_DELETE_CASE".into(),
            Default::default(),
            fastctx_types::agents::Choices { home: Some(home.to_path_buf()), ..Default::default() },
        )
        .await?;
    anyhow::ensure!(
        idle_after(host, doomed.to.resident.as_str(), None).await,
        "the shared deletion fixture did not settle"
    );
    bob.history(HistoryCall::Save {
        scope: history(&doomed.to),
        name: "delete-record".into(),
        text: text("delete-record", "SHARED_DELETE_CANARY"),
        expected: None,
    })
    .await?;
    let doomed_owner = copy(owner_pc, "owner-pc", &doomed.to).join("history/delete-record.md");
    let doomed_bob = copy(&base.join("bob-pc"), "bob-pc", &doomed.to).join("history/delete-record.md");
    anyhow::ensure!(
        file_is(&doomed_owner, &text("delete-record", "SHARED_DELETE_CANARY")).await
            && file_is(&doomed_bob, &text("delete-record", "SHARED_DELETE_CANARY")).await,
        "both owners must have the document before deletion"
    );
    owner.forget_conversation(doomed.to.resident.clone(), "host".parse()?).await?;
    for _ in 0..240 {
        if !doomed_owner.exists() && !doomed_bob.exists() {
            break;
        }
        pause().await;
    }
    record(
        "a co-owner deletes the same conversation and withdraws both accounts' copies",
        !doomed_owner.exists()
            && !doomed_bob.exists()
            && bob
                .history(HistoryCall::Read {
                    scope: history(&doomed.to),
                    name: "delete-record".into(),
                    version: Some(1),
                })
                .await
                .is_err(),
        json!(null),
    );
    let original_owner = Address { machine: machine("host"), resident: owner_id.clone() };
    let received = copy(&base.join("bob-pc"), "bob-pc", &original_owner).join("plan.md");
    record("sharing delivers the host owner's existing plan", file_is(&received, "").await, json!(received));
    owner
        .account(
            None,
            name("owner"),
            AccountChange::Share { machine: machine("host"), to: name("bob"), shared: false, jobs: None },
        )
        .await?;
    for _ in 0..240 {
        if !received.exists() {
            break;
        }
        pause().await;
    }
    record(
        "revocation withdraws the other owner's copy",
        !received.exists() && bob.history(HistoryCall::List { scope: history(&original_owner) }).await.is_err(),
        json!(null),
    );
    record(
        "the opener retains its own records after revocation",
        bob.history(HistoryCall::Read { scope, name: "shared-record".into(), version: None }).await.is_ok()
            && bob_copy.exists(),
        json!(null),
    );
    Ok(())
}
