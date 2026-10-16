//! The one line that installs fastctx on a Linux server, or a Windows
//! computer, and registers it as a machine of an account (`world-network.md`, 登录与入网), or as a managed
//! machine of it (受管机器): a token is made only after the offered archives
//! and bootstrap inputs have been checked. One line may serve several
//! servers, each taking its own name, as its code says.

use std::sync::Arc;

use fastctx_install::server::{Kernel, Source, certificate_pin, command as linux_line, windows_command};
use fastctx_types::accounts::{AccountChange, MemberKind, TokenWay};
use fastctx_types::ipc::Failure;
use fastctx_types::{AccountName, HubName, MachineName};
use jiff::Timestamp;

use crate::Node;

/// What the code in a line is: the name its one server takes, until when,
/// for managed machines or not, for how many servers (one when none, any
/// number at zero) and what it is called.
pub struct Code {
    pub name: Option<MachineName>,
    pub expires: Timestamp,
    pub managed: bool,
    pub uses: Option<u32>,
    pub label: Option<String>,
    /// For Windows computers rather than Linux servers.
    pub windows: bool,
}

pub async fn command(
    node: &Arc<Node>,
    hub: Option<&HubName>,
    account: &AccountName,
    code: Code,
) -> Result<String, Failure> {
    let Code { name, expires, managed, uses, label, windows } = code;
    let link = crate::world::hub_link(node, hub)?;
    let version = env!("CARGO_PKG_VERSION");
    let (release, certificate) = link.kernel_release(version).await.map_err(Failure::new)?;
    let pin = certificate_pin(&certificate).map_err(|error| Failure::new(error.to_string()))?;
    // The server downloads from where the hub is reached, not from this
    // machine's own way to it when the hub runs here.
    let addresses = link.hub_addresses();
    let address = addresses
        .iter()
        .find(|address| !address.ip().is_loopback())
        .or(addresses.first())
        .ok_or_else(|| Failure::new("this membership has no hub address"))?;
    let directory = format!("https://{address}{}/v{version}", fastctx_types::kernel::RELEASE_PATH);
    let kernel = Kernel {
        version,
        x86_64_sha256: release.x86_64_sha256.as_deref(),
        aarch64_sha256: release.aarch64_sha256.as_deref(),
    };
    let source = Source::Hub { release_directory: &directory, spki_sha256: &pin };
    let line = |token: &str| {
        if windows {
            windows_command(version, release.windows_x86_64_sha256.as_deref(), token, source)
        } else {
            linux_line(kernel, token, source)
        }
    };
    // Every input of the line is checked before a token is spent on it.
    line("FX5TOKEN").map_err(|error| Failure::new(error.to_string()))?;
    let change = AccountChange::Token {
        register: true,
        name,
        kind: MemberKind::Computer,
        expires,
        managed,
        uses,
        label,
        way: Some(TokenWay::Line),
    };
    let token = link
        .account_change(account, change)
        .await
        .map_err(Failure::new)?
        .ok_or_else(|| Failure::new("the hub made no token"))?;
    line(&token).map_err(|error| Failure::new(error.to_string()))
}
