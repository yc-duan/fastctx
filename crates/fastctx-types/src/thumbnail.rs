//! On-demand images for the user's interface; the holding node stores no thumbnail.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThumbnailRequest {
    pub path: String,
    /// A positive maximum for the longest side; images are never enlarged.
    pub max: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thumbnail {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<crate::session::Fingerprint>,
    pub width: u32,
    pub height: u32,
    pub mime: String,
    pub base64: String,
}
