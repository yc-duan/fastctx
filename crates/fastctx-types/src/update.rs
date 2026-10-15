//! New versions of Fastctx (`world-upgrade.md`, 新版本自己往后怎样升级): the
//! two channels releases come on, the manifest each release publishes, what
//! every machine tells its hub of versions and what the hub tells back, and
//! what an interface on a computer asks its node.

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::MachineName;

/// The repository whose releases are Fastctx's.
pub const REPOSITORY: &str = "yc-duan/fastctx";

/// The minisign key every package is signed with, as the official updater
/// is given it: the key file's text in base64. A package is installed only
/// with a signature of this key, wherever it came from.
pub const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDMzQjk0NThCRkZCRjg1OEMKUldTTWhiLy9pMFc1TThHUitkUVVyWCtzT0NsOERUUHZKelZjcHJmS3lBUzBtUXBVNHdkenEzZjEK";

/// The file every release publishes its manifest as.
pub const MANIFEST: &str = "latest.json";

/// The command-line program for a Rust target triple is the package
/// `fastctx-<triple>`.
pub const KERNEL_PREFIX: &str = "fastctx-";

/// Which releases a device takes. The stable channel is GitHub's latest
/// release; the beta channel the newest of all releases, test releases and
/// stable ones alike.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    #[default]
    Stable,
    Beta,
}

/// A release's `latest.json`: the manifest the official updater reads, with
/// the command-line programs beside it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
    /// The application's packages by updater target: `windows-x86_64-nsis`,
    /// `darwin-aarch64-app`, `linux-x86_64-appimage`, `linux-x86_64-deb`,
    /// `android-apk`, and the updater's own shorter keys.
    pub platforms: BTreeMap<String, Package>,
    /// The command-line programs by Rust target triple, each a `.tar.gz`
    /// with the program and its notices.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fastctx: BTreeMap<String, Package>,
}

/// One package of a release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    /// Where it is published. A package without one is only held by the
    /// machines that carry it, and comes from them through the hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Its minisign signature, in base64 as the official updater reads it.
    pub signature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

impl Manifest {
    /// The package `target` names: an application target, or
    /// `fastctx-<triple>` for the command-line program.
    pub fn package(&self, target: &str) -> Option<&Package> {
        match target.strip_prefix(KERNEL_PREFIX) {
            Some(triple) => self.fastctx.get(triple),
            None => self.platforms.get(target),
        }
    }

    /// Its version, when it is one a release can have.
    pub fn semver(&self) -> Option<semver::Version> {
        release(&self.version)
    }

    /// Whether this release is newer than `version`.
    pub fn newer_than(&self, version: &str) -> bool {
        match (self.semver(), release(version)) {
            (Some(offered), Some(current)) => offered > current,
            (Some(_), None) => true,
            _ => false,
        }
    }
}

/// A version a release can have: `major.minor.patch`, perhaps with a
/// pre-release; never build metadata.
pub fn release(version: &str) -> Option<semver::Version> {
    if !crate::kernel::release_version(version) {
        return None;
    }
    semver::Version::parse(version).ok()
}

/// The newer of two manifests, either of which may be missing.
pub fn newest<'a>(a: Option<&'a Manifest>, b: Option<&'a Manifest>) -> Option<&'a Manifest> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.semver() > a.semver() { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// Whether `target` is a name a package may have, which is also its file
/// name in a cache: lower-case letters, digits, `-`, `_` and `.`.
pub fn target_name(target: &str) -> bool {
    !target.is_empty()
        && target.len() <= 64
        && !target.starts_with('.')
        && target.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_.".contains(&byte))
}

/// A package by its release's version and its target.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Held {
    pub version: String,
    pub target: String,
}

/// The application on a computer or phone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct App {
    pub version: String,
    /// The updater target its packages are for.
    pub target: String,
}

/// What a machine tells its hub of versions: when its session opens and
/// whenever any of it changes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// The World program it runs.
    pub kernel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<App>,
    /// Whether it reached GitHub when it last looked; none before it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<bool>,
    /// The newest release it knows of on each channel.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub found: Vec<Manifest>,
    /// It fetches packages for machines that cannot reach GitHub, when the
    /// hub asks (`Settings::relays`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub relays: bool,
    /// The packages it holds, checked against their signatures.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holds: Vec<Held>,
}

