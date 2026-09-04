//! World: the user's trusted machines woven into one persistent environment for an AI.
//!
//! A World is one hub (`fastctx hub`) and any number of members (`fastctx node`). Every
//! member holds an Ed25519 identity, an X25519 wrap key, and the shared World key that the
//! hub never sees; the hub routes envelopes on their plaintext headers and stores ciphertext
//! it cannot read. This module tree is inert on a machine that never enrolled: nothing here
//! listens, dials out, or changes a model-visible byte until `~/.fastctx/world.toml` exists.
//!
//! Layout:
//! - `crypto`, `identity`, `keys`, `invite`, `envelope`: pure functions and file formats.
//! - `wire`, `messages`: the transport frames between a member and the hub, and the typed
//!   bodies carried inside envelopes.
//! - `members`, `grant`, `state`, `outbox`: the member's persistent view of the World.
//! - `link`: the HTTPS + WebSocket link and the network path it is pinned to.
//! - `session`, `client`: the member side of the hub connection and the API tool handlers use.
//! - `node`, `hub`: the two long-running processes.
//! - `surface`, `cli`: the World-mode tool surface and the control commands.

pub mod cli;
pub(crate) mod client;
pub(crate) mod crypto;
pub(crate) mod enroll;
pub(crate) mod envelope;
pub(crate) mod grant;
pub(crate) mod hub;
pub(crate) mod identity;
pub(crate) mod invite;
pub(crate) mod keys;
pub(crate) mod link;
pub(crate) mod members;
pub(crate) mod messages;
pub(crate) mod node;
pub(crate) mod outbox;
pub(crate) mod session;
pub(crate) mod state;
pub(crate) mod surface;
pub(crate) mod wire;

use crate::control::paths::ControlPaths;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Wire protocol minor version carried in `hello` and `challenge`; readers accept N-1.
pub(crate) const PROTOCOL_VERSION: u32 = 1;
/// HTTP path the hub upgrades to a WebSocket; the major version lives here.
pub(crate) const WS_PATH: &str = "/fastctx/world/v1";
/// Prefix of every domain-separated signature input.
pub(crate) const SIGNATURE_DOMAIN_PREFIX: &str = "fastctx-world/v1/";
/// Node names are words: `[a-z0-9-]{1,32}` (`spec.md` FR-W-003).
pub(crate) const MAX_NODE_NAME_LEN: usize = 32;
/// The hub's own name in envelope headers; never a legal member name because of the reserved
/// list in `validate_node_name`.
pub(crate) const HUB_NAME: &str = "hub";

const CONFIG_VERSION: u32 = 1;

/// TLS verification mode a member uses towards its hub.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TlsMode {
    /// A publicly trusted certificate, verified against the system roots.
    Webpki,
    /// The hub's self-signed certificate, pinned by SPKI hash at enrollment.
    Pinned,
    /// A reverse proxy or CDN terminates TLS; the channel binding is empty.
    Fronted,
}

impl TlsMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Webpki => "webpki",
            Self::Pinned => "pinned",
            Self::Fronted => "fronted",
        }
    }
}

/// Which network path the hub link takes (`design-transport.md` §4.4).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum NetworkMode {
    /// Try the physical interface first, then the operating system's routing and proxy.
    #[default]
    Auto,
    /// Pin the socket to a physical interface; ignore proxies, TUN adapters, and system DNS.
    Direct,
    /// Use the operating system's routing, resolver, and `HTTPS_PROXY`.
    System,
}

impl NetworkMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Direct => "direct",
            Self::System => "system",
        }
    }
}

/// `~/.fastctx/world.toml`: this machine's enrollment. Its existence is what puts the local
/// tool surface into World mode; everything mutable lives in `state.json` instead.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct WorldConfig {
    /// Format version of this file.
    pub(crate) version: u32,
    /// This member's name inside the World.
    pub(crate) name: String,
    /// Stable World id issued by the hub.
    pub(crate) world_id: String,
    /// Hub addresses (`host:port`), tried in order; IP literals survive DNS interception.
    pub(crate) hub: Vec<String>,
    /// Fingerprint of the hub's Ed25519 key, learned from the invite that enrolled this machine.
    pub(crate) hub_key: String,
    /// How the hub's TLS certificate is verified.
    pub(crate) tls: TlsMode,
    /// SPKI SHA-256 (hex) pinned at enrollment; present only in `pinned` mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pinned_spki_sha256: Option<String>,
    /// Network path selection.
    #[serde(default)]
    pub(crate) network: NetworkMode,
    /// Explicit physical interface for `direct`; omitted means the best candidate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) interface: Option<String>,
    /// When this machine enrolled, RFC 3339 UTC.
    pub(crate) enrolled_at: String,
}

/// Every World file under the user's FastCtx directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorldPaths {
    /// `~/.fastctx/world.toml`.
    pub(crate) config: PathBuf,
    /// `~/.fastctx/world/`, owner-only.
    pub(crate) dir: PathBuf,
    /// Ed25519 identity seed.
    pub(crate) identity_key: PathBuf,
    /// X25519 wrap seed.
    pub(crate) wrap_key: PathBuf,
    /// Every World key epoch this member holds.
    pub(crate) world_keys: PathBuf,
    /// Mutable link state: counters, cursors, last network mode.
    pub(crate) state: PathBuf,
    /// MAC-verified member records.
    pub(crate) members: PathBuf,
    /// MAC-verified grants.
    pub(crate) grants: PathBuf,
    /// Reliable messages awaiting the hub's ack, one file per sequence number.
    pub(crate) outbox_dir: PathBuf,
    /// Delivered steps mapped to job directories.
    pub(crate) steps_dir: PathBuf,
    /// Runtime status the daemon writes for `fastctx node status` and doctor.
    pub(crate) status: PathBuf,
    /// Local audit log, one JSON-lines file per day.
    pub(crate) audit_dir: PathBuf,
}

