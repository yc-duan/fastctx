//! The directory a hub kept before accounts had two levels and machines were
//! shared and their agents' logins assisted others (`world-network.md`),
//! read into today's shape.
//!
//! That hub kept `admin` and `may_register` on each account, `grants` of two
//! switches, working on a machine and using its agents, and `trust` of an
//! account in a machine. Today an account is the super administrator or a
//! member; working on a machine comes with all its agents; an agent's login
//! assists alone; and a machine acts for an account it is not logged in to
//! because the account used an agent there. What the old switches said maps
//! onto that without giving anyone more than they had:
//!
//! - the earliest opened administrator that is not locked becomes the super
//!   administrator, every other account a member;
//! - a grant that let an account work on a machine becomes a share of it;
//! - a grant of the agents alone becomes the assistance of that machine's
//!   default Codex and Claude Code logins, which were all the agents it
//!   named then that every machine has;
//! - an account's trust in a machine becomes that machine acting for it.

use jiff::Timestamp;
use serde_json::{Map, Value, json};

/// `book` in today's shape, and what changed in words for the hub's log;
/// no words when it was in today's shape already.
pub(crate) fn upgrade(mut book: Value) -> (Value, Vec<String>) {
    let mut said = Vec::new();
    let Some(object) = book.as_object_mut() else { return (book, said) };
    let old = object.contains_key("grants")
        || object.contains_key("trust")
        || accounts(object).iter().any(|account| account.get("level").is_none());
    if !old {
        return (book, said);
    }
    levels(object, &mut said);
    let grants = take_list(object, "grants");
    let trust = take_list(object, "trust");
    let mut shares = take_list(object, "shares");
    // Kept under `lends`, as `Directory` writes its assistance.
    let mut assists = take_list(object, "lends");
    let mut acting = take_list(object, "acting");
    for grant in grants {
        let (Some(machine), Some(account)) = (grant.get("machine").cloned(), grant.get("account").cloned()) else {
            continue;
        };
        let rights = grant.get("rights");
        let hands = rights.and_then(|rights| rights.get("hands")).and_then(Value::as_bool).unwrap_or(false);
        let agents = rights.and_then(|rights| rights.get("agents")).and_then(Value::as_bool).unwrap_or(false);
        if hands {
            said.push(format!("{} may work on {}: it is shared with it now", text(&account), text(&machine)));
            shares.push(json!({ "machine": machine, "account": account }));
        } else if agents {
            said.push(format!(
                "{} could use the agents on {}: their default Codex and Claude Code logins assist it now",
                text(&account),
                text(&machine)
            ));
            for agent in ["codex", "claude"] {
                assists.push(json!({ "machine": machine, "account": account, "agent": agent }));
            }
        }
    }
    let now = Timestamp::now().to_string();
    for trusted in trust {
        let (Some(machine), Some(account)) = (trusted.get("machine").cloned(), trusted.get("account").cloned()) else {
            continue;
        };
        said.push(format!("{} trusted {}: it acts for it now", text(&account), text(&machine)));
        acting.push(json!({ "machine": machine, "account": account, "since": now }));
    }
    object.insert("shares".to_string(), Value::Array(shares));
    object.insert("lends".to_string(), Value::Array(assists));
    object.insert("acting".to_string(), Value::Array(acting));
    (book, said)
}

fn accounts(object: &Map<String, Value>) -> Vec<&Value> {
    object.get("accounts").and_then(Value::as_array).map(|list| list.iter().collect()).unwrap_or_default()
}

fn take_list(object: &mut Map<String, Value>, key: &str) -> Vec<Value> {
    match object.remove(key) {
        Some(Value::Array(list)) => list,
        _ => Vec::new(),
    }
}

fn text(value: &Value) -> String {
    value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string())
}

/// Gives every account a level: the super administrator as the module
/// says, the rest members.
fn levels(object: &mut Map<String, Value>, said: &mut Vec<String>) {
    let Some(Value::Array(accounts)) = object.get_mut("accounts") else { return };
    if let Some(existing) =
        accounts.iter().position(|account| account.get("level").and_then(Value::as_str) == Some("super"))
    {
        for (index, account) in accounts.iter_mut().enumerate() {
            settle(account, index == existing);
        }
        return;
    }
    let flag = |account: &Value, key: &str| account.get(key).and_then(Value::as_bool).unwrap_or(false);
    let opened =
        |account: &Value| account.get("opened").and_then(Value::as_str).and_then(|text| text.parse::<Timestamp>().ok());
    let earliest = |keep: &dyn Fn(&Value) -> bool| {
        accounts
            .iter()
            .enumerate()
            .filter(|(_, account)| keep(account))
            .min_by_key(|(_, account)| opened(account))
            .map(|(index, _)| index)
    };
    let chosen = earliest(&|account| flag(account, "admin") && !flag(account, "locked"))
        .or_else(|| earliest(&|account| flag(account, "admin")))
        .or_else(|| earliest(&|_| true));
    for (index, account) in accounts.iter_mut().enumerate() {
        let name = account.get("name").map(text).unwrap_or_default();
        let was_admin = flag(account, "admin");
        let chosen = Some(index) == chosen;
        if chosen {
            said.push(format!("{name} is the hub's super administrator"));
            if flag(account, "locked") {
                said.push(format!("{name} was locked and is unlocked: the super administrator cannot be locked"));
                account["locked"] = Value::Bool(false);
            }
        } else if was_admin {
            said.push(format!("{name} was an administrator and is a member now"));
        }
        settle(account, chosen);
    }
}

fn settle(account: &mut Value, chosen: bool) {
    if let Some(account) = account.as_object_mut() {
        account.remove("admin");
        account.remove("may_register");
        account.insert("level".to_string(), Value::String(if chosen { "super" } else { "member" }.to_string()));
    }
}

/// Takes out the relays a directory kept before direct connections replaced
/// them (`world-network.md`, 直连): which relays each machine connected
/// through, and where a machine relayed for others. Nothing reads them any
/// more; a machine whose own line is down now reaches the hub through a
/// machine that allows direct connections, which the super administrator
/// allows anew. Says what was taken out.
pub(crate) fn drop_relays(book: &mut Value) -> Vec<String> {
    let mut said = Vec::new();
    let Some(members) = book.get_mut("members").and_then(Value::as_array_mut) else { return said };
    for member in members {
        let Some(member) = member.as_object_mut() else { continue };
        let name = member.get("name").map(text).unwrap_or_default();
        if let Some(relays) = member.remove("relays")
            && relays.as_array().is_some_and(|relays| !relays.is_empty())
        {
            said.push(format!("{name} no longer connects through the relays it had: {relays}"));
        }
        if let Some(at) = member.remove("relay_at")
            && at.as_array().is_some_and(|at| !at.is_empty())
        {
            said.push(format!("{name} no longer relays for others at {at}"));
        }
    }
    said
}
