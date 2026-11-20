//! What a conversation is born as (design-forget-account-v2.md F0): whom it
//! works for, by which assistance of its login where it does not hold the
//! machine, and in which login, known by its id. An assistance keeps its
//! grant as what it allows is changed; one ended and given again is another.
use super::*;

/// The conversation `resident`'s `about` record on `host`, as it is kept.
pub(super) fn about(host: &Path, resident: &str) -> Value {
    std::fs::read(host.join("world/conversations").join(resident).join("about"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

/// The grant the conversation was born working by, for its one account,
/// where it was born working by an assistance.
pub(super) fn grant(about: &Value) -> Option<String> {
    about["born_as"][0]["grant"].as_str().filter(|_| about["born_as"][0]["assisted"] == true).map(str::to_string)
}

/// Each of `assisted` was born for its own account by an assistance, and
/// `owned` for its owner by none; all in the one login, known by its id.
pub(super) fn check(host: &Path, assisted: &[(&str, &str)], owned: (&str, &str)) {
    let born = |(resident, account): (&str, &str)| {
        let about = about(host, resident);
        let one = about["born_as"].as_array().is_some_and(|accounts| {
            accounts.len() == 1 && accounts[0]["account"] == account && accounts[0]["generation"].is_string()
        });
        (about, one)
    };
    let (owner, owner_one) = born(owned);
    let login = owner["login"].as_str().filter(|login| login.starts_with("l-")).map(str::to_string);
    let mut grants = Vec::new();
    let mut ok = owner_one && login.is_some() && grant(&owner).is_none();
    let mut detail = vec![owner["born_as"].clone()];
    for &(resident, account) in assisted {
        let (about, one) = born((resident, account));
        let given = grant(&about);
        ok &= one && given.is_some() && about["login"].as_str() == login.as_deref() && !grants.contains(&given);
        grants.push(given);
        detail.push(about["born_as"].clone());
    }
    record(
        "each conversation is born knowing whom it works for, by which assistance, in which login",
        ok,
        json!({ "login": login, "born_as": detail }),
    );
}

/// An account `carol`, assisted by `agent`'s login `home` on `host`: three
/// conversations, before and after what the assistance allows is changed,
/// and after it is ended and given again.
pub(super) async fn exercise(
    base: &Path,
    owner: &World,
    listen: SocketAddr,
    host: &Path,
    agent: &AgentName,
    home: &Path,
) -> Result<()> {
    owner
        .account(
            None,
            name("owner"),
            AccountChange::OpenAccount { name: name("carol"), password: PASS.into(), visitor: true },
        )
        .await?;
    let (carol_device, carol) = device(base, "carol-phone", "carol", listen).await?;
    let assist = |assisting: bool, turns: Option<u16>| AccountChange::Assist {
        machine: machine("host"),
        to: name("carol"),
        agent: agent.clone(),
        home: Some(home.to_path_buf()),
        assisting,
        until: None,
        weekly: None,
        daily: None,
        five_hour: None,
        turns,
        share: None,
        manages: None,
        purge: false,
        shown: Vec::new(),
    };
    let choices = fastctx_types::agents::Choices { home: Some(home.to_path_buf()), ..Default::default() };
    let open = |words: &'static str| {
        let (carol, agent, choices) = (&carol, agent.clone(), choices.clone());
        async move {
            let opened = carol
                .open_with(agent, None, Some("host".parse()?), words.into(), Default::default(), choices)
                .await
                .with_context(|| format!("opening {words}"))?;
            let resident = opened.to.resident.to_string();
            anyhow::ensure!(idle_after(host, &resident, None).await, "{words} did not settle");
            Ok::<_, anyhow::Error>(grant(&about(host, &resident)))
        }
    };
    owner.account(None, name("owner"), assist(true, None)).await.context("assisting carol")?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let first = open("CAROL_FIRST").await?;
    owner.account(None, name("owner"), assist(true, Some(4))).await.context("changing carol's assistance")?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let changed = open("CAROL_CHANGED").await?;
    owner.account(None, name("owner"), assist(false, None)).await.context("ending carol's assistance")?;
    owner.account(None, name("owner"), assist(true, None)).await.context("assisting carol again")?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let again = open("CAROL_AGAIN").await?;
    record(
        "an assistance keeps its grant as what it allows changes, and one ended and given again is another",
        first.is_some() && first == changed && again.is_some() && again != first,
        json!([first, changed, again]),
    );
    carol_device.disconnect();
    Ok(())
}
