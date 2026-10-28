//! New versions of Fastctx (`world-upgrade.md`, 新版本自己往后怎样升级): the
//! two lines releases come on, the manifest each release publishes, what
//! every machine tells its hub of versions and what the hub tells back, and
//! what an interface on a computer asks its node.

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::MachineName;

mod view;
pub use view::*;

mod status;
pub use status::*;

/// The first release accepting selected versions and remote update settings.
pub const ASKED_SINCE: &str = "1.0.1-beta.3";

/// The first release with the standalone repair entry point.
pub const REPAIR_SINCE: &str = "1.0.1-beta.12";

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

/// Which releases a device takes: the line of the version it runs. One that
/// runs a test release takes test releases and stable ones alike; one that
/// runs a stable release, stable ones alone (`Channel::of`). Devices of 1.0.1
/// beta 1 and 2 also read the network's `Policy::channel` and their own
/// `Settings::beta`, which say no more now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    #[default]
    Stable,
    Beta,
}

/// A line a later release adds is read as the test releases': what it
/// offers is not taken for a stable release.
impl crate::tolerant::Tolerant for Channel {
    const ALL: &'static [Self] = &[Channel::Stable, Channel::Beta];
    const UNKNOWN: Self = Channel::Beta;
}

impl Channel {
    /// The line of `version`: a test release's is the beta line.
    pub fn of(version: &str) -> Self {
        match release(version) {
            Some(version) if !version.pre.is_empty() => Channel::Beta,
            _ => Channel::Stable,
        }
    }

    /// Whether the release `version` is on this line.
    pub fn takes(self, version: &str) -> bool {
        self == Channel::Beta || release(version).is_some_and(|version| version.pre.is_empty())
    }
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
    /// The update to what it runs, when someone asked for it from elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked: Option<Asked>,
    /// It runs the hub of the network it tells.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hub_here: bool,
    /// How fast, in bytes a second, its last download from GitHub went;
    /// none before it made one. The hub asks the fastest to fetch for the
    /// network first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_speed: Option<u64>,
    /// The package of its World program for its system, as releases name
    /// it: the hub brings in a new release's packages for the systems of
    /// its network before they are asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Reports a fetch's progress and stops it when the hub cancels it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub package_progress: bool,
    #[serde(default)]
    pub stage: Stage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto: Option<bool>,
    #[serde(default)]
    pub auto_chosen: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_failure: Option<CheckFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<Timestamp>,
    #[serde(default)]
    pub checking: bool,
    #[serde(default)]
    pub updates_view: bool,
    #[serde(default)]
    pub repair_supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair: Option<crate::repair::Report>,

}

impl Report {
    /// A failed check still describes reachability until a later connection succeeds.
    pub fn needs_network(&self) -> bool {
        self.github == Some(false)
            && self.checked_at.is_none_or(|at| self.github_at.is_none_or(|reached| reached <= at))
    }
}

/// The versions a machine of the network runs; for one away, what it ran
/// when it was last connected while the hub ran.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Running {
    pub kernel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<bool>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub away: bool,
    /// It runs the network's hub.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hub_here: bool,
    #[serde(default)]
    pub repair_supported: bool,

}

/// What the network's administrators set for updates: the version it asks
/// every machine to run at least, said and never enforced, and whether its
/// managed machines update by themselves. `channel` is what devices of 1.0.1
/// beta 1 and 2 follow; a hub tells it them as the beta line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    #[serde(default)]
    pub channel: Channel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<String>,
    /// Whether managed machines take new releases by themselves.
    #[serde(default = "yes")]
    pub managed_auto: bool,
    /// How fast the hub takes packages in at most, in bytes a second, where
    /// its super administrator raised or lowered it from `INTAKE`. Set by
    /// its own change; a change of the rest leaves it as it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intake: Option<u64>,
}

impl Default for Policy {
    fn default() -> Self {
        Self { channel: Channel::Stable, minimum: None, managed_auto: true, intake: None }
    }
}

