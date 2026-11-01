//! Job progress between this release and the earlier one, as a network has
//! it while its machines update one by one: a conversation of this release
//! runs a script on a machine of the earlier release, and a conversation that
//! lives on that machine runs one on a machine of this release. Each sees
//! every step of the part run on the other side once, ended, with its output.
use super::*;
use fastctx_types::run_facts::{RunFacts, StepDid};
use fastctx_types::session::{Happened, Watched, What};
use fastctx_types::{AgentName, ResidentId};

/// The script a conversation runs on `executor`: a file looked at there, then
/// an `on` from there to `nested`, each a step of the job on `executor`.
fn script(tag: &str, executor: &str, nested: &str) -> String {
    format!(
        "{tag} [[run: on {executor} <<'OUTER'\nfastctx view /etc/os-release > /dev/null\non {nested} <<'INNER'\necho {tag}_NESTED\nINNER\necho {tag}_DONE\nOUTER]]"
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn exercise(
    base: &Path,
    world: &World,
    listen: SocketAddr,
    host: &Path,
    conversation: &ResidentId,
    agent: &AgentName,
    mock_configuration: &[u8],
    earlier: &Path,
) -> Result<()> {
    let (old, mut old_node) = node_with(base, "old-pc", "alice", listen, Some(earlier)).await?;

    // The earlier machine runs the part, this release's conversation keeps
    // its facts.
    let before = last_turn(host, conversation.as_str());
    world.talk(conversation.clone(), Some("host".parse()?), script("EARLIER_EXECUTOR", "old-pc", "alice-pc")).await?;
    anyhow::ensure!(
        idle_after(host, conversation.as_str(), before).await,
        "the run on the earlier machine did not settle"
    );
    let facts = last_run(world, conversation, "host").await?;
    steps(
        facts.as_ref(),
        "old-pc",
        "alice-pc",
        "EARLIER_EXECUTOR",
        "an earlier machine's steps of a job reached this release's conversation, each once",
    );

    // A conversation on the earlier machine, answered by the same model,
    // has a machine of this release run the part.
    let login = base.join("old-codex");
    std::fs::create_dir_all(&login)?;
    std::fs::write(login.join("config.toml"), mock_configuration)?;
    world.add_agent_login("old-pc".parse()?, agent.clone(), login.clone()).await?;
    let installed = world.change_agent_login("old-pc".parse()?, agent.clone(), login.clone(), true, true, true).await?;
    anyhow::ensure!(
        matches!(installed.end, Some(fastctx_types::logins::ChangeEnd::Done { .. })),
        "installing the earlier machine's mock tools failed: {installed:?}"
    );
    let mut ready = false;
    for _ in 0..120 {
        let listed = serde_json::to_value(world.agents(Some("old-pc".parse()?)).await?)?;
        if listed.to_string().contains(&login.to_string_lossy().to_string()) {
            ready = true;
            break;
        }
        pause().await;
    }
    anyhow::ensure!(ready, "the earlier machine never advertised its mock login");
    let opened = world
        .open_with(
            agent.clone(),
            None,
            Some("old-pc".parse()?),
            script("EARLIER_RECEIVER", "alice-pc", "alice-two"),
            Default::default(),
            fastctx_types::agents::Choices { home: Some(login.clone()), ..Default::default() },
        )
        .await
        .context("opening a conversation on the earlier machine")?;
    let resident = opened.to.resident;
    anyhow::ensure!(
        idle_after(&old, resident.as_str(), None).await,
        "the earlier machine's conversation did not settle"
    );
    let facts = last_run(world, &resident, "old-pc").await?;
    steps(
        facts.as_ref(),
        "alice-pc",
        "alice-two",
        "EARLIER_RECEIVER",
        "this release's steps of a job reached the earlier machine's conversation, each once",
    );
    old_node.kill()?;
    old_node.wait()?;
    upgrade(&old, world, &resident, earlier).await
}

/// The earlier machine updated in place, as its user installing this release
/// does: its World moves forward though killed along the way, answers as it
/// did, and the earlier release installed again stops instead of misreading it.
async fn upgrade(old: &Path, world: &World, resident: &ResidentId, earlier: &Path) -> Result<()> {
    let before = history(world, resident, "old-pc").await?;
    let installed = old.join("bin/fastctx");
    let this = KERNEL.get().expect("kernel path");
    replace(&installed, this)?;
    // Killed outright, the first time early, the others while it moves the
    // earlier records in.
    for after in [300, 900, 1700] {
        let mut node = start_with(old, &installed)?;
        tokio::time::sleep(Duration::from_millis(after)).await;
        node.kill()?;
        node.wait()?;
    }
    let mut node = start_with(old, &installed)?;
    record(
        "this release served the earlier machine's World, killed three times along its move",
        online(old).await,
        json!(null),
    );
    let after = history(world, resident, "old-pc").await?;
    record(
        "the earlier conversation's history reads the same after the move",
        before == after,
        json!({"before": before.len(), "after": after.len(), "first difference": before.iter().zip(&after).position(|(a, b)| a != b)}),
    );
    node.kill()?;
    node.wait()?;
    let format = std::fs::read_to_string(old.join("world/format.json")).unwrap_or_default();
    record("the World says the format this release wrote", format.contains("\"format\": 2"), json!(format));

    replace(&installed, earlier)?;
    let mut refused = start_with(old, &installed)?;
    let mut ended = None;
    for _ in 0..240 {
        if let Some(status) = refused.try_wait()? {
            ended = Some(status);
            break;
        }
        pause().await;
    }
    if ended.is_none() {
        refused.kill()?;
        refused.wait()?;
    }
    let said = std::fs::read_to_string(old.join("stdout.log")).unwrap_or_default();
    let tail: String = said.lines().rev().take(3).collect::<Vec<_>>().join(" | ");
    record(
        "the earlier release installed again stopped where it opens the moved World's database",
        ended.is_some_and(|status| !status.success()) && said.contains("book_deeds_began"),
        json!(tail),
    );

    replace(&installed, this)?;
    let mut node = start_with(old, &installed)?;
    let served = online(old).await;
    let again = served && history(world, resident, "old-pc").await? == after;
    record("and this release served it again as before", again, json!(null));
    node.kill()?;
    node.wait()?;
    Ok(())
}

/// Puts `program` where the World runs its node from.
fn replace(installed: &Path, program: &Path) -> Result<()> {
    match std::fs::remove_file(installed) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    std::fs::copy(program, installed)?;
    Ok(())
}

/// The conversation's history on `machine`, each event as it reads without its time.
async fn history(world: &World, id: &ResidentId, machine: &str) -> Result<Vec<Value>> {
    let mut watch = world.watch(id.clone(), Some(machine.parse()?)).await?;
    let events = tokio::time::timeout(Duration::from_secs(40), async {
        while let Some(event) = watch.next().await? {
            if let Watched::History { events } = event {
                return Ok(events);
            }
        }
        anyhow::bail!("the history stream ended before its snapshot")
    })
    .await??;
    Ok(events.into_iter().map(|event| serde_json::to_value(event.happened)).collect::<Result<_, _>>()?)
}

/// The facts of the last run the conversation on `machine` finished, as its
/// history shows them.
async fn last_run(world: &World, id: &ResidentId, machine: &str) -> Result<Option<RunFacts>> {
    let mut watch = world.watch(id.clone(), Some(machine.parse()?)).await?;
    let events = tokio::time::timeout(Duration::from_secs(40), async {
        while let Some(event) = watch.next().await? {
            if let Watched::History { events } = event {
                return Ok(events);
            }
        }
        anyhow::bail!("the history stream ended before its snapshot")
    })
    .await??;
    Ok(events.into_iter().rev().find_map(|event| match event.happened {
        Happened::Finished { item, .. } => match item.what {
            What::Tool(tool) => tool.run.map(|run| *run),
            _ => None,
        },
        _ => None,
    }))
}

/// Records whether `facts` hold the steps of the run's job on `executor`, a
/// script of one `on` running there whole: a file looked at and an `on` to
/// `nested`, each once and ended, the latter with the output it said.
fn steps(facts: Option<&RunFacts>, executor: &str, nested: &str, tag: &str, what: &str) {
    let Some(facts) = facts else {
        record(what, false, json!("no run facts"));
        return;
    };
    let said = format!("{tag}_NESTED");
    let nested_there = facts
        .steps
        .iter()
        .filter(|step| {
            step.ended.is_some()
                && step.machine.as_str() == executor
                && matches!(&step.did, StepDid::On { to, tail: Some(tail), .. }
                    if to.as_str() == nested && tail.contains(&said))
        })
        .count();
    let looked = facts
        .steps
        .iter()
        .filter(|step| {
            step.ended.is_some()
                && step.machine.as_str() == executor
                && matches!(&step.did, StepDid::View { paths, .. } if paths.iter().any(|path| path.ends_with("os-release")))
        })
        .count();
    let ids: std::collections::BTreeSet<_> = facts.steps.iter().map(|step| &step.id).collect();
    let elsewhere = facts.steps.iter().filter(|step| step.machine.as_str() != executor).count();
    record(
        what,
        nested_there == 1 && looked == 1 && elsewhere == 0 && ids.len() == facts.steps.len(),
        json!({"nested": nested_there, "looked": looked, "elsewhere": elsewhere, "steps": facts.steps.len(), "ids": ids.len()}),
    );
}
