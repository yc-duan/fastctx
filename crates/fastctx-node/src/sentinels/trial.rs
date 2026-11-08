//! Trying the sentinels' model and search once from their home, with the
//! keys kept there and that machine's own way out, so the user learns while
//! still in the settings whether the address, the model's name and the key
//! are right, rather than when a sentinel first wakes (`world-senses.md`,
//! 功能页).

use std::sync::Arc;
use std::time::{Duration, Instant};

use fastctx_agent::model::{Message, Request, client};
use fastctx_net::link::Link;
use fastctx_sentinel::web::{Search as _, Tavily};
use fastctx_types::{
    AccountName,
    access::HubAccount,
    ipc::Failure,
    remote::{RemoteCall, RemoteReply},
    sentinel_settings::{KeyName, Tried, TryReport},
};

use crate::Node;

/// How long the model has to answer a try.
const MODEL_WITHIN: Duration = Duration::from_secs(90);
/// How long the search has to answer a try.
const SEARCH_WITHIN: Duration = Duration::from_secs(30);
/// How much of what came back a try shows.
const SAID: usize = 600;

/// The account's sentinel home tries its model and search: here, or asked of
/// that machine.
pub(crate) async fn tried(node: &Arc<Node>, link: &Arc<Link>, account: &AccountName) -> Result<TryReport, Failure> {
    let scope = HubAccount { hub: link.membership().id.clone(), account: account.clone() };
    let home = super::keys::settings(node, &scope)
        .await?
        .home
        .ok_or_else(|| Failure::refused("choose the sentinel home first"))?;
    let view = crate::authorization::current_view(link)?;
    if view.is_me(&home) {
        return here(node, &scope).await;
    }
    if !view.call_rights(&view.me, account, &home).hands {
        return Err(Failure::refused("this account may not use the sentinel home"));
    }
    if !view.member(&home).is_some_and(|member| member.online) {
        return Err(Failure::not_run("the sentinel home is offline"));
    }
    let call = RemoteCall::SentinelTry;
    match link.call(&home, account, call.purpose(), call).await.map_err(|error| Failure::new(error.to_string()))?? {
        RemoteReply::SentinelTried(report) => Ok(*report),
        _ => Err(Failure::new("the sentinel home answered another kind of call")),
    }
}

/// Tries the account's model and search here, while this machine is its
/// sentinel home; each that is set up, at once.
pub(crate) async fn here(node: &Arc<Node>, scope: &HubAccount) -> Result<TryReport, Failure> {
    let settings = super::keys::settings(node, scope).await?;
    if !super::keys::home_is_here(node, scope, &settings)? {
        return Err(Failure::refused("this machine is not the account's sentinel home"));
    }
    let keys = {
        let (reading, scope) = (node.clone(), scope.clone());
        fastctx_platform::task::blocking(move || {
            Ok::<_, Failure>((
                super::keys::held(&reading, &scope, KeyName::Model)?,
                super::keys::held(&reading, &scope, KeyName::Search)?,
            ))
        })
        .await?
    };
    let model = async {
        let model = settings.model?;
        let Some(key) = keys.0 else { return Some(missing("the model has no key yet")) };
        Some(timed(MODEL_WITHIN, &key, answer(model, key.clone())).await)
    };
    let search = async {
        let search = settings.search?;
        let Some(key) = keys.1 else { return Some(missing("the search has no key yet")) };
        Some(timed(SEARCH_WITHIN, &key, found(search.endpoint, key.clone())).await)
    };
    let (model, search) = tokio::join!(model, search);
    Ok(TryReport { model, search })
}

/// What the model says to a word.
async fn answer(settings: fastctx_agent::model::ModelSettings, key: String) -> Result<String, String> {
    let request = Request {
        system: "Reply with one short word.".into(),
        messages: vec![Message::User { text: "Say ready.".into() }],
        tools: Vec::new(),
        // Room for a reasoning model to think before its word.
        max_output: 512,
    };
    match client(settings, key).complete(&request).await {
        Ok(response) => match response.message {
            Message::Assistant { text, .. } if !text.trim().is_empty() => Ok(text.trim().to_string()),
            _ => Ok(format!("an answer with no words ({})", response.stop_reason)),
        },
        Err(error) => Err(error.reason),
    }
}

/// How many results the search finds for a word.
async fn found(endpoint: String, key: String) -> Result<String, String> {
    let result = Tavily { endpoint, key, audit: None }.search("weather").await?;
    let value: serde_json::Value = serde_json::from_str(&result.text).map_err(|error| error.to_string())?;
    Ok(format!("{} results", value["results"].as_array().map_or(0, Vec::len)))
}

/// `trying` within `within`, how long it took and what came of it, with
/// `key` hidden wherever the provider wrote it back: what a try says leaves
/// the home, and the key never does.
async fn timed(within: Duration, key: &str, trying: impl Future<Output = Result<String, String>>) -> Tried {
    let started = Instant::now();
    let outcome = tokio::time::timeout(within, trying).await;
    let ms = started.elapsed().as_millis() as u64;
    let shown = |text: &str| shortened(&if key.is_empty() { text.to_string() } else { text.replace(key, "***") });
    match outcome {
        Ok(Ok(said)) => Tried { ok: true, ms, said: shown(&said) },
        Ok(Err(why)) => Tried { ok: false, ms, said: shown(&why) },
        Err(_) => Tried { ok: false, ms, said: format!("no answer within {} s", within.as_secs()) },
    }
}

fn missing(why: &str) -> Tried {
    Tried { ok: false, ms: 0, said: why.into() }
}

/// `text` within what a try shows.
fn shortened(text: &str) -> String {
    match text.char_indices().nth(SAID) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}
