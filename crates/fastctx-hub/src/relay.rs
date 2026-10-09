//! `fastctx relay`: forwards one port to the hub, byte for byte
//! (`world-network.md`, 转接站只转字节).
//!
//! A machine with a poor line to the hub dials a relay instead; the relay
//! passes the connection to the hub as it is, so the TLS between the machine
//! and the hub runs through it whole, and the relay can neither read nor
//! change it. A relay needs no account and is no member. It forwards only to
//! the hub it was started for, never anywhere else.

use std::net::SocketAddr;

use tokio::net::{TcpListener, TcpStream};

/// Forwards every connection to `listen` to the first of `hub`'s addresses
/// that answers, until the process is stopped.
pub async fn run(listen: SocketAddr, hub: Vec<SocketAddr>) -> anyhow::Result<()> {
    if hub.is_empty() {
        anyhow::bail!("a relay needs the hub's address");
    }
    let listener =
        TcpListener::bind(listen).await.map_err(|error| anyhow::anyhow!("listening on {listen}: {error}"))?;
    line(format_args!(
        "relay {} listening on {listen}, forwarding to {}",
        env!("CARGO_PKG_VERSION"),
        hub.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
    ));
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((inbound, peer)) => {
                    fastctx_net::dial::configure(&inbound);
                    tokio::spawn(forward(inbound, peer, hub.clone()));
                }
                // Out of descriptors, for one: the next accept may succeed.
                Err(error) => {
                    line(format_args!("accepting: {error}"));
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            },
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    line(format_args!("stopping"));
    Ok(())
}

async fn forward(mut inbound: TcpStream, peer: SocketAddr, hub: Vec<SocketAddr>) {
    let mut failures = Vec::new();
    for address in &hub {
        match TcpStream::connect(address).await {
            Ok(mut outbound) => {
                fastctx_net::dial::configure(&outbound);
                match tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await {
                    Ok((up, down)) => line(format_args!("{peer}: {up} bytes to the hub, {down} back")),
                    Err(error) => line(format_args!("{peer}: the forwarding ended: {error}")),
                }
                return;
            }
            Err(error) => failures.push(format!("{address}: {error}")),
        }
    }
    line(format_args!("{peer}: the hub could not be reached: {}", failures.join("; ")));
}

fn line(text: impl std::fmt::Display) {
    eprintln!("{} {text}", jiff::Timestamp::now());
}
