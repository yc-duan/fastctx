//! A device's copy of what it may hold, with no interface (design §29 3.6):
//! logs a device in (or takes up the logins its directory has), keeps a copy
//! as a light device does (`--whole` to hold everything), waits until its
//! passes settle, and prints what the copy holds as JSON, one line a stream.
//! The vertical slice runs it as its third kind of member.
//!
//! ```text
//! replica <device directory> [--hub <host:port> --account <name> --name <device name>]
//!         [--whole] [--seconds <n>] [--older <scope> <stream>]... [--ask <scope> <stream>]...
//! ```
//!
//! The password is read from `FASTCTX_PASSWORD`. `--older` pages a stream
//! down from the hub to its first item before it is printed. `--ask` asks
//! the hub for a stream's top page as soon as the device links to it,
//! before anything settles, and prints what the hub answered.

use fastctx_client::device::Device;
use fastctx_client::device::replica::{OlderTaken, Tier};
use std::path::PathBuf;
use std::time::{Duration, Instant};

struct Asked {
    directory: PathBuf,
    hub: Option<String>,
    account: Option<String>,
    name: Option<String>,
    whole: bool,
    seconds: u64,
    older: Vec<(String, String)>,
    ask: Vec<(String, String)>,
}

fn asked() -> Result<Asked, String> {
    let mut args = std::env::args().skip(1);
    let directory = PathBuf::from(args.next().ok_or("the device directory comes first")?);
    let mut asked =
        Asked { directory, hub: None, account: None, name: None, whole: false, seconds: 120, older: Vec::new(), ask: Vec::new() };
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} takes a value"));
        match arg.as_str() {
            "--hub" => asked.hub = Some(value()?),
            "--account" => asked.account = Some(value()?),
            "--name" => asked.name = Some(value()?),
            "--seconds" => asked.seconds = value()?.parse().map_err(|_| "--seconds takes a number")?,
            "--whole" => asked.whole = true,
            "--older" => {
                let scope = value()?;
                let stream = args.next().ok_or("--older takes a scope and a stream")?;
                asked.older.push((scope, stream));
            }
            "--ask" => {
                let scope = value()?;
                let stream = args.next().ok_or("--ask takes a scope and a stream")?;
                asked.ask.push((scope, stream));
            }
            other => return Err(format!("not understood: {other}")),
        }
    }
    Ok(asked)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).chars().take(300).collect()
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(why) = run().await {
        println!("{}", serde_json::json!({ "error": why }));
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let asked = asked()?;
    let error = |error: std::io::Error| error.to_string();
    let cache = fastctx_client::cache::Cache::open(asked.directory.join("cache")).map_err(error)?;
    let device = Device::open(asked.directory.join("device"), cache).map_err(error)?;
    if let (Some(hub), Some(account)) = (&asked.hub, &asked.account) {
        let password = std::env::var("FASTCTX_PASSWORD").map_err(|_| "FASTCTX_PASSWORD holds the password")?;
        let addresses = tokio::net::lookup_host(hub.as_str()).await.map_err(error)?.collect();
        let account = fastctx_types::AccountName::try_from(account.clone()).map_err(|error| error.to_string())?;
        let name = asked.name.clone().map(fastctx_types::MachineName::try_from).transpose().map_err(|error| error.to_string())?;
        // Logged in as asked: unless a network the device is on has that
        // account logged in already (codex-058517ed #25).
        if !device.state().hubs.iter().any(|hub| hub.logins.contains(&account)) {
            device.login(addresses, account, password, name).await.map_err(|error| error.to_string())?;
        }
    }
    let tier = if asked.whole { Tier::Whole } else { Tier::Light };
    let replica = {
        let device = device.clone();
        tokio::task::spawn_blocking(move || device.replicate(tier)).await.map_err(|error| error.to_string())?.map_err(error)?
    };
    // Asked at once, as the first thing the device asks of a scope: the top
    // page, as soon as there is a link to its hub; what the hub answered.
    for (scope, stream) in &asked.ask {
        let hub = hub_id(&replica, scope).ok_or(format!("{scope} is not held here"))?;
        let linked_by = Instant::now() + Duration::from_secs(60);
        let answer = loop {
            match replica.older(&hub, scope, stream, None, 50).await {
                Err(why) if why.starts_with("no link to") && Instant::now() < linked_by => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Ok(taken) => break format!("{taken:?}"),
                Err(why) => break why,
            }
        };
        println!("{}", serde_json::json!({ "asked": scope, "stream": stream, "answer": answer }));
    }
    // Settled: two passes in a row ended well, with every hub's last pass
    // well and nothing new held but the running logs: a machine's, an MCP
    // server's and the hub's own grow by the minute while they run, and a
    // quiet follower passes once a minute, so they would never hold still.
    let deadline = Instant::now() + Duration::from_secs(asked.seconds);
    let mut passes = replica.passes();
    let mut settled = 0;
    let mut held_before = std::collections::BTreeMap::new();
    // What each pass seen changed, the last few, for a copy that does not
    // settle: how many passes, and which streams moved in them.
    let started = Instant::now();
    let mut seen = Vec::new();
    while settled < 2 && Instant::now() < deadline {
        if tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), passes.changed()).await.is_err() {
            break;
        }
        let library = replica.library();
        let mut held = std::collections::BTreeMap::new();
        for (scope, _, _) in library.live_scopes().map_err(|error| error.to_string())? {
            for (stream, next) in library.streams(&scope).map_err(|error| error.to_string())? {
                if !matches!(stream.as_str(), "node" | "mcp" | "hub") {
                    held.insert(format!("{scope} {stream}"), next);
                }
            }
        }
        let on = hubs(&replica);
        // Every pass ended well, and nothing its tier fetches as records
        // come is missing: a pass may end with objects still to fetch.
        let lacking = on.iter().try_fold(0, |sum, hub| {
            library.lacking_within(hub.as_str(), tier.fetches_at_most()).map(|(count, _)| sum + count)
        });
        let well = !on.is_empty()
            && on.iter().all(|hub| matches!(replica.ended(hub), Some(Ok(()))))
            && lacking.as_ref().is_ok_and(|count| *count == 0);
        let moved: Vec<String> = held
            .iter()
            .filter(|(stream, next)| held_before.get(*stream) != Some(*next))
            .map(|(stream, next)| format!("{stream}={next}"))
            .chain(held_before.keys().filter(|stream| !held.contains_key(*stream)).map(|stream| format!("{stream} gone")))
            .take(6)
            .collect();
        settled = if well && moved.is_empty() { settled + 1 } else { 0 };
        seen.push(serde_json::json!({ "at": started.elapsed().as_secs(), "well": well, "moved": moved }));
        held_before = held;
    }
    // Pages down what was asked, from the hub, as a reader scrolling would.
    for (scope, stream) in &asked.older {
        let hub = hub_id(&replica, scope).ok_or(format!("{scope} is not held here"))?;
        // Down from the top, along what is covered without a gap, a page
        // below where that ends, until it reaches below everything.
        let mut pages = 0;
        while let Some(before) = uncovered(&replica, scope, stream)? {
            match replica.older(&hub, scope, stream, before, 200).await? {
                OlderTaken::Taken => {}
                taken => return Err(format!("the page of {stream} of {scope} was not taken: {taken:?}")),
            }
            pages += 1;
            if pages > 10_000 {
                return Err(format!("{stream} of {scope} does not come whole"));
            }
        }
    }
    let library = replica.library();
    for (scope, _, _) in library.live_scopes().map_err(|error| error.to_string())? {
        for (stream, next) in library.streams(&scope).map_err(|error| error.to_string())? {
            let page = library
                .view_page(&scope, &stream, &fastctx_content::Anchor::Latest, 50, 1 << 20)
                .map_err(|error| error.to_string())?;
            let coverage = library.coverage(&scope, &stream).map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::json!({
                    "scope": scope,
                    "stream": stream,
                    "next": next,
                    "revision": page.revision,
                    "items": page.entries.iter().map(|entry| serde_json::json!({
                        "item": text(&entry.item),
                        "revision": entry.revision,
                        "kind": entry.kind,
                        "body": text(&entry.body),
                    })).collect::<Vec<_>>(),
                    "older": page.older.is_some(),
                    "covered": coverage.iter().map(|run| serde_json::json!({
                        "whole_below": run.low.0.is_empty() && run.low.1.is_empty(),
                        "up_to_top": run.high.is_none(),
                        "through": run.through,
                    })).collect::<Vec<_>>(),
                })
            );
        }
    }
    let ended: Vec<_> = hubs(&replica)
        .into_iter()
        .map(|hub| serde_json::json!({ "hub": hub.to_string(), "ended": format!("{:?}", replica.ended(&hub)) }))
        .collect();
    println!("{}", serde_json::json!({ "settled": settled >= 2, "tier": format!("{tier:?}"), "passes": ended }));
    if settled < 2 {
        let last: Vec<_> = seen.iter().rev().take(8).rev().collect();
        println!("{}", serde_json::json!({ "passes_seen": seen.len(), "last": last }));
        return Err("the copy did not settle in time".into());
    }
    Ok(())
}

