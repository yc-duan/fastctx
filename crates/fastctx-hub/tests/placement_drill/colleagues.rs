//! A real delegated conversation reads the requester's plan without a writable copy.
use super::*;
use fastctx_types::{
    Address, Call, ResidentId,
    collab::{AgentLogin, Callee, Collab},
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
    let reply=ask(host,json!({"call":"ask","resident":parent,"label":"placement colleague","prompt":"COLLEAGUE_CASE [[run: on alice-two pwd]]","callee":"helper","login":null})).await?;
    let address: Address = serde_json::from_value(reply["colleague"].clone()).context("colleague address")?;
    anyhow::ensure!(idle(host, address.resident.as_str()).await, "the colleague did not settle");
    let path = plan_file(host, address.resident.as_str())?;
    record("a colleague inherits the requester's usable working computer", path.starts_with(working), json!(path));
    record(
        "a colleague is given no writable plan copy",
        !path.exists() && block(host, address.resident.as_str(), "plan-guide").is_empty(),
        json!(null),
    );
    record(
        "the colleague's plan context stores no requester body on the host",
        !block(host, address.resident.as_str(), "plan").contains("OFFLINE_EDIT"),
        json!(null),
    );
    let mut client = fastctx_client::Client::connect(
        &WorldDir::under(host),
        Peer::Command { resident: Some(address.clone()), session: None },
    )
    .await?;
    let written = client
        .call(Call::Plan {
            hub: None,
            account: None,
            operation: fastctx_types::plan::Operation::Write { markdown: "UNAUTHORIZED_PLAN".into() },
        })
        .await;
    record("the colleague cannot write the requester's plan", written.is_err(), json!(null));
    Ok(address.resident)
}
