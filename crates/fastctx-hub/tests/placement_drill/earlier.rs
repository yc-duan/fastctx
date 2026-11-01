//! A machine of the earlier release among this release's, as a network has
//! it while its machines update one by one. A conversation of this release
//! runs a script on it, and a conversation that lives on it runs one on a
//! machine of this release: each sees every step of the part run on the
//! other side once, ended, with its output. A project of the account spans
//! it and a machine of this release, its ledger holding history: both take
//! what the other writes. Then it is updated in place.
use super::*;
use fastctx_types::history::{HistoryCall, HistoryReply, Scope};
use fastctx_types::project::{RecordFields, record_path};
use fastctx_types::run_facts::{RunFacts, StepDid};
use fastctx_types::session::{Happened, Watched, What};
use fastctx_types::{AgentName, ResidentId, TownName};

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
    let town = project(base, world, &old).await?;
    let before = history(world, &resident, "old-pc").await?;
    old_node.kill()?;
    old_node.wait()?;
    upgrade(&old, &base.join("alice-pc"), world, &resident, &town, earlier, before).await
}

/// A project of the account spanning the earlier machine and a machine of
/// this release, history appended to its ledger: the earlier machine takes
/// the entries written after the history, and writes one of its own that
/// this release's machine takes.
async fn project(base: &Path, world: &World, old: &Path) -> Result<TownName> {
    let town: TownName = "mixed-history".parse()?;
    world
        .open_town(
            town.clone(),
            RecordFields {
                name: Some("Mixed history".into()),
                what: Some("BEFORE_HISTORY".into()),
                machines: Some(vec![machine("old-pc"), machine("alice-pc")]),
                ..Default::default()
            },
            Some(name("alice")),
            None,
        )
        .await?;
    let saved = world
        .history(HistoryCall::Save {
            scope: Scope::Project { project: town.clone() },
            name: "mixed-note".into(),
            text: "---\nname: mixed-note\ndescription: Mixed fixture\n---\nMIXED_HISTORY\n".into(),
            expected: None,
        })
        .await?;
    anyhow::ensure!(matches!(saved, HistoryReply::Saved { .. }), "saving the project's history did not finish");
    world
        .edit_town(Some(town.clone()), RecordFields { what: Some("AFTER_HISTORY".into()), ..Default::default() })
        .await?;
    let version = |root: &Path, version: u32| WorldDir::under(root).town(&town).ledger().join(record_path(version));
    record(
        "an earlier machine took a project's entry written after history in its ledger",
        holds(&version(old, 2), "AFTER_HISTORY").await,
        json!(null),
    );

    let edited = command(old, &old.join("bin/fastctx"))
        .args(["project", "edit", town.as_str(), "--what", "EARLIER_EDIT"])
        .output()?;
    let said = format!("{}{}", String::from_utf8_lossy(&edited.stdout), String::from_utf8_lossy(&edited.stderr));
    let alice = base.join("alice-pc");
    record(
        "an earlier machine wrote to a project holding history, and this release's machine took it",
        edited.status.success() && holds(&version(&alice, 3), "EARLIER_EDIT").await,
        json!(said.trim()),
    );
    record(
        "this release's machine reads the history of a project an earlier machine writes to",
        holds(&projected(&alice, &town), "MIXED_HISTORY").await,
        json!(null),
    );
    Ok(town)
}

/// Where a machine at `root` keeps the project history document the drill
/// saves in `town`, for its residents to read.
fn projected(root: &Path, town: &TownName) -> PathBuf {
    WorldDir::under(root).town(town).path().join("history").join("mixed-note.md")
}

/// Waits for the file at `path` to hold `text`.
async fn holds(path: &Path, text: &str) -> bool {
    for _ in 0..240 {
        if std::fs::read_to_string(path).is_ok_and(|read| read.contains(text)) {
            return true;
        }
        pause().await;
    }
    false
}

