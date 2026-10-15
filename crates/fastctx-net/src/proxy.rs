//! The system's proxy, for when a connection pinned to the physical card
//! cannot reach the hub: an HTTP proxy, taken from `HTTPS_PROXY` or
//! `ALL_PROXY`, or on Windows from the Internet Settings, reached with
//! `CONNECT`.

use std::net::SocketAddr;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proxy {
    pub host: String,
    pub port: u16,
    pub credentials: Option<(String, String)>,
    /// Where the setting came from.
    pub source: String,
}

impl std::fmt::Display for Proxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the proxy {}:{} ({})", self.host, self.port, self.source)
    }
}

/// The proxy the system would use for `target`, if any. `NO_PROXY` exempts
/// a target it names.
pub fn system(target: SocketAddr) -> Option<Proxy> {
    system_for(&target.ip().to_string())
}

/// The proxy the system would use for `host`, a name or an address, if any.
/// `NO_PROXY` exempts a host it names, and a name under a domain it names.
pub fn system_for(host: &str) -> Option<Proxy> {
    for name in ["NO_PROXY", "no_proxy"] {
        if let Ok(list) = std::env::var(name)
            && list.split(',').map(str::trim).filter(|entry| !entry.is_empty()).any(|entry| {
                let domain = entry.trim_start_matches('.');
                entry == "*"
                    || host.eq_ignore_ascii_case(domain)
                    || host.to_ascii_lowercase().ends_with(&format!(".{}", domain.to_ascii_lowercase()))
            })
        {
            return None;
        }
    }
    for name in ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        if let Ok(value) = std::env::var(name)
            && let Some(proxy) = parse(value.trim(), name)
        {
            return Some(proxy);
        }
    }
    windows::internet_settings()
}

/// An HTTP proxy URL, with or without its scheme: `http://user:password@host:port`.
/// Other schemes, such as SOCKS, are not spoken.
fn parse(text: &str, source: &str) -> Option<Proxy> {
    if text.is_empty() {
        return None;
    }
    let (scheme, rest) = text.split_once("://").unwrap_or(("http", text));
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let rest = rest.split('/').next().unwrap_or(rest);
    let (credentials, address) = match rest.rsplit_once('@') {
        Some((user, address)) => {
            let (name, password) = user.split_once(':').unwrap_or((user, ""));
            (Some((unescape(name), unescape(password))), address)
        }
        None => (None, rest),
    };
    let (host, port) = if let Some(bracketed) = address.strip_prefix('[') {
        let (host, after) = bracketed.split_once(']')?;
        (host.to_string(), after.strip_prefix(':').map(str::parse).transpose().ok()?.unwrap_or(default_port))
    } else {
        match address.rsplit_once(':') {
            Some((host, port)) => (host.to_string(), port.parse().ok()?),
            None => (address.to_string(), default_port),
        }
    };
    (!host.is_empty()).then(|| Proxy { host, port, credentials, source: source.to_string() })
}

/// Undoes URL percent-escapes.
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(value) = bytes
                .get(index + 1..index + 3)
                .and_then(|pair| std::str::from_utf8(pair).ok())
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        {
            out.push(value);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A TCP connection to `target` through `proxy`.
pub async fn connect(proxy: &Proxy, target: SocketAddr) -> Result<TcpStream, String> {
    connect_to(proxy, &target.to_string()).await
}

/// A TCP connection to `target`, `host:port`, through `proxy`.
pub async fn connect_to(proxy: &Proxy, target: &str) -> Result<TcpStream, String> {
    let mut stream = TcpStream::connect((proxy.host.as_str(), proxy.port))
        .await
        .map_err(|error| format!("reaching {proxy}: {error}"))?;
    let mut request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
    if let Some((name, password)) = &proxy.credentials {
        request.push_str(&format!("Proxy-Authorization: Basic {}\r\n", STANDARD.encode(format!("{name}:{password}"))));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await.map_err(|error| format!("writing to {proxy}: {error}"))?;
    // The answer is a status line and headers, read a byte at a time so
    // nothing of the tunnel after them is taken.
    let mut answer = Vec::new();
    let mut byte = [0_u8; 1];
    while !answer.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte).await {
            Ok(0) => return Err(format!("{proxy} closed the connection before answering")),
            Ok(_) => answer.push(byte[0]),
            Err(error) => return Err(format!("reading from {proxy}: {error}")),
        }
    }
    let status = String::from_utf8_lossy(&answer).lines().next().unwrap_or_default().to_string();
    if status.split_whitespace().nth(1) != Some("200") {
        return Err(format!("{proxy} refused the connection: {status}"));
    }
    Ok(stream)
}

#[cfg(windows)]
mod windows {
    use super::{Proxy, parse};

    /// The proxy in the user's Internet Settings, when it is switched on.
    pub fn internet_settings() -> Option<Proxy> {
        const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
        let enabled =
            fastctx_platform::registry::dword_value(fastctx_platform::registry::Hive::CurrentUser, KEY, "ProxyEnable")
                .ok()
                .flatten()
                .unwrap_or(0);
        if enabled != 1 {
            return None;
        }
        let server =
            fastctx_platform::registry::string_value(fastctx_platform::registry::Hive::CurrentUser, KEY, "ProxyServer")
                .ok()
                .flatten()?
                .to_string_lossy()
                .into_owned();
        // "host:port", or "http=host:port;https=host:port;…" per scheme.
        let entry = if server.contains('=') {
            let entries: Vec<(&str, &str)> = server.split(';').filter_map(|part| part.split_once('=')).collect();
            let pick = |scheme: &str| {
                entries.iter().find(|(name, _)| name.eq_ignore_ascii_case(scheme)).map(|(_, value)| *value)
            };
            pick("https").or_else(|| pick("http"))?.to_string()
        } else {
            server
        };
        parse(entry.trim(), "Windows Internet Settings")
    }
}

#[cfg(not(windows))]
mod windows {
    pub fn internet_settings() -> Option<super::Proxy> {
        None
    }
}
