//! A delegated conversation keeps a plan of its own, and it and the conversation that asked it
//! read each other's. The asker's collaboration, followed from a phone, tells the colleague as it
//! is asked and as it settles.
use super::*;
use fastctx_types::{
    Address, Call, ResidentId,
    collab::{AgentLogin, Callee, Collab},
    collaboration::Tree,
    residents::Activity,
};

pub(super) async fn exercise(
    world: &World,
    host: &Path,
    working: &Path,
    parent: &ResidentId,
    _home: &Path,
) -> Result<ResidentId> {
    let login = AgentLogin { machine: machine("host"), agent: "codex".parse()?, home: None };
    let before = world.collab_read(None, Some(name("alice"))).await?;
    world
        .collab_write(
            None,
            Some(name("alice")),
            Collab {
                version: before.collab.version,
                on: true,
                callers: vec![login.clone()],
                callees: vec![Callee {
                    name: "helper".into(),
                    use_when: "placement drill".into(),
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
    let mut configured = false;
    for _ in 0..240 {
        let machines =
            ask(host, json!({"call":"machines","resident":parent,"login":null,"thread":null,"scratch":false})).await?;
        if machines.to_string().contains("helper") {
            configured = true;
            break;
        }
        pause().await;
    }
    anyhow::ensure!(configured, "the collaboration configuration did not reach its host");
    let (mut following, first) = world.watch_collaboration(parent.clone(), Some("host".parse()?)).await?;
    record(
        "a conversation's collaboration is followed from a phone, from its own branch",
        first.conversation.address.resident == *parent,
        json!(first.colleagues.iter().map(|branch| branch.address.clone()).collect::<Vec<_>>()),
    );
    // The colleague reads the requester's plan from a job on another machine,
    // so the request travels back to its home with the plan it names.
    let read = working.join("colleague-read.txt");
    let prompt =
        format!("COLLEAGUE_CASE [[run: on alice-two <<'READ'\nfastctx plan {parent} > '{}'\nREAD]]", read.display());
    let reply=ask(host,json!({"call":"ask","resident":parent,"label":"placement colleague","prompt":prompt,"callee":"helper","login":null})).await?;
    let address: Address = serde_json::from_value(reply["colleague"].clone()).context("colleague address")?;
    let told = told_until(&mut following, |tree| tree.colleagues.iter().any(|branch| branch.address == address)).await;
    record(
        "the followed collaboration tells the colleague it asked",
        told.is_some(),
        json!(told.map(|tree| tree.colleagues.iter().map(|branch| branch.address.clone()).collect::<Vec<_>>())),
    );
    anyhow::ensure!(idle_after(host, address.resident.as_str(), None).await, "the colleague did not settle");
    let settled = told_until(&mut following, |tree| {
        tree.colleagues.iter().any(|branch| {
            branch.address == address && branch.now.as_ref().is_some_and(|now| now.activity == Activity::Idle)
        })
    })
    .await;
    record(
        "and tells the colleague settled once it is",
        settled.is_some(),
        json!(settled.map(|tree| serde_json::to_value(&tree.colleagues).unwrap_or_default())),
    );
    drop(following);
    let path = plan_file(host, address.resident.as_str())?;
    record("a colleague inherits the requester's usable working computer", path.starts_with(working), json!(path));
    let guide = block(host, address.resident.as_str(), "plan-guide");
    record(
        "a colleague is told a plan file of its own, which the conversation that asked it reads",
        guide.contains("YOUR PLAN") && guide.contains(&format!("{parent}, the conversation that asked you")),
        json!(guide.chars().take(400).collect::<String>()),
    );
    record(
        "the colleague's plan context stores no requester body on the host",
        !block(host, address.resident.as_str(), "plan").contains("OFFLINE_EDIT"),
        json!(null),
    );
    let said = std::fs::read_to_string(&read).unwrap_or_default();
    record(
        "the colleague reads the requester's plan from a job on another machine",
        said.starts_with(&format!("The plan of {parent}, version")) && said.contains("OFFLINE_EDIT"),
        json!(said),
    );
    // Its first plan, written as a model writes it: from a job on its working
    // computer, before any file is there.
    let before = last_turn(host, address.resident.as_str());
    world
        .talk(
            address.resident.clone(),
            Some("host".parse()?),
            format!(
                "COLLEAGUE_OWN_PLAN [[run: on alice-two <<'WRITE'\nwrite '{}' <<'TEXT'\n# Part\nCOLLEAGUE_OWN_PLAN\nTEXT\nWRITE]]",
                path.display()
            ),
        )
        .await?;
    anyhow::ensure!(
        idle_after(host, address.resident.as_str(), before).await,
        "the colleague's plan write did not settle"
    );
    record(
        "the colleague writes its own plan",
        plan_is(world, &address.resident, "# Part\nCOLLEAGUE_OWN_PLAN\n").await,
        json!(null),
    );
    let mut client = fastctx_client::Client::connect(
        &WorldDir::under(host),
        Peer::Command { resident: Some(Address { machine: machine("host"), resident: parent.clone() }), session: None },
    )
    .await?;
    let of = fastctx_types::plan::Of { resident: address.resident.clone(), machine: None };
    let asked = client
        .call(Call::Plan { hub: None, account: None, operation: fastctx_types::plan::Operation::Read, of: Some(of) })
        .await;
    let read_back = match &asked {
        Ok(fastctx_types::Reply::Plan { result: fastctx_types::plan::Result::Page { page } }) => {
            page.body.as_ref().map(|body| body.markdown.clone())
        }
        _ => None,
    };
    record(
        "the conversation that asked reads its colleague's plan",
        read_back.as_deref() == Some("# Part\nCOLLEAGUE_OWN_PLAN\n"),
        json!(format!("{asked:?}").chars().take(400).collect::<String>()),
    );
    Ok(address.resident)
}

/// The first tree the followed collaboration tells that `wanted` holds of,
/// within two minutes.
async fn told_until(
    following: &mut fastctx_client::CollaborationWatch,
    wanted: impl Fn(&Tree) -> bool,
) -> Option<Tree> {
    let waiting = async {
        while let Ok(Some(tree)) = following.next().await {
            if wanted(&tree) {
                return Some(tree);
            }
        }
        None
    };
    tokio::time::timeout(std::time::Duration::from_secs(120), waiting).await.ok().flatten()
}
