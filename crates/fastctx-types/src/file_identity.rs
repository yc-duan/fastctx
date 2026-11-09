//! Local filesystem coordinates are not ownership or global asset identities.
//! A path and a change stamp may vary while the opened file's identity remains.

use crate::sync::cursor::{ByteCount, Count};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "platform", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileIdentity {
    Unix { device: Count, inode: Count },
    Windows { volume: Count, index: Count },
}

/// Indicators detect changes but never prove that an old prefix is unchanged.
/// Decimal indicator strings preserve signed platform tick values exactly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileChangeStamp {
    pub size: ByteCount,
    pub modified: Timestamp,
    pub indicators: [String; 2],
}
