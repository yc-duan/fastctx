//! Staggered IPv6/IPv4 attempts, each bounded independently.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::Duration;

use fastctx_types::update::ReachFailure;
use tokio::net::TcpStream;
use tokio::task::JoinSet;

use super::{Failure, PATIENCE};

pub(super) async fn connect(host: &str, port: u16) -> Result<TcpStream, Failure> {
    let resolved = tokio::time::timeout(PATIENCE, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| Failure::new(ReachFailure::Dns, format!("resolving {host} timed out")))?
        .map_err(|e| Failure::new(ReachFailure::Dns, format!("resolving {host}: {e}")))?;
    addresses(host, port, resolved.collect()).await
}

pub(super) async fn addresses(host: &str, port: u16, mut addresses: Vec<SocketAddr>) -> Result<TcpStream, Failure> {
    addresses.dedup();
    let v6_first = addresses.first().is_some_and(SocketAddr::is_ipv6);
    let (mut first, mut second): (VecDeque<_>, VecDeque<_>) =
        addresses.into_iter().partition(|address| address.is_ipv6() == v6_first);
    let mut ordered = VecDeque::new();
    while !first.is_empty() || !second.is_empty() {
        ordered.extend(first.pop_front());
        ordered.extend(second.pop_front());
    }
    let mut attempts = JoinSet::new();
    let deadline = tokio::time::sleep(PATIENCE);
    tokio::pin!(deadline);
    let mut next = tokio::time::Instant::now();
    let mut last = format!("{host} resolved to no addresses");
    loop {
        if attempts.is_empty() && ordered.is_empty() {
            return Err(Failure::new(ReachFailure::Connect, last));
        }
        tokio::select! {
            _ = &mut deadline => return Err(Failure::new(ReachFailure::Timeout, format!("connecting to {host}:{port} timed out"))),
            _ = tokio::time::sleep_until(next), if !ordered.is_empty() => {
                let address = ordered.pop_front().expect("an address remains");
                attempts.spawn(async move {
                    tokio::time::timeout(PATIENCE, TcpStream::connect(address)).await
                        .map_err(|_| format!("connecting to {address} timed out"))?
                        .map_err(|e| format!("connecting to {address}: {e}"))
                });
                next = tokio::time::Instant::now() + Duration::from_millis(250);
            }
            result = attempts.join_next(), if !attempts.is_empty() => {
                match result.expect("an attempt remains") {
                    Ok(Ok(stream)) => return Ok(stream),
                    Ok(Err(why)) => last = why,
                    Err(error) => last = error.to_string(),
                }
                if attempts.is_empty() { next = tokio::time::Instant::now(); }
            }
        }
    }
}
