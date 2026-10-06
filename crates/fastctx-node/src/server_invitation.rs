//! An invitation is issued only after its pinned hub can supply the release.

use std::sync::Arc;

use fastctx_install::server::{Kernel, Source, certificate_pin, command};
use fastctx_types::MachineName;
use fastctx_types::ipc::Failure;
use fastctx_types::scope::NamedScope;
use jiff::Timestamp;

use crate::Node;

pub async fn invite(
    node: &Arc<Node>,
    name: Option<MachineName>,
    expires: Timestamp,
    scope: Option<NamedScope>,
) -> Result<String, Failure> {
    let link = node.link().ok_or_else(|| Failure::new("this machine belongs to no World with a hub"))?;
    let version = env!("CARGO_PKG_VERSION");
    let (release, certificate) = link.kernel_release(version).await.map_err(Failure::new)?;
    let pin = certificate_pin(&certificate).map_err(|error| Failure::new(error.to_string()))?;
    let address =
        link.membership().hub.first().ok_or_else(|| Failure::new("this membership has no root hub address"))?;
    let directory = format!("https://{address}{}/v{version}", fastctx_types::kernel::RELEASE_PATH);
    let kernel = Kernel { version, x86_64_sha256: &release.x86_64_sha256, aarch64_sha256: &release.aarch64_sha256 };
    let source = Source::Hub { release_directory: &directory, spki_sha256: &pin };
    // Validate every command input before spending an invitation.
    command(kernel, "invitation", source).map_err(|error| Failure::new(error.to_string()))?;
    let scope = scope.map(|scope| crate::ledger::names::by_id(node, scope)).transpose().map_err(Failure::new)?;
    let code = link.invite(name, expires, fastctx_net::trust::Kind::Machine, scope).await.map_err(Failure::new)?;
    command(kernel, &code.to_text(), source).map_err(|error| Failure::new(error.to_string()))
}
