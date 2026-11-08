//! Public destinations only: a name resolved here must not lead to a private
//! address, and a connection without a proxy goes only to the addresses
//! checked.
use super::{Failure, PATIENCE, Response};
use hyper::Uri;
use std::net::IpAddr;

/// Reads one public HTTPS URL. Redirects remain the caller's to validate and
/// log. Before anything is sent, an address the name leads to is seen to be
/// public: the addresses this machine resolves it to, every one public; or,
/// where this machine cannot see them (it cannot resolve the name and a
/// proxy will, or a fake-ip proxy such as Clash answers every name with its
/// own block, 198.18.0.0/15, and every address it gives is in it), what
/// public DNS answers (`super::public_dns`), and a name only a local network
/// knows (`nas`, `router.lan`) is refused without asking. An address written
/// in the URL must be public itself. Through a proxy the name is handed on
/// for the proxy to resolve, as domain-routing proxies and networks whose
/// own DNS answers wrongly need; a proxy that resolves a name otherwise than
/// the view checked is the one way left past this check.
/// Without a proxy, the connection goes to the addresses checked. TLS and
/// HTTP use the original hostname either way.
pub async fn get(url: &str, accept: &str) -> Result<Response, Failure> {
    let uri: Uri = url.parse().map_err(|e| format!("invalid public URL: {e}"))?;
    if uri.scheme_str() != Some("https") {
        return Err("public web requires HTTPS".into());
    }
    let host = uri.host().ok_or("public URL has no host")?.trim_matches(['[', ']']);
    let port = uri.port_u16().unwrap_or(443);
    let written = host.parse::<IpAddr>().ok();
    let proxy = crate::proxy::system_for(host);
    let operation = async {
        if written.is_some_and(|ip| !is_public(ip)) {
            return Err(Failure::from("web destination is not public"));
        }
        let addresses: Vec<_> = match tokio::net::lookup_host((host, port)).await {
            Ok(addresses) => addresses.collect(),
            Err(_) if proxy.is_some() && written.is_none() => Vec::new(),
            Err(error) => return Err(Failure::from(error.to_string())),
        };
        let fakes = addresses.iter().filter(|address| fake(address.ip())).count();
        let mixed = fakes > 0 && fakes < addresses.len();
        if mixed || addresses.iter().any(|address| !is_public(address.ip()) && !fake(address.ip())) {
            return Err(Failure::from("web destination is not public"));
        }
        let unseen = written.is_none() && (addresses.is_empty() || fakes > 0);
        if unseen {
            if local_name(host) {
                return Err(Failure::from("web destination is a name of a local network, not a public one"));
            }
            let public = super::public_dns::addresses(host)
                .await
                .map_err(|why| format!("could not learn from public DNS where {host} leads: {why}"))?;
            if public.is_empty() {
                return Err(Failure::from("public DNS knows no address for the web destination"));
            }
            if public.iter().any(|ip| !is_public(*ip)) {
                return Err(Failure::from("web destination is not public"));
            }
        }
        let tcp = match &proxy {
            Some(proxy) => {
                let target = if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") };
                crate::proxy::connect_to(proxy, &target).await?
            }
            None if addresses.is_empty() => return Err(Failure::from("the web destination has no address")),
            None => super::connect::addresses(host, port, addresses).await?,
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

/// Whether `host` is a name only a local network gives an address: one
/// without a dot, or under a suffix set aside for such names.
fn local_name(host: &str) -> bool {
    const LOCAL: &[&str] = &[
        ".local",
        ".localhost",
        ".localdomain",
        ".lan",
        ".home",
        ".home.arpa",
        ".internal",
        ".intranet",
        ".corp",
        ".private",
    ];
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    !host.contains('.') || LOCAL.iter().any(|suffix| host.ends_with(suffix))
}

/// Whether `ip` is in the block set aside for benchmarking networks
/// (198.18.0.0/15), which fake-ip proxies hand out for every name.
fn fake(ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V4(ip) if ip.octets()[0] == 198 && (ip.octets()[1] == 18 || ip.octets()[1] == 19))
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
