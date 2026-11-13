//! Where an account's devices stand in syncing with a hub (design §28.9):
//! what a device reports of itself, and what the hub reckons from its own
//! records for the account's sync status page. Times are milliseconds since
//! the Unix epoch; amounts are bytes unless named otherwise.

use serde::{Deserialize, Serialize};

/// How many scopes not yet pulled whole a report names at most.
pub const UNPULLED_NAMED: usize = 256;

/// What a device tells the hub of where it stands (design §28.9).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReport {
    /// Where it stands in each owner library it follows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<OwnerStanding>,
    /// Scopes it holds that are not yet pulled whole in their holding
    /// period, and up to `UNPULLED_NAMED` of their keys.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub unpulled: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unpulled_scopes: Vec<String>,
    /// Objects it is to hold and lacks.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub missing_objects: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub missing_bytes: u64,
    /// Frames it made that the hub does not have yet.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub unsent_frames: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub unsent_bytes: u64,
    /// Bytes a second it took from the hub over the last minute.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub rate: u64,
}

/// Where a device stands in one owner library.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerStanding {
    pub owner: String,
    pub generation: String,
    /// The change number it caught up to, and when that change was made.
    pub change: u64,
    pub at: i64,
    /// The bytes of frames it took since it last stood anew in the library.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub taken: u64,
}

/// What an account's machines on a hub amount to, as the hub last sampled
/// them: told in each of the account's machines' views.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSync {
    pub account: crate::AccountName,
    /// Its machines that report, and those of them caught up.
    pub devices: u32,
    pub synced: u32,
    /// How far behind the furthest behind is.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub behind: u64,
}

/// What kind of bytes is left to come.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    Conversations,
    Output,
    Other,
}

/// One device of the account, as the hub reckons it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSync {
    pub machine: crate::MachineName,
    /// The member's kind as the directory has it (`computer`, `phone`, …).
    pub kind: String,
    pub online: bool,
    /// Since when it is online, or offline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    /// When it last reported; none: it never did on this hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported: Option<i64>,
    /// How far behind the latest change it stands.
    pub behind: u64,
    /// Bytes of records left to come, by part; and whether that is at most.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left: Vec<(Part, u64)>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub about: bool,
    pub unpulled: u32,
    pub missing_objects: u64,
    pub missing_bytes: u64,
    pub unsent_frames: u64,
    pub unsent_bytes: u64,
    pub rate: u64,
    pub synced: bool,
}

/// A point of a device's line: how far behind it stood at `at`, and
/// whether it was online then.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LagPoint {
    pub machine: crate::MachineName,
    pub at: i64,
    pub behind: u64,
    pub online: bool,
}

/// What the hub holds of the account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubTotals {
    pub conversations: u64,
    pub files: u64,
    pub file_bytes: u64,
    pub record_bytes: u64,
}

/// The account's sync status on a hub.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncStatus {
    pub at: i64,
    /// The span the points cover, in hours, and the step between points, in
    /// milliseconds.
    pub hours: u32,
    pub step: i64,
    pub devices: Vec<DeviceSync>,
    pub points: Vec<LagPoint>,
    pub totals: HubTotals,
}

/// What this machine itself sees, the hub reached or not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalSync {
    pub machine: crate::MachineName,
    pub unsent_frames: u64,
    pub unsent_bytes: u64,
    /// Since when it is cut off from the hub, if it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_off: Option<i64>,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}
