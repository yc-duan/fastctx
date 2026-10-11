//! The public Linux release contract shared by the hub and its callers.

use serde::{Deserialize, Serialize};

pub const RELEASE_PATH: &str = "/fastctx/releases";
pub const X86_64_ARCHIVE: &str = "fastctx-x86_64-unknown-linux-gnu.tar.gz";
pub const AARCH64_ARCHIVE: &str = "fastctx-aarch64-unknown-linux-gnu.tar.gz";
pub const SUMS: &str = "SHA256SUMS";

/// Available Linux archive digests. Missing digests are sent only to callers
/// that explicitly accept a partial release.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KernelRelease {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x86_64_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aarch64_sha256: Option<String>,
}

/// Stable versions are also single safe directory components.
pub fn stable_version(version: &str) -> bool {
    let parts: Vec<_> = version.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
        })
}