/// The hubs the copy holds scopes of, as their descriptors name them: a
/// pass ends on each of them before the copy counts as settled.
fn hubs(replica: &fastctx_client::device::replica::Replica) -> Vec<fastctx_net::ids::HubId> {
    let mut found: Vec<fastctx_net::ids::HubId> = Vec::new();
    for (scope, _, _) in replica.library().live_scopes().unwrap_or_default() {
        if let Some(hub) = hub_id(replica, &scope)
            && !found.iter().any(|known| known.as_str() == hub.as_str())
        {
            found.push(hub);
        }
    }
    found
}

/// An item's order key and identity.
type Key = (Vec<u8>, Vec<u8>);

/// Where what is covered of `stream` of `scope` from its top down ends, a
/// page to be asked below it; none once it reaches below everything.
fn uncovered(replica: &fastctx_client::device::replica::Replica, scope: &str, stream: &str) -> Result<Option<Option<Key>>, String> {
    let runs = replica.library().coverage(scope, stream).map_err(|error| error.to_string())?;
    // From the top: the run reaching above everything, then each run that
    // ends where the one above it begins.
    let Some(mut low) = runs.iter().find(|run| run.high.is_none()).map(|run| run.low.clone()) else {
        return Ok(Some(None));
    };
    loop {
        if low.0.is_empty() && low.1.is_empty() {
            return Ok(None);
        }
        match runs.iter().find(|run| run.high.as_ref() == Some(&low)) {
            Some(run) => low = run.low.clone(),
            None => return Ok(Some(Some(low))),
        }
    }
}

/// The hub the scope `scope` of the copy is of, as its descriptor names it.
fn hub_id(replica: &fastctx_client::device::replica::Replica, scope: &str) -> Option<fastctx_net::ids::HubId> {
    let (_, descriptor, _) = replica.library().live_scopes().ok()?.into_iter().find(|(key, _, _)| key == scope)?;
    let read: serde_json::Value = serde_json::from_slice(&descriptor).ok()?;
    fastctx_net::ids::HubId::try_from(read["hub"].as_str()?.to_string()).ok()
}
