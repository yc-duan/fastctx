//! `fastctx proxy`: the HTTP proxy this computer reaches its hubs through,
//! chosen in Fastctx itself, so a node its system service starts uses it as
//! well as one a shell starts (`world-network.md`, 登记了，就一直连着).

use anyhow::bail;
use clap::Subcommand;
use fastctx_types::{Call, Reply};

#[derive(Subcommand)]
pub enum ProxyCommand {
    /// Reach the hubs through this HTTP proxy, as http://host:port, before
    /// the system's.
    Set {
        #[arg(value_parser = parser)]
        url: String,
    },
    /// Reach the hubs as the system's settings say again.
    Clear,
    /// The proxy chosen here, if any.
    Show,
}

pub fn run(command: ProxyCommand) -> anyhow::Result<()> {
    let url = match command {
        ProxyCommand::Set { url } => set(Some(url))?,
        ProxyCommand::Clear => set(None)?,
        ProxyCommand::Show => match crate::ask::call(Call::HubProxy { url: None, change: false })?.0 {
            Reply::HubProxy { url } => url,
            other => bail!("the node answered something else: {other:?}"),
        },
    };
    match url {
        Some(url) => println!("the hubs are reached through the proxy {}", fastctx_net::proxy::shown(&url)),
        None => println!("the hubs are reached as the system's settings say"),
    }
    Ok(())
}

/// A proxy as the command line takes it: checked before anything is done.
pub fn parser(text: &str) -> Result<String, String> {
    fastctx_net::proxy::check(text).map(|_| text.trim().to_string())
}

/// Sets the proxy, or clears it with none; returns it as it is now.
pub fn set(url: Option<String>) -> anyhow::Result<Option<String>> {
    match crate::ask::call(Call::HubProxy { url, change: true })?.0 {
        Reply::HubProxy { url } => Ok(url),
        other => bail!("the node answered something else: {other:?}"),
    }
}
