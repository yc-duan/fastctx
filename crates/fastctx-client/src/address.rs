//! A hub's address as a person writes it (`world-network.md`, 登录与入网):
//! `host:port`, several separated by commas or spaces, a host name looked
//! up here.

use std::net::SocketAddr;

pub async fn hub_addresses(text: &str) -> Result<Vec<SocketAddr>, String> {
    let mut addresses = Vec::new();
    for part in text.split([',', ' ']).map(str::trim).filter(|part| !part.is_empty()) {
        let found = tokio::net::lookup_host(part)
            .await
            .map_err(|error| format!("the hub's address {part}: {error}; write it as host:port"))?;
        for address in found {
            if !addresses.contains(&address) {
                addresses.push(address);
            }
        }
    }
    if addresses.is_empty() {
        return Err("write the hub's address as host:port".to_string());
    }
    Ok(addresses)
}