impl Policy {
    /// How fast the hub takes packages in at most, in bytes a second.
    pub fn intake(&self) -> u64 {
        self.intake.unwrap_or(INTAKE)
    }
}

/// How fast a hub takes packages in by default, in bytes a second: some 40
/// Mbit/s, about half the lowest a cloud's guard against floods lets a small
/// server be set to (`world-network.md`, 中枢替网络存的东西).
pub const INTAKE: u64 = 5_000_000;

/// How slow and how fast a hub may be set to take packages in: slower than
/// the first, one message of a transfer would wait past what the other side
/// waits for; past the second it is no ceiling.
pub const INTAKE_RANGE: std::ops::RangeInclusive<u64> = 500_000..=1_000_000_000;

/// An update a machine away from the network is to make once it is back,
/// kept by the hub until the machine runs that release or a newer one
/// (`world-upgrade.md`, 别的机器的更新).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Due {
    pub version: String,
    /// Who asked for it, and when.
    pub by: crate::AccountName,
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<Manifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<Problem>,
}

/// An update someone asked of a machine from elsewhere, which it made: told
/// the hub it was asked through with its versions while it runs what it
/// updated to, for the machine log (`world-upgrade.md`, 谁来更新).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asked {
    pub by: crate::AccountName,
    /// What it ran before.
    pub from: String,
}

/// What the hub tells every machine of versions: the newest release seen
/// on each line by any machine or by the hub itself, the network's
/// policy, what each machine online runs, the hub's own version, and the
/// updates machines are to make once they are back.
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
    /// The version the hub runs; none from a hub before 1.0.1 beta 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub due: BTreeMap<MachineName, Due>,
    #[serde(default)]
    pub updates_view: bool,
    #[serde(default)]
    pub repair_supported: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub repairs: BTreeMap<MachineName, RepairDue>,

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
    /// The hub accepts progress and can cancel this request.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub progress: bool,
}

/// Bytes received while preparing a package, before its final answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageProgress {
    pub received: u64,
    pub total: Option<u64>,
}

/// A device's own choices about updates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// New versions are downloaded when they appear and installed once
    /// nothing here is under way; otherwise only offered. What the device's
    /// owner chose, when `chosen` says they did.
    #[serde(default)]
    pub auto: bool,
    /// Whether the owner chose `auto`; until they do, the device takes its
    /// kind's default (`Settings::auto_for`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub chosen: bool,
    /// What 1.0.1 beta 1 and 2 kept of test releases; accepted but never written.
    #[serde(default, skip_serializing)]
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
        Self { auto: false, chosen: false, beta: None, relays: true }
    }
}

/// What kind of device takes updates, for its default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A computer with the application: someone sits at it.
    App,
    /// A machine with no application, such as a server.
    Headless,
    /// A managed machine, which follows its network.
    Managed,
    #[serde(other)]
    Phone,
}

impl Settings {
    /// Changes only the choices explicitly supplied by the owner.
    pub fn with_preferences(mut self, auto: Option<bool>, relays: Option<bool>) -> Self {
        if let Some(auto) = auto {
            self.auto = auto;
            self.chosen = true;
        }
        if let Some(relays) = relays {
            self.relays = relays;
        }
        self
    }

    /// Whether a device of `kind` takes new releases by itself, under the
    /// network's `policy`: a managed machine as its network says; another as
    /// its owner chose, or by default, a computer with the application not,
    /// a machine without one so.
    pub fn auto_for(&self, kind: Kind, policy: Option<&Policy>) -> bool {
        match kind {
            Kind::Managed => policy.is_none_or(|policy| policy.managed_auto),
            _ if self.chosen => self.auto,
            Kind::App | Kind::Phone => false,
            Kind::Headless => true,
        }
    }
}