/// The earlier machine updated in place, as its user installing this release
/// does: its World moves forward though killed along the way, answers as it
/// did, and the earlier release installed again stops instead of misreading
/// it. `peer` is another machine of its account, which sees it come back;
/// `before` its conversation's history as the earlier release read it;
/// `town` a project whose history that release kept as entries only.
async fn upgrade(
    old: &Path,
    peer: &Path,
    world: &World,
    resident: &ResidentId,
    town: &TownName,
    earlier: &Path,
    before: Vec<Value>,
) -> Result<()> {
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
    let (served, seen) = serving(old, peer).await;
    record("this release served the earlier machine's World, killed three times along its move", served, seen);
    let after = history_again(world, resident, "old-pc").await?;
    let missing = lost(&recorded(&before), &recorded(&after));
    record(
        "the earlier conversation's history lost and changed nothing in the move",
        missing.is_null(),
        json!({"lost": missing, "added": added(&before, &after)}),
    );
    record(
        "and its project's history, which the earlier release took as entries only, is there to read",
        holds(&projected(old, town), "MIXED_HISTORY").await,
        json!(null),
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
    let (served, seen) = serving(old, peer).await;
    let again = if served { history_again(world, resident, "old-pc").await? } else { Vec::new() };
    record(
        "and this release served it again as before",
        served && again == after,
        if served {
            json!({"lost": lost(&recorded(&after), &recorded(&again)), "added": added(&after, &again)})
        } else {
            seen
        },
    );
    node.kill()?;
    node.wait()?;
    Ok(())
}

/// Waits for the node at `old` to answer with its hub's view, and for the
/// machine at `peer` to see it online through their hub; says what it saw
/// last when either does not come within a minute.
async fn serving(old: &Path, peer: &Path) -> (bool, Value) {
    let began = std::time::Instant::now();
    let mut last = json!(null);
    while began.elapsed() < Duration::from_secs(60) {
        match ask(old, json!({"call": "nodes"})).await {
            Ok(answer) if answer.to_string().contains("\"online\"") => {
                if online_now(peer, "old-pc").await {
                    return (true, json!({"seconds": began.elapsed().as_secs_f64()}));
                }
                last = json!("its peer does not see old-pc online");
            }
            Ok(answer) => {
                last =
                    json!({"answered without the hub's view": answer.to_string().chars().take(300).collect::<String>()})
            }
            Err(error) => last = json!({"failed": error.to_string()}),
        }
        pause().await;
    }
    (false, last)
}

/// A history as it is kept: the host's thread object a Codex conversation's
/// history carries is what its app-server says as it is read, live state
/// such as whether it takes input now, kept by neither release.
fn recorded(events: &[Value]) -> Vec<Value> {
    events
        .iter()
        .cloned()
        .map(|mut event| {
            if let Some(change) = event.get_mut("change").and_then(Value::as_object_mut) {
                change.remove("metadata");
            }
            event
        })
        .collect()
}

/// Where `after` first lost or changed something `before` said, if it did:
/// a field `before` gives the same in `after`, a list as long, each item
/// alike. What `after` says that `before` left unsaid is not a loss.
fn lost(before: &[Value], after: &[Value]) -> Value {
    fn at(a: &Value, b: &Value, path: String) -> Value {
        match (a, b) {
            (Value::Null, _) => json!(null),
            (Value::Object(a), Value::Object(b)) => {
                for (key, value) in a {
                    let found = at(value, b.get(key).unwrap_or(&Value::Null), format!("{path}.{key}"));
                    if !found.is_null() {
                        return found;
                    }
                }
                json!(null)
            }
            (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
                for (index, (x, y)) in a.iter().zip(b).enumerate() {
                    let found = at(x, y, format!("{path}[{index}]"));
                    if !found.is_null() {
                        return found;
                    }
                }
                json!(null)
            }
            _ if a == b => json!(null),
            _ => {
                let cut = |value: &Value| value.to_string().chars().take(300).collect::<String>();
                json!({"at": path, "before": cut(a), "after": cut(b)})
            }
        }
    }
    if before.len() != after.len() {
        return json!({"events before": before.len(), "after": after.len()});
    }
    at(&Value::Array(before.to_vec()), &Value::Array(after.to_vec()), String::new())
}

/// Where `after` says more than `before` did, as paths.
fn added(before: &[Value], after: &[Value]) -> Value {
    fn at(a: &Value, b: &Value, path: String, found: &mut Vec<String>) {
        match (a, b) {
            (Value::Null, Value::Null) => {}
            (Value::Null, _) => found.push(path),
            (Value::Object(a), Value::Object(b)) => {
                for (key, value) in b {
                    at(a.get(key).unwrap_or(&Value::Null), value, format!("{path}.{key}"), found);
                }
            }
            (Value::Array(a), Value::Array(b)) if a.len() == b.len() => {
                for (index, (x, y)) in a.iter().zip(b).enumerate() {
                    at(x, y, format!("{path}[{index}]"), found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for (index, (a, b)) in before.iter().zip(after).enumerate() {
        at(a, b, format!("[{index}]"), &mut found);
    }
    json!(found)
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

/// The history on `machine` once it answers again after a restart: until
/// its hub has its session, a watch there is refused as offline.
async fn history_again(world: &World, id: &ResidentId, machine: &str) -> Result<Vec<Value>> {
    let began = std::time::Instant::now();
    loop {
        match history(world, id, machine).await {
            Ok(events) => return Ok(events),
            Err(error) if began.elapsed() > Duration::from_secs(60) => return Err(error),
            Err(_) => pause().await,
        }
    }
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
