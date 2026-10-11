//! The one line that installs fastctx on a Linux server and registers it as
//! a machine of an account (`world-network.md`, 登录与入网): a token is made
//! only after the offered archives and bootstrap inputs have been checked.

use std::sync::Arc;

use fastctx_install::server::{Kernel, Source, certificate_pin, command as line};
use fastctx_types::accounts::{AccountChange, MemberKind};
use fastctx_types::ipc::Failure;
use fastctx_types::{AccountName, HubName, MachineName};
use jiff::Timestamp;

use crate::Node;

pub async fn command(
    node: &Arc<Node>,
    hub: Option<&HubName>,
    account: &AccountName,
    name: Option<MachineName>,
    expires: Timestamp,
) -> Result<String, Failure> {
    let link = crate::world::hub_link(node, hub)?;
    let version = env!("CARGO_PKG_VERSION");
    let (release, certificate) = link.kernel_release(version).await.map_err(Failure::new)?;
    let pin = certificate_pin(&certificate).map_err(|error| Failure::new(error.to_string()))?;
    let address =
        link.membership().addresses.first().ok_or_else(|| Failure::new("this membership has no hub address"))?;
    let directory = format!("https://{address}{}/v{version}", fastctx_types::kernel::RELEASE_PATH);
    let kernel = Kernel {
        version,
        x86_64_sha256: release.x86_64_sha256.as_deref(),
        aarch64_sha256: release.aarch64_sha256.as_deref(),
    };
    let source = Source::Hub { release_directory: &directory, spki_sha256: &pin };
    // Every input of the line is checked before a token is spent on it.
    line(kernel, "token", source).map_err(|error| Failure::new(error.to_string()))?;
    let change = AccountChange::Token { register: true, name, kind: MemberKind::Computer, expires };
    let token = link
        .account_change(account, change)
        .await
        .map_err(Failure::new)?
        .ok_or_else(|| Failure::new("the hub made no token"))?;
    line(kernel, &token, source).map_err(|error| Failure::new(error.to_string()))
}