impl WorldPaths {
    pub(crate) fn from_control(paths: &ControlPaths) -> Self {
        let dir = paths.fastctx_dir.join("world");
        Self {
            config: paths.fastctx_dir.join("world.toml"),
            identity_key: dir.join("identity.key"),
            wrap_key: dir.join("wrap.key"),
            world_keys: dir.join("world.keys"),
            state: dir.join("state.json"),
            members: dir.join("members.json"),
            grants: dir.join("grants.json"),
            outbox_dir: dir.join("outbox"),
            steps_dir: dir.join("steps"),
            status: dir.join("status.json"),
            audit_dir: paths.fastctx_dir.join("audit"),
            dir,
        }
    }

    /// Creates the owner-only World directory and its subdirectories.
    pub(crate) fn ensure(&self) -> Result<(), String> {
        crate::edit::private_storage::ensure_private_directory(&self.dir, "World state")?;
        for directory in [&self.outbox_dir, &self.steps_dir] {
            std::fs::create_dir_all(directory).map_err(|error| {
                format!(
                    "Cannot create the World directory {}: {error}",
                    crate::paths::display_path(directory)
                )
            })?;
        }
        Ok(())
    }
}

/// Whether this machine is enrolled. A file-existence check only: it is what `fastctx serve`
/// consults to choose the World tool surface, and it must never touch the network.
pub(crate) fn is_enrolled(paths: &ControlPaths) -> bool {
    paths.fastctx_dir.join("world.toml").is_file()
}

/// Loads `world.toml`; `Ok(None)` when the machine is not enrolled.
pub(crate) fn load_config(paths: &WorldPaths) -> Result<Option<WorldConfig>, String> {
    let bytes = match std::fs::read(&paths.config) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Cannot read {}: {error}",
                crate::paths::display_path(&paths.config)
            ));
        }
    };
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        format!(
            "{} is not valid UTF-8: {error}",
            crate::paths::display_path(&paths.config)
        )
    })?;
    let config: WorldConfig = toml_edit::de::from_str(text).map_err(|error| {
        format!(
            "Cannot parse {}: {error}",
            crate::paths::display_path(&paths.config)
        )
    })?;
    if config.version > CONFIG_VERSION {
        return Err(format!(
            "{} was written by a newer fastctx (format {}); this build reads format {} at most.",
            crate::paths::display_path(&paths.config),
            config.version,
            CONFIG_VERSION
        ));
    }
    Ok(Some(config))
}

/// Writes `world.toml` atomically.
pub(crate) fn save_config(paths: &WorldPaths, config: &WorldConfig) -> Result<(), String> {
    let text = toml_edit::ser::to_string_pretty(config)
        .map_err(|error| format!("Cannot encode world.toml: {error}"))?;
    write_atomic(&paths.config, text.as_bytes())
}

/// Removes `world.toml`, which takes the machine out of World mode on the next connection.
pub(crate) fn remove_config(paths: &WorldPaths) -> Result<(), String> {
    match std::fs::remove_file(&paths.config) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Cannot remove {}: {error}",
            crate::paths::display_path(&paths.config)
        )),
    }
}

/// Validates a member name: lowercase ASCII letters, digits, and hyphens, up to 32 bytes, not a
/// reserved routing word.
pub(crate) fn validate_node_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > MAX_NODE_NAME_LEN {
        return Err(format!(
            "Invalid node name \"{name}\": expected 1 to {MAX_NODE_NAME_LEN} characters."
        ));
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(format!(
            "Invalid node name \"{name}\": use lowercase letters, digits, and hyphens only."
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(format!(
            "Invalid node name \"{name}\": it cannot start or end with a hyphen."
        ));
    }
    if matches!(name, "hub" | "all" | "local" | "self" | "none") || name.starts_with("tag-") {
        return Err(format!(
            "Invalid node name \"{name}\": that word is reserved."
        ));
    }
    Ok(())
}

/// Current time as RFC 3339 UTC with second precision.
pub(crate) fn now_rfc3339() -> String {
    format_rfc3339(time::OffsetDateTime::now_utc())
}

pub(crate) fn format_rfc3339(moment: time::OffsetDateTime) -> String {
    moment
        .replace_nanosecond(0)
        .unwrap_or(moment)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

pub(crate) fn parse_rfc3339(text: &str) -> Result<time::OffsetDateTime, String> {
    time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
        .map_err(|error| format!("Invalid RFC 3339 timestamp \"{text}\": {error}"))
}

/// Writes a file atomically (temporary sibling plus rename), owner-only on Unix.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| {
        format!(
            "Cannot write {} because it has no parent directory.",
            crate::paths::display_path(path)
        )
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let temporary = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        use std::io::Write;
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!(
            "Cannot write {}: {error}",
            crate::paths::display_path(path)
        ));
    }
    Ok(())
}

/// Reads a whole file; `Ok(None)` when it does not exist.
pub(crate) fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "Cannot read {}: {error}",
            crate::paths::display_path(path)
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::validate_node_name;

    #[test]
    fn node_names_are_words_and_reserved_routing_words_are_rejected() {
        assert!(validate_node_name("linux-builder").is_ok());
        assert!(validate_node_name("a").is_ok());
        for bad in ["", "Hub", "hub", "all", "-x", "x-", "a b", "tag-x", "x_y"] {
            assert!(
                validate_node_name(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
        assert!(validate_node_name(&"a".repeat(33)).is_err());
    }
}
