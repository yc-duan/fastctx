//! HTTP CONNECT through the system or environment proxy, for the `system` network mode.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A proxy the `system` mode should use.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProxyConfig {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) credentials: Option<(String, String)>,
    /// Where the setting came from, for status lines.
    pub(crate) source: String,
}

impl ProxyConfig {
    pub(crate) fn describe(&self) -> String {
        format!("http://{}:{} ({})", self.host, self.port, self.source)
    }
}

/// The proxy to use for `target_host`, if any: `HTTPS_PROXY` / `https_proxy` / `ALL_PROXY`
/// first, then the Windows Internet Settings proxy; `NO_PROXY` exempts hosts.
pub(crate) fn discover(target_host: &str) -> Option<ProxyConfig> {
    if no_proxy_matches(target_host) {
        return None;
    }
    for name in ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        if let Some(value) = std::env::var_os(name) {
            let value = value.to_string_lossy().trim().to_string();
            if value.is_empty() {
                continue;
            }
            if let Some(config) = parse_proxy_url(&value, name) {
                return Some(config);
            }
        }
    }
    windows_proxy()
}

fn no_proxy_matches(target_host: &str) -> bool {
    for name in ["NO_PROXY", "no_proxy"] {
        let Some(value) = std::env::var_os(name) else {
            continue;
        };
        for entry in value.to_string_lossy().split(',') {
            let entry = entry.trim().trim_start_matches('.');
            if entry.is_empty() {
                continue;
            }
            if entry == "*" || target_host == entry || target_host.ends_with(&format!(".{entry}")) {
                return true;
            }
        }
    }
    false
}

fn parse_proxy_url(value: &str, source: &str) -> Option<ProxyConfig> {
    let text = if value.contains("://") {
        value.to_string()
    } else {
        format!("http://{value}")
    };
    let url = url::Url::parse(&text).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_string();
    let port = url
        .port()
        .unwrap_or(if url.scheme() == "https" { 443 } else { 80 });
    let credentials = if url.username().is_empty() {
        None
    } else {
        Some((
            percent_decode(url.username()),
            url.password().map(percent_decode).unwrap_or_default(),
        ))
    };
    Some(ProxyConfig {
        host,
        port,
        credentials,
        source: source.to_string(),
    })
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len() + 1
            && let Some(value) = bytes
                .get(index + 1..index + 3)
                .and_then(|pair| std::str::from_utf8(pair).ok())
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        {
            output.push(value);
            index += 3;
            continue;
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

#[cfg(windows)]
fn windows_proxy() -> Option<ProxyConfig> {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey, RegGetValueW,
        RegOpenKeyExW,
    };

    let path: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\0"
        .encode_utf16()
        .collect();
    let mut key = std::ptr::null_mut();
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut key) } != 0 {
        return None;
    }
    let read_dword = |name: &str| -> Option<u32> {
        let name: Vec<u16> = format!("{name}\0").encode_utf16().collect();
        let mut value: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let status = unsafe {
            RegGetValueW(
                key,
                std::ptr::null(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut value as *mut u32).cast(),
                &mut size,
            )
        };
        (status == 0).then_some(value)
    };
    let read_string = |name: &str| -> Option<String> {
        let name: Vec<u16> = format!("{name}\0").encode_utf16().collect();
        let mut size: u32 = 0;
        let status = unsafe {
            RegGetValueW(
                key,
                std::ptr::null(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if status != 0 || size == 0 {
            return None;
        }
        let mut buffer = vec![0_u16; (size as usize).div_ceil(2)];
        let status = unsafe {
            RegGetValueW(
                key,
                std::ptr::null(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != 0 {
            return None;
        }
        let length = buffer
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..length]))
    };
    let enabled = read_dword("ProxyEnable").unwrap_or(0) == 1;
    let server = read_string("ProxyServer");
    unsafe { RegCloseKey(key) };
    if !enabled {
        return None;
    }
    let server = server?;
    // "host:port" or "http=host:port;https=host:port;…".
    let entry = if server.contains('=') {
        server
            .split(';')
            .filter_map(|part| part.split_once('='))
            .find(|(scheme, _)| {
                scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http")
            })
            .map(|(_, value)| value.to_string())?
    } else {
        server
    };
    parse_proxy_url(entry.trim(), "Windows Internet Settings")
}

#[cfg(not(windows))]
fn windows_proxy() -> Option<ProxyConfig> {
    None
}

/// Opens a TCP tunnel to `target` through `proxy`.
pub(crate) async fn connect_through(
    proxy: &ProxyConfig,
    target_host: &str,
    target_port: u16,
) -> Result<TcpStream, String> {
    let mut stream = TcpStream::connect((proxy.host.as_str(), proxy.port))
        .await
        .map_err(|error| {
            format!(
                "cannot reach the proxy {}:{}: {error}",
                proxy.host, proxy.port
            )
        })?;
    let mut request = format!(
        "CONNECT {target_host}:{target_port} HTTP/1.1\r\nHost: {target_host}:{target_port}\r\nProxy-Connection: keep-alive\r\n"
    );
    if let Some((user, password)) = &proxy.credentials {
        request.push_str(&format!(
            "Proxy-Authorization: Basic {}\r\n",
            crate::world::crypto::b64_encode(format!("{user}:{password}").as_bytes())
        ));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| format!("cannot write to the proxy: {error}"))?;
    let mut response = Vec::new();
    let mut byte = [0_u8; 1];
    while !response.ends_with(b"\r\n\r\n") {
        let read = stream
            .read(&mut byte)
            .await
            .map_err(|error| format!("cannot read the proxy's answer: {error}"))?;
        if read == 0 {
            return Err("the proxy closed the connection before answering CONNECT".to_string());
        }
        response.push(byte[0]);
        if response.len() > 16 * 1024 {
            return Err("the proxy's answer to CONNECT is unreasonably long".to_string());
        }
    }
    let status_line = String::from_utf8_lossy(&response)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    if status != 200 {
        return Err(format!("the proxy refused CONNECT: {status_line}"));
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::parse_proxy_url;

    #[test]
    fn proxy_urls_parse_with_and_without_scheme_and_credentials() {
        let plain = parse_proxy_url("127.0.0.1:7890", "test").unwrap();
        assert_eq!((plain.host.as_str(), plain.port), ("127.0.0.1", 7890));
        let full = parse_proxy_url("http://us%40er:p%3Ass@proxy.local:3128", "test").unwrap();
        assert_eq!(full.port, 3128);
        assert_eq!(
            full.credentials,
            Some(("us@er".to_string(), "p:ss".to_string()))
        );
        assert!(parse_proxy_url("socks5://127.0.0.1:1080", "test").is_none());
    }
}
