//! Who is logged in to a Codex home and how much of its limits is used, as
//! its own app-server says: the World reads no credentials, and the
//! requests these make go out from the machine the app-server runs on.

use fastctx_types::agents::{AgentAccount, AgentCredits, AgentLimits, Fact, LimitWindow};
use serde_json::{Value, json};

use super::server::AppServer;

impl super::CodexHost {
    pub async fn require_login(self: &std::sync::Arc<Self>) -> Result<(), String> {
        let server = self.server().await?;
        match account(&server).await {
            Fact::Known { .. } => Ok(()),
            Fact::Unavailable { reason } | Fact::Failed { reason } => Err(reason),
        }
    }
}

/// `account/read`, without asking it to refresh the login: an API key and a
/// ChatGPT login without an email name no one.
pub(super) async fn account(server: &AppServer) -> Fact<AgentAccount> {
    let answer = match server.request("account/read", json!({ "refreshToken": false })).await {
        Ok(answer) => answer,
        Err(error) => return Fact::failed(format!("account/read: {error}")),
    };
    let account = &answer["account"];
    match account["type"].as_str() {
        None if account.is_null() => Fact::unavailable("no one is logged in to this Codex home"),
        Some("chatgpt") => Fact::known(AgentAccount {
            email: account["email"].as_str().map(str::to_string),
            organization: None,
            plan: account["planType"].as_str().map(str::to_string),
        }),
        Some(_) => Fact::known(AgentAccount::default()),
        None => Fact::failed(format!("account/read answered an account of no type: {account}")),
    }
}

/// `account/rateLimits/read`: every limit's windows, each named by its
/// limit and known by its length, never by which slot it came in (the
/// first is not always the five-hour one), and the credits the account
/// goes on with once a window is used up.
pub(super) async fn limits(server: &AppServer) -> Fact<AgentLimits> {
    let observed_at = jiff::Timestamp::now();
    let answer = match server.request("account/rateLimits/read", json!({})).await {
        Ok(answer) => answer,
        Err(error) => return Fact::failed(format!("account/rateLimits/read: {error}")),
    };
    let snapshots: Vec<&Value> = match answer["rateLimitsByLimitId"].as_object() {
        Some(buckets) if !buckets.is_empty() => buckets.values().collect(),
        _ => vec![&answer["rateLimits"]],
    };
    let mut windows = Vec::new();
    let mut credits = None;
    for snapshot in snapshots.into_iter().filter(|snapshot| !snapshot.is_null()) {
        let held = &snapshot["credits"];
        if credits.is_none() && held["hasCredits"].as_bool() == Some(true) {
            credits = Some(AgentCredits {
                balance: held["balance"].as_str().map(str::to_string),
                unlimited: held["unlimited"].as_bool() == Some(true),
            });
        }
        let name =
            snapshot["limitName"].as_str().or_else(|| snapshot["limitId"].as_str()).unwrap_or("codex").to_string();
        for slot in ["primary", "secondary"] {
            let window = &snapshot[slot];
            let Some(used) = window["usedPercent"].as_i64() else { continue };
            let Ok(used) = u32::try_from(used) else {
                return Fact::failed(format!("Codex reported {used}% used of {name}"));
            };
            let resets_at = match window["resetsAt"].as_i64().map(jiff::Timestamp::from_second).transpose() {
                Ok(resets_at) => resets_at,
                Err(error) => return Fact::failed(format!("Codex reported an invalid reset time: {error}")),
            };
            windows.push(LimitWindow {
                name: name.clone(),
                minutes: window["windowDurationMins"].as_u64().and_then(|minutes| u32::try_from(minutes).ok()),
                used_tenths: used.saturating_mul(10),
                resets_at,
            });
        }
    }
    Fact::known(AgentLimits { observed_at, windows, credits })
}
