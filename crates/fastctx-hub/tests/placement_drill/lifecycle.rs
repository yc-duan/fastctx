//! Record ownership survives a manual conversation move and ends with true deletion.
use super::*;
use fastctx_types::{
    Address, ResidentId,
    history::{HistoryCall, HistoryReply, Scope},
};

pub(super) fn seed(root: &Path, machine_name: &str, id: &ResidentId, home: Option<&Path>) -> Result<PathBuf> {
    let directory = root.join("world/conversations").join(id.as_str());
    std::fs::create_dir_all(&directory)?;
    let about: fastctx_types::residents::About = serde_json::from_value(json!({
        "id":id, "host":{"agent":"codex","client":"record-lifecycle-drill","version":"0"},
        "machine":machine_name,"born_at":jiff::Timestamp::now(),"origin":{"kind":"user"},"session":null,
        "home":home
    }))?;
    std::fs::write(directory.join("about"), serde_json::to_vec(&about)?)?;
    Ok(directory)
}

fn scope(id: &ResidentId, place: &str) -> Scope {
    Scope::Conversation { resident: id.clone(), machine: machine(place) }
}
async fn ready(world: &World, id: &ResidentId, place: &str) -> Result<()> {
    for _ in 0..240 {
        if world.history(HistoryCall::List { scope: scope(id, place) }).await.is_ok() {
            return Ok(());
        }
        pause().await;
    }
    anyhow::bail!("conversation registration did not settle on {place}")
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn exercise(
    base: &Path,
    world: &World,
    source: &Path,
    destination: &Path,
    source_node: &mut Child,
    destination_node: &mut Child,
    listen: SocketAddr,
    mock_configuration: &[u8],
) -> Result<()> {
    // The moved conversation writes notes, which a login offers only with
    // the workbench open in it.
    let login = base.join("alice-two-codex");
    std::fs::create_dir_all(&login)?;
    std::fs::write(login.join("config.toml"), mock_configuration)?;
    world.add_agent_login("alice-two".parse()?, "codex".parse()?, login.clone()).await?;
    let opened = world
        .change_agent_login("alice-two".parse()?, "codex".parse()?, login.clone(), true, true, true, None, Some(true))
        .await?;
    anyhow::ensure!(
        matches!(opened.end, Some(fastctx_types::logins::ChangeEnd::Done { .. })),
        "opening the workbench in alice-two's login failed: {opened:?}"
    );
    source_node.kill()?;
    source_node.wait()?;
    let id: ResidentId = "codex-1234abcd".parse()?;
    let original = seed(source, "alice-pc", &id, Some(&login))?;
    let birth = std::fs::read(original.join("about"))?;
    *source_node = start(source)?;
    ready(world, &id, "alice-pc").await?;
    let text = "---\nname: moving-record\ndescription: Migration identity fixture\n---\nMOVING_HISTORY\n";
    world
        .history(HistoryCall::Save {
            scope: scope(&id, "alice-pc"),
            name: "moving-record".into(),
            text: text.into(),
            expected: None,
        })
        .await?;
    world.plan_write(None, None, id.clone(), None, "MOVING_PLAN".into(), Some(machine("alice-pc"))).await?;
    anyhow::ensure!(file_is(&original.join("plan.md"), "MOVING_PLAN").await, "the original plan was not copied");
    source_node.kill()?;
    source_node.wait()?;
    destination_node.kill()?;
    destination_node.wait()?;
    let relocated = destination.join("world/conversations").join(id.as_str());
    anyhow::ensure!(!relocated.exists(), "a live destination conversation already exists");
    std::fs::rename(&original, &relocated)?;
    *destination_node = start(destination)?;
    ready(world, &id, "alice-two").await?;
    *source_node = start(source)?;
    let old = world
        .history(HistoryCall::Read { scope: scope(&id, "alice-pc"), name: "moving-record".into(), version: None })
        .await?;
    let current = world
        .history(HistoryCall::Read { scope: scope(&id, "alice-two"), name: "moving-record".into(), version: None })
        .await?;
    record(
        "a moved conversation keeps its birth and both record addresses",
        std::fs::read(relocated.join("about"))? == birth
            && old == current
            && matches!(current, HistoryReply::Document { ref document } if document.text == text && document.revision.version == 1),
        json!(null),
    );
    let owner = Address { machine: machine("alice-two"), resident: id.clone() };
    let changed = text.replace("MOVING_HISTORY", "MOVED_HISTORY_WRITE");
    let local = relocated.join("history/moving-record.md");
    anyhow::ensure!(file_is(&local, text).await, "the moved history was not published");
    saw_as(destination, &owner, &local).await?;
    write_as(destination, &owner, &local, &changed).await?;
    let source_copy = WorldDir::under(source)
        .owner(&fastctx_types::layout::Names { me: machine("alice-pc"), was: Vec::new(), renamed: Vec::new() }, &owner)
        .path()
        .join("history/moving-record.md");
    record(
        "the moved conversation writes the same history and synchronizes back",
        file_is(&source_copy, &changed).await,
        json!(null),
    );
    let (new_root, mut new_node) = node(base, "alice-new", "alice", listen).await?;
    let new_copy = WorldDir::under(&new_root)
        .owner(&fastctx_types::layout::Names { me: machine("alice-new"), was: Vec::new(), renamed: Vec::new() }, &owner)
        .path()
        .join("history/moving-record.md");
    record("a newly joined computer gets existing account history", file_is(&new_copy, &changed).await, json!(null));
    world.forget_conversation(id.clone(), "alice-two".parse()?).await?;
    for _ in 0..240 {
        if !local.exists() && !source_copy.exists() && !new_copy.exists() {
            break;
        }
        pause().await;
    }
    record(
        "true conversation deletion withdraws every history copy and its old versions",
        !local.exists()
            && !source_copy.exists()
            && !new_copy.exists()
            && world
                .history(HistoryCall::Read {
                    scope: scope(&id, "alice-pc"),
                    name: "moving-record".into(),
                    version: Some(1),
                })
                .await
                .is_err(),
        json!(null),
    );
    let _ = new_node.kill();
    let _ = new_node.wait();
    Ok(())
}
