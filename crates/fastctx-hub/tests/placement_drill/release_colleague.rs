//! Asking a colleague, which works toward no goal in any build: it opens
//! no goal; what it tells with message_colleague reaches the conversation
//! that asked it and wakes it; the end of its turn comes back too; and that
//! conversation no longer waits for it once the turn ended
//! (`world-colleagues.md`, 两个工具). Then its callee works on one at a
//! time: colleagues asked together wait their turns, and a restart while
//! they wait loses none (同事从哪里来、能去哪里).
use super::*;
use fastctx_types::collab::{AgentLogin, AtOnce, Callee, Collab};

/// What the drill's script checks of the models' inputs: the colleague's
/// id, when the scene ran.
pub(super) async fn exercise(
    base: &Path,
    owner: &World,
    listen: SocketAddr,
    mock_configuration: &[u8],
    agent: &AgentName,
) -> Result<Option<Value>> {
    let (root, mut child) = node_with(base, "release-pc", "owner", listen, None, false).await?;
    let pc: MachineName = "release-pc".parse()?;
    let home = base.join("release-codex");
    std::fs::create_dir_all(&home)?;
    std::fs::write(home.join("config.toml"), mock_configuration)?;
    owner.add_agent_login(pc.clone().into(), agent.clone(), home.clone()).await?;
    let installed = owner
        .change_agent_login(pc.clone().into(), agent.clone(), home.clone(), true, true, true, None, Some(true))
        .await?;
    anyhow::ensure!(
        matches!(installed.end, Some(fastctx_types::logins::ChangeEnd::Done { .. })),
        "installing the release machine's mock tools failed: {installed:?}"
    );
    let login = AgentLogin { machine: pc.clone(), agent: agent.clone(), home: Some(home.clone()) };
    let before = owner.collab_read(None, Some(name("owner"))).await?;
    owner
        .collab_write(
            None,
            Some(name("owner")),
            Collab {
                version: before.collab.version,
                on: true,
                callers: vec![login.clone()],
                callees: vec![Callee {
                    name: "release-helper".into(),
                    use_when: "release drill".into(),
                    login,
                    model: Some("mock-model".into()),
                    effort: None,
                    screen: false,
                    at_once: None,
                }],
                prompt: None,
                rules: String::new(),
            },
        )
        .await?;
    let parent = owner
        .open_with(
            agent.clone(),
            None,
            Some(pc.clone().into()),
            "RELEASE_PARENT".into(),
            Default::default(),
            fastctx_types::agents::Choices { home: Some(home), ..Default::default() },
        )
        .await?;
    let parent = parent.to.resident;
    anyhow::ensure!(idle_after(&root, parent.as_str(), None).await, "the asking conversation did not settle");
    let mut offered = false;
    for _ in 0..240 {
        let machines =
            ask(&root, json!({"call":"machines","resident":parent,"login":null,"thread":null,"scratch":false})).await?;
        if machines.to_string().contains("release-helper") {
            offered = true;
            break;
        }
        pause().await;
    }
    anyhow::ensure!(offered, "the release machine's collaboration settings did not reach it");
    let asked_turn = last_turn(&root, parent.as_str());
    let reply = ask(
        &root,
        json!({"call":"ask","resident":parent,"label":"release colleague",
               "prompt":"RELEASE_COLLEAGUE [[note: RELEASE_TOLD]]","callee":"release-helper","login":null}),
    )
    .await?;
    let colleague: fastctx_types::Address =
        serde_json::from_value(reply["colleague"].clone()).context("the release colleague's address")?;
    let id = colleague.resident.to_string();
    // Its request is kept before the asker hears of it: the asker waits for
    // it from then on.
    let evidence =
        ask(&root, json!({"call":"turn_evidence","resident":parent,"turn":null,"refresh_limits":false})).await?;
    let waited = evidence["evidence"]["waiting"].to_string().contains(&id);
    anyhow::ensure!(idle_after(&root, &id, None).await, "the release colleague did not settle");
    record("a colleague opens no goal", block(&root, &id, "goal").is_empty(), json!(block(&root, &id, "goal")));
    // What it told wakes the conversation that asked it, and so does the end
    // of its turn: the asker takes a turn after the one it asked in.
    let woke = idle_after(&root, parent.as_str(), asked_turn).await;
    record("what a colleague tells in a release wakes the conversation that asked it", woke, json!(null));
    let mut waiting = Value::Null;
    let mut settled = false;
    for _ in 0..480 {
        let evidence =
            ask(&root, json!({"call":"turn_evidence","resident":parent,"turn":null,"refresh_limits":false})).await?;
        waiting = evidence["evidence"]["waiting"].clone();
        if !waiting.to_string().contains(&id) {
            settled = true;
            break;
        }
        pause().await;
    }
    record(
        "in a release, the conversation that asked waits for its colleague until the colleague's turn ended, and no more",
        waited && settled,
        json!({"waited": waited, "after": waiting}),
    );
    one_at_a_time(owner, &root, &mut child, &parent).await?;
    let _ = child.kill();
    let _ = child.wait();
    Ok(Some(json!({"colleague": id, "parent": parent})))
}

