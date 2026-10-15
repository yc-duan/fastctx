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

/// A released version: `major.minor.patch`, or a test release of it with a
/// semver pre-release such as `1.0.1-beta.2`; never build metadata. Each is
/// also a single safe directory component.
pub fn release_version(version: &str) -> bool {
    let number = |part: &str| {
        !part.is_empty()
            && part.bytes().all(|byte| byte.is_ascii_digit())
            && (part.len() == 1 || !part.starts_with('0'))
    };
    let (core, pre) = match version.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (version, None),
    };
    let parts: Vec<_> = core.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| number(part))
        && pre.is_none_or(|pre| {
            pre.split('.').all(|part| {
                !part.is_empty()
                    && part.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                    && (!part.bytes().all(|byte| byte.is_ascii_digit()) || number(part))
            })
        })
}
