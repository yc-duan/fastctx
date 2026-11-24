//! Deleting a conversation deletes the colleagues it asked with it (the user,
//! 2026-11-23: 删除一段会话，那这段会话他派发的同事也要一并删除): those on its
//! machine there and then, and one on another machine by the hub, which has
//! that machine delete it.
use super::*;
use fastctx_types::{Address, ResidentId, collab::AgentLogin};

/// Deletes `parent`, on the machine at `root` named `pc`, which asked
/// `colleagues` there; then has a conversation on a second machine ask the
/// callee on `pc`, and deletes that conversation.
#[allow(clippy::too_many_arguments)]
pub(super) async fn exercise(
    base: &Path,
    owner: &World,
    listen: SocketAddr,
    mock_configuration: &[u8],
    agent: &AgentName,
    root: &Path,
    pc: &MachineName,
    parent: &ResidentId,
    colleagues: &[String],
) -> Result<()> {
    let gone = |root: &Path, id: &str| !root.join("world/conversations").join(id).exists();
    owner.forget_conversation(parent.clone(), pc.to_string().parse()?).await?;
    let mut all = false;
    for _ in 0..480 {
        if gone(root, parent.as_str()) && colleagues.iter().all(|id| gone(root, id)) {
            all = true;
            break;
        }
        pause().await;
    }
    record(
        "deleting a conversation deletes the colleagues it asked on its machine",
        all && !colleagues.is_empty(),
        json!({
            "parent gone": gone(root, parent.as_str()),
            "colleagues": colleagues.iter().map(|id| (id.clone(), gone(root, id))).collect::<Vec<_>>(),
        }),
    );
    // A conversation on another machine asks the callee on `pc`.
    let (two, mut child) = node_with(base, "release-two", "owner", listen, None, false).await?;
    let second: MachineName = "release-two".parse()?;
    let home = base.join("release-two-codex");
    std::fs::create_dir_all(&home)?;
    std::fs::write(home.join("config.toml"), mock_configuration)?;
    owner.add_agent_login(second.clone().into(), agent.clone(), home.clone()).await?;
    let installed = owner
        .change_agent_login(second.clone().into(), agent.clone(), home.clone(), true, true, true, None, Some(true))
        .await?;
    anyhow::ensure!(
        matches!(installed.end, Some(fastctx_types::logins::ChangeEnd::Done { .. })),
        "installing the second release machine's mock tools failed: {installed:?}"
    );
    let mut collab = owner.collab_read(None, Some(name("owner"))).await?.collab;
    collab.callers.push(AgentLogin { machine: second.clone(), agent: agent.clone(), home: Some(home.clone()) });
    owner.collab_write(None, Some(name("owner")), collab).await?;
    let asker = owner
        .open_with(
            agent.clone(),
            None,
            Some(second.clone().into()),
            "FORGET_FAR_PARENT".into(),
            Default::default(),
            fastctx_types::agents::Choices { home: Some(home), ..Default::default() },
        )
        .await?
        .to
        .resident;
    anyhow::ensure!(
        idle_after(&two, asker.as_str(), None).await,
        "the asking conversation on release-two did not settle"
    );
    let mut offered = false;
    for _ in 0..240 {
        let machines =
            ask(&two, json!({"call":"machines","resident":asker,"login":null,"thread":null,"scratch":false})).await?;
        if machines.to_string().contains("release-helper") {
            offered = true;
            break;
        }
        pause().await;
    }
    anyhow::ensure!(offered, "the callee on release-pc was not offered to a conversation on release-two");
    let reply = ask(
        &two,
        json!({"call":"ask","resident":asker,"label":"far ask","prompt":"FORGET_FAR_ASK",
               "callee":"release-helper","login":null}),
    )
    .await?;
    let far: Address = serde_json::from_value(reply["colleague"].clone()).context("the far colleague's address")?;
    anyhow::ensure!(idle_after(root, far.resident.as_str(), None).await, "the far colleague did not settle");
    record(
        "a conversation on another machine asks a colleague on the first",
        far.machine == *pc && !gone(root, far.resident.as_str()),
        json!(far),
    );
    owner.forget_conversation(asker.clone(), second.to_string().parse()?).await?;
    let mut far_gone = false;
    for _ in 0..480 {
        if gone(root, far.resident.as_str()) && gone(&two, asker.as_str()) {
            far_gone = true;
            break;
        }
        pause().await;
    }
    record(
        "deleting it deletes its colleague on the other machine, by the hub",
        far_gone,
        json!({"colleague": far, "colleague gone": gone(root, far.resident.as_str()), "asker gone": gone(&two, asker.as_str())}),
    );
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}
