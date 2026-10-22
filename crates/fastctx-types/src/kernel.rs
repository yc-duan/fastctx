//! The public release contract of the command-line program, shared by the
//! hub and its callers: the archives a one-line installation downloads.

use serde::{Deserialize, Serialize};

pub const RELEASE_PATH: &str = "/fastctx/releases";
pub const X86_64_ARCHIVE: &str = "fastctx-x86_64-unknown-linux-gnu.tar.gz";
pub const AARCH64_ARCHIVE: &str = "fastctx-aarch64-unknown-linux-gnu.tar.gz";
/// Windows on x64; Windows on Arm runs it as it runs any x64 program.
pub const WINDOWS_X86_64_ARCHIVE: &str = "fastctx-x86_64-pc-windows-msvc.zip";
/// What the update packages call that archive (`crate::update`).
pub const WINDOWS_X86_64_PACKAGE: &str = "fastctx-x86_64-pc-windows-msvc";
/// Macs with Apple silicon, and with Intel.
pub const DARWIN_AARCH64_ARCHIVE: &str = "fastctx-aarch64-apple-darwin.tar.gz";
pub const DARWIN_X86_64_ARCHIVE: &str = "fastctx-x86_64-apple-darwin.tar.gz";
pub const SUMS: &str = "SHA256SUMS";

/// The archives a hub cannot pack from itself, each with its name among the
/// update packages (`crate::update`): one placed on the hub by hand comes
/// first, else the one the hub holds among the packages it brought in.
pub const HELD_ARCHIVES: [(&str, &str); 3] = [
    (WINDOWS_X86_64_ARCHIVE, WINDOWS_X86_64_PACKAGE),
    (DARWIN_AARCH64_ARCHIVE, "fastctx-aarch64-apple-darwin"),
    (DARWIN_X86_64_ARCHIVE, "fastctx-x86_64-apple-darwin"),
];

/// The archive a World program's update package is, by the package's
/// name: what a hub may have placed by hand among its releases.
pub fn archive_of(package: &str) -> Option<&'static str> {
    match package {
        "fastctx-x86_64-unknown-linux-gnu" => Some(X86_64_ARCHIVE),
        "fastctx-aarch64-unknown-linux-gnu" => Some(AARCH64_ARCHIVE),
        _ => HELD_ARCHIVES.iter().find(|(_, held)| *held == package).map(|(archive, _)| *archive),
    }
}

/// What the update packages call one of `HELD_ARCHIVES`.
pub fn held_package(archive: &str) -> Option<&'static str> {
    HELD_ARCHIVES.iter().find(|(held, _)| *held == archive).map(|(_, package)| *package)
}

/// Available archive digests. Missing Linux digests are sent only to callers
/// that explicitly accept a partial release; the Windows and Mac ones are
/// sent when the hub has that archive, which an earlier hub never says.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KernelRelease {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x86_64_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aarch64_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows_x86_64_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub darwin_aarch64_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub darwin_x86_64_sha256: Option<String>,
}

impl KernelRelease {
    /// The digest of one of `HELD_ARCHIVES`.
    pub fn held(&mut self, archive: &str) -> Option<&mut Option<String>> {
        match archive {
            WINDOWS_X86_64_ARCHIVE => Some(&mut self.windows_x86_64_sha256),
            DARWIN_AARCH64_ARCHIVE => Some(&mut self.darwin_aarch64_sha256),
            DARWIN_X86_64_ARCHIVE => Some(&mut self.darwin_x86_64_sha256),
            _ => None,
        }
    }
}

/// Whether release `version` comes after release `than`, in semver order,
/// a test release before the release it leads to; false where either is
/// not a release's version.
pub fn later(version: &str, than: &str) -> bool {
    match (semver::Version::parse(version), semver::Version::parse(than)) {
        (Ok(version), Ok(than)) => version > than,
        _ => false,
    }
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
