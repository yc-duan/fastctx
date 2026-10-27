//! What members say to each other, in the body of an envelope
//! (`crate::envelope`).

use fastctx_types::remote::{Letter, RemoteCall, RemoteResult};
use serde::{Deserialize, Serialize};

use crate::wire::Carried;

/// What a call or an answer holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Body {
    /// Asks the receiver to do something now.
    Call { call: RemoteCall },
    /// A call too large for a message: it follows over the envelope's data
    /// connection, as the JSON of a `RemoteCall`.
    LargeCall {
        carried: Carried,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<crate::call_details::CallDetails>,
    },
    /// A call that comes with the envelope's data connection, whose source
    /// is the caller: the two ends exchange what the call is about, such as
    /// the files of a copy, while the call waits for its answer.
    StreamCall { call: RemoteCall },
    /// The answer to the call the envelope names.
    Answer { result: RemoteResult },
    /// The answer to the call the envelope names, too large for a message:
    /// it follows over the envelope's data connection, as the JSON of a
    /// `RemoteResult`.
    LargeAnswer { carried: Carried },
}

/// What a letter holds. The sender names it with `id`: the receiver takes a
/// letter from one sender with one id once, and refuses the same id with
/// different contents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LetterBody {
    pub id: LetterId,
    pub letter: Letter,
}

/// A letter's name, chosen by its sender: lowercase letters, digits and
/// hyphens, at most 64 characters. A letter sent again after a crash, not
/// knowing whether it went, carries the same id.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LetterId(String);

impl LetterId {
    /// An id as long as any can be, for measuring.
    pub fn longest() -> Self {
        Self("z".repeat(64))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for LetterId {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let ok = !text.is_empty()
            && text.len() <= 64
            && text.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if ok { Ok(Self(text)) } else { Err(format!("{text:?} is not a letter id")) }
    }
}

impl From<LetterId> for String {
    fn from(id: LetterId) -> String {
        id.0
    }
}

/// The resident a call from `from` asks the machine it goes to to work for,
/// when it names one: a hop of an `on`, a run handed over to where its
/// resident works, a colleague asked, a file placed. The hub notes it, for
/// the letters that later tell that resident what came of the work.
pub fn worked_for(from: &fastctx_types::MachineName, call: &RemoteCall) -> Option<fastctx_types::Address> {
    let address = |resident: &fastctx_types::ResidentId| fastctx_types::Address {
        machine: from.clone(),
        resident: resident.clone(),
    };
    match call {
        RemoteCall::On { origin, .. } | RemoteCall::Follow { origin, .. } => origin.sentinel.is_none().then(||origin.resident.clone()),
        RemoteCall::Run { resident, .. } | RemoteCall::Place { resident, .. } => Some(address(resident)),
        RemoteCall::Ask { asker, .. } => Some(address(asker)),
        RemoteCall::SentinelHome {request:fastctx_types::sentinel_control::HomeRequest::Begin {owner,..}} => Some(owner.clone()),
        RemoteCall::SentinelHome {request:fastctx_types::sentinel_control::HomeRequest::Judge {request}} => Some(request.owner.clone()),
        _ => None,
    }
}

/// A bounded project-entry request whose origin the hub can verify before
/// forwarding it. Streamed calls cannot carry this control operation.
pub fn project_entry(
    message: &crate::envelope::Envelope,
) -> Option<(fastctx_types::ResidentId, fastctx_types::TownName)> {
    if message.kind != crate::envelope::Kind::Call
        || message.stream.is_some()
        || message.body.get("call").and_then(|call| call.get("call")).and_then(serde_json::Value::as_str)
            != Some("enter_project")
    {
        return None;
    }
    match message.body::<Body>().ok()? {
        Body::Call { call: RemoteCall::EnterProject { resident, town } } => Some((resident, town)),
        _ => None,
    }
}

/// A complete plan operation returned by a remote job to its conversation.
pub fn plan_request(message: &crate::envelope::Envelope) -> Option<fastctx_types::ResidentId> {
    if message.kind != crate::envelope::Kind::Call || message.stream.is_some() {
        return None;
    }
    if message.body.get("call").and_then(|call| call.get("call")).and_then(serde_json::Value::as_str) != Some("plan") {
        return None;
    }
    match message.body::<Body>().ok()? {
        Body::Call { call: RemoteCall::Plan { resident, .. } } => Some(resident),
        _ => None,
    }
}

/// The agent's login a call starts a session of, if it starts one: a
/// session opened for the user, or a colleague asked; none is the agent's
/// default login there.
pub fn login_asked(call: &RemoteCall) -> Option<(fastctx_types::AgentName, Option<std::path::PathBuf>)> {
    match call {
        RemoteCall::Open { agent, home, .. } => Some((agent.clone(), home.clone())),
        RemoteCall::Ask { started, .. } => {
            Some((started.callee.login.agent.clone(), started.callee.login.home.clone()))
        }
        _ => None,
    }
}