/// The versions a machine of the network runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Running {
    pub kernel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<bool>,
}

/// What the network's administrators set for updates: which channel its
/// devices follow unless their owners choose, and the version it asks
/// every machine to run at least. The minimum is said, never enforced.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    #[serde(default)]
    pub channel: Channel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<String>,
}

/// What the hub tells every machine of versions: the newest release seen
/// on each channel by any machine or by the hub itself, the network's
/// policy, and what each machine online runs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Network {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable: Option<Manifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beta: Option<Manifest>,
    #[serde(default)]
    pub policy: Policy,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub machines: BTreeMap<MachineName, Running>,
}

impl Network {
    pub fn on(&self, channel: Channel) -> Option<&Manifest> {
        match channel {
            Channel::Stable => self.stable.as_ref(),
            Channel::Beta => newest(self.stable.as_ref(), self.beta.as_ref()),
        }
    }
}

/// A package the hub asks a machine to fetch for others and deliver to it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fetch {
    /// The hub's number of the request, which the delivery names.
    pub id: u64,
    pub version: String,
    pub target: String,
    pub package: Package,
}

/// A device's own choices about updates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// New versions are downloaded when they appear and installed once
    /// nothing here is under way; otherwise only offered.
    #[serde(default)]
    pub auto: bool,
    /// Whether this device takes test releases; none follows the network.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beta: Option<bool>,
    /// Fetches packages for the network's machines that cannot reach GitHub.
    #[serde(default = "yes")]
    pub relays: bool,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self { auto: false, beta: None, relays: true }
    }
}

impl Settings {
    /// The channel this device follows under `policy`.
    pub fn channel(&self, policy: &Policy) -> Channel {
        match self.beta {
            Some(true) => Channel::Beta,
            Some(false) => Channel::Stable,
            None => policy.channel,
        }
    }
}

/// What a device found when it last looked on GitHub.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Looked {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Timestamp>,
    /// Whether GitHub answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<bool>,
    /// Why the look failed, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable: Option<Manifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beta: Option<Manifest>,
}

impl Looked {
    pub fn on(&self, channel: Channel) -> Option<&Manifest> {
        match channel {
            Channel::Stable => self.stable.as_ref(),
            Channel::Beta => newest(self.stable.as_ref(), self.beta.as_ref()),
        }
    }
}

/// Where a package is coming from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    Github,
    /// Through the hub, from its copy or a machine it asked.
    Network,
}

/// A package on its way here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fetching {
    pub version: String,
    pub target: String,
    pub via: Via,
    pub received: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

/// A package that could not be had, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failed {
    pub version: String,
    pub target: String,
    pub why: String,
}

/// A package this machine holds, checked, and where.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kept {
    pub version: String,
    pub target: String,
    pub path: String,
}

/// How updates stand on this machine, as its node keeps them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub settings: Settings,
    /// The channel it follows now.
    pub channel: Channel,
    pub looked: Looked,
    /// What the hub tells, when this machine is in a network whose hub
    /// tells it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<Network>,
    /// The newest release on its channel, from GitHub or the network.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<Manifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetching: Option<Fetching>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<Failed>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<Kept>,
    /// The application the interface here says it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<App>,
    /// The package this machine's World program is updated from when no
    /// application carries it: `fastctx-<triple>`.
    pub kernel_target: String,
}

/// What an interface on a computer asks its node about updates; each is
/// answered with the state after it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do")]
pub enum UpdateCall {
    State,
    /// Look on GitHub now.
    Look,
    Set {
        settings: Settings,
    },
    /// The interface here is this application; `carried` is the directory
    /// of the command-line programs it carries, which this machine hands
    /// the network when asked.
    App {
        app: App,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        carried: Option<String>,
    },
    /// Bring the package `target` of `version` here.
    Fetch {
        version: String,
        target: String,
    },
    /// Update this machine's World program from the newest release on its
    /// channel, as `fastctx update` does.
    Kernel,
}
