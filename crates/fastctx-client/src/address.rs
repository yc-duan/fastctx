//! A hub's address as a person writes it (`world-network.md`, 登录与入网):
//! `host:port`, several separated by commas or spaces, a host name looked
//! up here. What a Chinese or Japanese keyboard types for the ASCII signs
//! (a full-width colon, comma or digit, the ideographic full stop, comma
//! and space) is read as those signs; a scheme written before the address
//! and a path after it are left out; an address with no port is at the port
//! a hub made from the application listens on.

use std::net::{IpAddr, SocketAddr};

/// The port a hub made from the application listens on.
pub const HUB_PORT: u16 = 7443;

pub async fn hub_addresses(text: &str) -> Result<Vec<SocketAddr>, String> {
    let mut addresses = Vec::new();
    for part in plain(text).split([',', ' ', ';']).map(str::trim).filter(|part| !part.is_empty()) {
        let written = with_port(part);
        let found = tokio::net::lookup_host(written.as_str())
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

/// `text` with the full-width forms of the ASCII signs, and the ideographic
/// full stop, comma and space, as the ASCII they stand for.
fn plain(text: &str) -> String {
    text.chars()
        .map(|sign| match sign {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(sign as u32 - 0xFEE0).unwrap_or(sign),
            '\u{3000}' => ' ',
            '\u{3001}' => ',',
            '\u{3002}' | '\u{FF61}' => '.',
            _ => sign,
        })
        .collect()
}

/// One address with no scheme or path, and with the hub's port where none
/// is written.
fn with_port(part: &str) -> String {
    let part = part.split_once("://").map_or(part, |(_, rest)| rest);
    let part = part.split('/').next().unwrap_or(part).trim_end_matches(':');
    if part.parse::<SocketAddr>().is_ok() {
        return part.to_string();
    }
    if let Ok(ip) = part.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>() {
        return SocketAddr::new(ip, HUB_PORT).to_string();
    }
    match part.rsplit_once(':') {
        Some((_, port)) if port.parse::<u16>().is_ok() => part.to_string(),
        _ => format!("{part}:{HUB_PORT}"),
    }
}
