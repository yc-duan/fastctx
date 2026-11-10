//! The account's opt-in history setting, or the computer's while alone.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistorySwitch {
    pub on: bool,
}

pub const HUB_TOO_OLD: &str = "update the hub before changing the account's notes setting";
