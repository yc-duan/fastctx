//! What one member says to another: an envelope the hub reads, checks and
//! routes, around the body it carries.
//!
//! Nothing in it is sealed or signed: the session it comes on was proved by
//! the sender's key, and the hub sets who it is from by that session. The
//! envelope says which account the sender speaks for and what it asks of
//! the receiver for that account (`Purpose`); the hub passes it on only when
//! the directory allows that, and the receiver checks it again against its
//! own view before acting (`crate::directory`).

use fastctx_types::access::Purpose;
use fastctx_types::{AccountName, MachineName};
use serde::{Deserialize, Serialize};

use crate::ids::HubId;
use crate::wire::Ticket;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub hub: HubId,
    pub from: MachineName,
    pub to: MachineName,
    /// The number of a call or a letter, counting up across everything
    /// its sender ever sends; an answer names the call it answers by it in
    /// `re`, and is itself numbered 0.
    pub n: u64,
    pub kind: Kind,
    /// Whom a call or a letter is for; an answer is for whoever the call
    /// was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<AccountName>,
    /// What a call or a letter asks of the receiver for `account`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<Purpose>,
    /// For what answers a call, that call's number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub re: Option<u64>,
    /// The data connection that goes with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<Ticket>,
    /// What it carries (`crate::message`), as JSON the hub passes on.
    pub body: serde_json::Value,
}

/// What an envelope carries; the hub routes by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A call: on to the receiver, which answers once.
    Call,
    /// The answer to a call.
    Answer,
    /// A letter, which the hub keeps until its receiver takes it.
    Letter,
    /// The receiver refused a call before doing anything of it; the body is
    /// the reason. It travels as the answer.
    Refused,
}

impl Kind {
    /// Whether it travels as an answer to a call.
    pub fn is_answer(self) -> bool {
        matches!(self, Kind::Answer | Kind::Refused)
    }
}

impl Envelope {
    /// The body as `T`.
    pub fn body<T: serde::de::DeserializeOwned>(&self) -> Result<T, String> {
        serde_json::from_value(self.body.clone()).map_err(|error| format!("it cannot be read: {error}"))
    }

    /// For a call or a letter, the account and purpose it names; an error
    /// when it names none.
    pub fn standing(&self) -> Result<(&AccountName, Purpose), String> {
        match (&self.account, self.purpose) {
            (Some(account), Some(purpose)) => Ok((account, purpose)),
            _ => Err("it names no account it is for".to_string()),
        }
    }
}
