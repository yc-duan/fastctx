//! Frozen legacy-token accounting follows its terminal across cancellation.
use fastctx_types::{access::HubAccount, usage::Tokens};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Record {
    pub ledger: String,
    pub session: Option<String>,
    pub at: Timestamp,
    pub who: Option<HubAccount>,
    pub with: Vec<HubAccount>,
    pub model: String,
    pub tokens: Tokens,
    pub key: String,
}
