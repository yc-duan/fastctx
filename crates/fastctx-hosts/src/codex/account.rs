//! Who is logged in to a Codex home and how much of its limits is used, as
//! its own app-server says: the World reads no credentials, and the
//! requests these make go out from the machine the app-server runs on.
//!
//! A read (`account/rateLimits/read`) answers every limit's snapshot; while
//! the login's sessions run, the app-server pushes sparse updates
//! (`account/rateLimits/updated`), which Codex asks clients to merge into the
//! last read: a value an update leaves out or sends as null keeps what was
//! read (the schema of `AccountRateLimitsUpdatedNotification`, 0.160).

use std::collections::BTreeMap;
use std::path::PathBuf;

use fastctx_types::agents::{AgentAccount, AgentCredits, AgentLimits, Fact, LimitWindow};
use fastctx_types::logins::Identity;
use jiff::Timestamp;
use serde_json::{Value, json};

use super::server::AppServer;

#[derive(Default)]
pub(super) struct Owner {
    pub generation: u64,
    pub identity: Option<(Timestamp, Identity)>,
}

impl super::CodexHost {
    /// Reads the complete account-validated response, including ordinaryUsageAllowed.
    pub async fn rate_limits_now(self: &std::sync::Arc<Self>) -> Result<Value, String> {
        let server = self.server().await?;
        let answer = server.request("account/rateLimits/read", json!({})).await?;
        self.limits_heard(Timestamp::now(), LimitsHow::Read(answer.clone()));
        Ok(answer)
    }

    pub async fn require_login(self: &std::sync::Arc<Self>) -> Result<(), String> {
        let server = self.server().await?;
        match account(&server).await {
            Fact::Known { .. } => Ok(()),
            Fact::Unavailable { reason } | Fact::Failed { reason } => Err(reason),
        }
    }

    /// Where this host's app-server tells what it reads or hears of its
    /// login's limits.
    pub fn report_limits_to(&self, sink: tokio::sync::mpsc::UnboundedSender<LimitsReport>) {
        *self.limits.lock().expect("never poisoned") = Some(sink);
    }

    pub(super) fn limits_heard(&self, at: Timestamp, how: LimitsHow) {
        let owner = self.limits_owner.lock().expect("never poisoned").identity.clone();
        if let Some(sink) = self.limits.lock().expect("never poisoned").as_ref() {
            let _ = sink.send(LimitsReport { home: self.codex_home.clone(), at, how, owner });
        }
    }
}

/// What an app-server read or pushed of its login's limits, and when.
#[derive(Clone, Debug)]
pub struct LimitsReport {
    /// The login: the home the app-server runs for.
    pub home: PathBuf,
    pub at: Timestamp,
    pub how: LimitsHow,
    /// Identity observed in the producing app-server, not at delivery time.
    pub owner: Option<(Timestamp, Identity)>,
}

#[derive(Clone, Debug)]
pub enum LimitsHow {
    /// The answer of `account/rateLimits/read`: every limit.
    Read(Value),
    /// The `rateLimits` of `account/rateLimits/updated`: one limit's
    /// snapshot, with only what changed.
    Updated(Value),
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
        // A configured provider that needs no OpenAI login works with none.
        None if account.is_null() && answer["requiresOpenaiAuth"].as_bool() == Some(false) => {
            Fact::known(AgentAccount::default())
        }
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

/// `account/read`, as who is logged in and how: the account's type as
/// Codex names it (`chatgpt`, `apiKey`, …). With no account, no one is
/// logged in, unless the configured provider needs no OpenAI login.
pub(super) async fn identity(server: &AppServer) -> Fact<Identity> {
    let answer = match server.request("account/read", json!({ "refreshToken": false })).await {
        Ok(answer) => answer,
        Err(error) => return Fact::failed(format!("account/read: {error}")),
    };
    let account = &answer["account"];
    if account.is_null() {
        let needed = answer["requiresOpenaiAuth"].as_bool() != Some(false);
        return Fact::known(Identity { logged_in: !needed, ..Identity::default() });
    }
    let Some(kind) = account["type"].as_str() else {
        return Fact::failed(format!("account/read answered an account of no type: {account}"));
    };
    Fact::known(Identity {
        logged_in: true,
        method: Some(kind.to_string()),
        email: account["email"].as_str().map(str::to_string),
        organization: None,
        plan: account["planType"].as_str().map(str::to_string),
    })
}

/// `account/rateLimits/read`: every limit's windows, each named by its
/// limit and known by its length, never by which slot it came in (the
/// first is not always the five-hour one), and the credits the account
/// goes on with once a window is used up.
pub(super) async fn limits(server: &AppServer) -> Fact<AgentLimits> {
    let observed_at = jiff::Timestamp::now();
    match server.request("account/rateLimits/read", json!({})).await {
        Ok(answer) => limits_of(&buckets(&answer), observed_at),
        Err(error) => Fact::failed(format!("account/rateLimits/read: {error}")),
    }
}

/// A limit's key: its id, as Codex keys the limits it reads; empty for a
/// snapshot that names none.
fn limit_key(snapshot: &Value) -> String {
    snapshot["limitId"].as_str().unwrap_or_default().to_string()
}

/// The limits a read answered, by limit.
pub fn buckets(answer: &Value) -> BTreeMap<String, Value> {
    match answer["rateLimitsByLimitId"].as_object() {
        Some(buckets) if !buckets.is_empty() => buckets
            .iter()
            .filter(|(_, snapshot)| !snapshot.is_null())
            .map(|(id, snapshot)| (id.clone(), snapshot.clone()))
            .collect(),
        _ if answer["rateLimits"].is_object() => {
            [(limit_key(&answer["rateLimits"]), answer["rateLimits"].clone())].into()
        }
        _ => BTreeMap::new(),
    }
}

/// Merges one limit's sparse update into the limits last read.
pub fn merge(buckets: &mut BTreeMap<String, Value>, update: &Value) {
    let Some(fields) = update.as_object() else { return };
    let bucket = buckets.entry(limit_key(update)).or_insert_with(|| json!({}));
    let Some(bucket) = bucket.as_object_mut() else { return };
    for (name, value) in fields {
        if !value.is_null() {
            bucket.insert(name.clone(), value.clone());
        }
    }
}

/// The windows and credits of the limits read, as read at `observed_at`.
pub fn limits_of(buckets: &BTreeMap<String, Value>, observed_at: Timestamp) -> Fact<AgentLimits> {
    let mut windows = Vec::new();
    let mut credits = None;
    for snapshot in buckets.values() {
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
                since: None,
            });
        }
    }
    Fact::known(AgentLimits { observed_at, windows, credits })
}
