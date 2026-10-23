//! Temporary account-record wording, kept together for editorial replacement.

pub const KEY: &str = "a record key must contain 1 to 256 UTF-8 bytes without control characters";
pub const KIND: &str = "a record kind must contain 1 to 32 lowercase ASCII letters, digits, underscores or hyphens";
pub const ACCOUNT_REPLACED: &str = "the account was replaced";
pub const DEVICE_REPLACED: &str = "the requesting device was replaced";
pub const REVISION: &str = "the record revision cannot be incremented";
pub const HUB_REPLY: &str = "the hub did not answer the account record request";
pub const HUB_LIST: &str = "the hub did not list the account records";
pub const WATCH_CLOSED: &str = "the account records watch closed";
pub const HUB_CLOSED: &str = "the hub connection closed";
pub const WATCH_CONNECTION: &str = "account records require a watch connection";
pub const NODE_OLD: &str = "upgrade this node to use account records";
pub const CONFLICT: &str = "record version conflict";

pub fn body_limit(size: usize) -> String {
    format!("the record body is {size} bytes; the limit is {}", super::MAX_BODY_BYTES)
}
pub fn kind_limit() -> String {
    format!("this record kind has reached its limit of {} live records", super::MAX_KIND_KEYS)
}
pub fn account_limit(size: usize) -> String {
    format!("the account records would occupy {size} bytes; the limit is {}", super::MAX_ACCOUNT_BYTES)
}
pub fn store(error: impl std::fmt::Display) -> String {
    format!("the hub could not store the record: {error}")
}
pub fn read(error: impl std::fmt::Display) -> String {
    format!("reading the record: {error}")
}
pub fn durability(version: u64, error: impl std::fmt::Display) -> String {
    format!("record version {version} is visible, but its durability could not be confirmed: {error}")
}
pub fn disconnected(reason: impl std::fmt::Display) -> String {
    format!("not connected to the hub: {reason}")
}

pub fn old_hub(hub: impl std::fmt::Display) -> String {
    format!("upgrade hub {hub} to use account records")
}