/// A release as GitHub lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<Timestamp>,
    /// Marked as a test release.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prerelease: bool,
    /// Where its manifest is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
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
    /// Every release with a manifest, the newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub releases: Vec<Release>,
    /// The manifests of the releases newer than what runs where it looked,
    /// the newest first, a few at most: what a device can update to, with
    /// the size of each package.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub newer: Vec<Manifest>,
    #[serde(default)]
    pub checking: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<CheckFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<Timestamp>,
    #[serde(default)]
    pub failures: u32,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<FailureReason>,
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
    /// The World program the node that answers runs; none from a node
    /// before 1.0.1 beta 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<String>,
    pub settings: Settings,
    /// The line it follows: that of the version it runs.
    #[serde(deserialize_with = "crate::tolerant::read")]
    pub channel: Channel,
    /// Whether it takes new releases by itself now (`Settings::auto_for`).
    #[serde(default)]
    pub auto: bool,
    /// The release its owner chose to update to, when it is not the newest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen: Option<Manifest>,
    /// The oldest major version among its hubs; an unknown hub uses the
    /// current kernel's major. None outside a network or on a hub's own machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub_major: Option<u64>,
    pub looked: Looked,
    /// What the hub tells, when this machine is in a network whose hub
    /// tells it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<Network>,
    /// The newest release on its channel, from GitHub or the network.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<Manifest>,
    /// The selected or newest newer release carrying this device's package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<Manifest>,
    /// The newest compatible candidate within the hubs' major version,
    /// independent of whether automatic updates are enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automatic: Option<String>,
    /// The offered compatible release exceeds the automatic major-version limit.
    #[serde(default)]
    pub above_hub: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetching: Option<Fetching>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<Failed>,
    /// When the failed release is tried again by itself; none where the
    /// interface keeps its own pace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<jiff::Timestamp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<Kept>,
    /// The application the interface here says it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<App>,
    /// The package this machine's World program is updated from when no
    /// application carries it: `fastctx-<triple>`.
    pub kernel_target: String,
    /// The World program this machine changes to, while it does, or that it
    /// did not take, with why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renewing: Option<Renewing>,
    /// The release someone asked from elsewhere that the application here
    /// update to: it takes it as an automatic update would, as soon as
    /// nothing here is under way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take: Option<String>,
    /// Whether an interface on this machine is open now.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub open: bool,
    #[serde(default)]
    pub stage: Stage,
    /// This node accepts local installation-stage reports.
    #[serde(default)]
    pub progress: bool,
    /// Accepts atomic partial preference changes.
    #[serde(default)]
    pub preferences: bool,
    #[serde(default)]
    pub repair_supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair: Option<crate::repair::Report>,

    /// Can route the distinct repair request to another machine.
    #[serde(default)]
    pub repair_routing: bool,

}

/// A change of this machine's World program, as it goes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Renewing {
    pub version: String,
    pub phase: crate::renewal::Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What an interface on a computer asks its node about updates; each is
/// answered with the state after it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "do")]
pub enum UpdateCall {
    /// Accepts a durable repair and returns before any node handover.
    Repair {
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        id: Option<String>,
    },
    /// Changes only the choices named, leaving other choices intact.
    Preferences {
        auto: Option<bool>,
        relays: Option<bool>,
    },
    /// An interface reports its installation stage; only local interfaces may.
    Progress {
        stage: Stage,
    },
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
        /// The World program it carries, beside its own executable: what a
        /// node that installed a new application while it was closed hands
        /// to the renewal.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        program: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        executable: Option<String>,
    },
    /// Bring the package `target` of `version` here.
    Fetch {
        version: String,
        target: String,
    },
    /// Update this machine's World program, as `fastctx update` does: to
    /// `version`, or from the newest release on its line.
    Kernel {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    /// What the interface here offers to update to is `version` from now
    /// on; none, the newest on its line again.
    Choose {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
}

/// A distinct repair arrangement, never interpreted by an older node as an update.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairDue {
    pub id: String,
    pub version: String,
    pub by: crate::AccountName,
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<Manifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<Problem>,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<Timestamp>,
}
