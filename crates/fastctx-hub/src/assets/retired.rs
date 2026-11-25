//! Scopes the hub no longer gives any device: the logs earlier releases
//! wrote into the asset library — each machine's own (`b/<body>/log`), the
//! hub's (`h/<hub>/log`) and each MCP command's (`Readers::Working`). Nothing
//! reads them and no release writes them any more (`fastctx-node`'s
//! `assets/logs.rs`), yet each new machine pulled what was kept of them. A
//! device that asks anything of one is told it holds it no longer, so it lets
//! go of what it has (`serve`, `Reply::Revoked`).
//!
//! An account's records (`activity`, `usage`, `jobs`, `output`) are not
//! among them: earlier nodes still write their own, and a machine away is
//! read from them (codex-580b4d87).

use super::scopes::Scope;
use fastctx_types::assets::{Readers, StreamClass};

/// Whether the whole of `scope` is no longer given to any device.
pub(crate) fn whole(scope: &Scope) -> bool {
    let key = scope.descriptor.scope.as_str();
    match &scope.descriptor.readers {
        Readers::Working { .. } => true,
        Readers::Hub | Readers::Body { .. } => key.ends_with("/log"),
        _ => false,
    }
}

/// Whether `stream` of `scope` is no longer given to any device.
pub(crate) fn stream(scope: &Scope, _stream: &str) -> bool {
    whole(scope)
}

/// Whether the streams of `class` in `scope` are no longer given to any
/// device.
pub(crate) fn class(scope: &Scope, _class: &StreamClass) -> bool {
    whole(scope)
}
