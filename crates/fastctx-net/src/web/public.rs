//! Public destinations only, with DNS answers pinned across connection setup.
use super::{Failure, PATIENCE, Response};
use hyper::Uri;
use std::net::IpAddr;

/// Reads one public HTTPS URL. Redirects remain the caller's to validate and log.
/// Proxies receive a validated IP, so they cannot resolve a public-looking name
/// to a private destination. TLS and HTTP still use the original hostname.
pub async fn get(url: &str, accept: &str) -> Result<Response, Failure> {
    let uri: Uri = url.parse().map_err(|e| format!("invalid public URL: {e}"))?;
    if uri.scheme_str() != Some("https") {
        return Err("public web requires HTTPS".into());
    }
    let host = uri.host().ok_or("public URL has no host")?.trim_matches(['[', ']']);
    let port = uri.port_u16().unwrap_or(443);
    let operation = async {
        let addresses: Vec<_> = tokio::net::lookup_host((host, port)).await.map_err(|e| e.to_string())?.collect();
        if addresses.is_empty() || addresses.iter().any(|a| !is_public(a.ip())) {
            return Err(Failure::from("web destination is not public"));
        }
        let tcp = if let Some(proxy) = crate::proxy::system_for(host) {
            let mut last = String::new();
            let mut connected = None;
            for address in &addresses {
                match crate::proxy::connect_to(&proxy, &address.to_string()).await {
                    Ok(stream) => {
                        connected = Some(stream);
                        break;
                    }
                    Err(error) => last = error,
                }
            }
            connected.ok_or(last)?
        } else {
            super::connect::addresses(host, port, addresses).await?
        };
        let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
        let tls =
            tokio_rustls::TlsConnector::from(super::config()).connect(name, tcp).await.map_err(|e| e.to_string())?;
        let response =
            super::request(tls, host, port, uri.path_and_query().map_or("/", |p| p.as_str()), accept, 0, None).await?;
        Ok(Response {
            status: response.status().as_u16(),
            length: None,
            url: url.into(),
            headers: response.headers().clone(),
            body: response.into_body(),
        })
    };
    tokio::time::timeout(PATIENCE, operation).await.map_err(|_| "public web request timed out")?
}
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, _, _] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_broadcast()
                || a == 0
                || a >= 240
                || a == 100 && (64..=127).contains(&b)
                || a == 198 && (b == 18 || b == 19))
        }
        IpAddr::V6(ip) => {
            !ip.is_loopback()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_unique_local()
                && !ip.is_unicast_link_local()
                && ip.to_ipv4_mapped().is_none()
                && ip.segments()[0] & 0xe000 == 0x2000
        }
    }
}