/// Three colleagues of a callee that works on one at a time, asked together:
/// the first starts and the others wait in turn; the node restarted while
/// they wait, each still has its turn, and never two at once.
async fn one_at_a_time(
    owner: &World,
    root: &Path,
    child: &mut Child,
    parent: &fastctx_types::ResidentId,
) -> Result<()> {
    let mut collab = owner.collab_read(None, Some(name("owner"))).await?.collab;
    collab.callees[0].at_once = Some(AtOnce::Most(std::num::NonZeroU32::MIN));
    owner.collab_write(None, Some(name("owner")), collab).await?;
    let mut limited = false;
    for _ in 0..240 {
        let machines =
            ask(root, json!({"call":"machines","resident":parent,"login":null,"thread":null,"scratch":false})).await?;
        if machines.to_string().contains("\"at_once\":1") {
            limited = true;
            break;
        }
        pause().await;
    }
    anyhow::ensure!(limited, "the callee's limit did not reach its machine");
    let mut asked = Vec::new();
    for index in 0..3 {
        let reply = ask(
            root,
            json!({"call":"ask","resident":parent,"label":format!("queued {index}"),
                   "prompt":"RELEASE_QUEUE [[slow: 4]]","callee":"release-helper","login":null}),
        )
        .await?;
        let address: fastctx_types::Address =
            serde_json::from_value(reply["colleague"].clone()).context("a queued colleague's address")?;
        asked.push((address.resident.to_string(), reply["waiting"].as_u64()));
    }
    record(
        "with one at once, the first of three colleagues asked together starts and the others wait in turn",
        asked[0].1.is_none() && asked[1].1 == Some(0) && asked[2].1 == Some(1),
        json!(asked),
    );
    let ids: Vec<&str> = asked.iter().map(|(id, _)| id.as_str()).collect();
    let mut most = 0;
    let mut restarted = false;
    let mut done = false;
    for _ in 0..960 {
        let nows: Vec<Value> = ids.iter().map(|id| now(root, id)).collect();
        most = most.max(nows.iter().filter(|now| !now["turn"].is_null()).count());
        // Restarted once the first is at work and the others wait.
        if !restarted && !nows[0]["turn"].is_null() {
            child.kill()?;
            child.wait()?;
            *child = start_without_preview(root)?;
            anyhow::ensure!(online(root).await, "the release machine did not come back");
            restarted = true;
            continue;
        }
        if restarted && nows[1..].iter().all(|now| now["activity"] == "idle" && now["last_turn"].is_string()) {
            done = true;
            break;
        }
        pause().await;
    }
    record(
        "the colleagues that waited each had their turn after the node restarted, never two of them at once",
        restarted && done && most <= 1,
        json!({"restarted": restarted, "done": done, "most at once": most}),
    );
    Ok(())
}
