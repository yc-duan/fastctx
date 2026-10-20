//! Plain requests to the open web, for new versions on GitHub: over HTTPS
//! checked against the web's public certificate authorities, by the proxy
//! chosen in Fastctx or else the system's when one is named, as the hub is
//! reached. Plain HTTP is spoken only to this computer itself.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt as _, Empty};
use hyper::Request;
use hyper::body::Incoming;
use hyper::http::Uri;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

/// How long a connection, or a wait for the next bytes, may take.
const PATIENCE: Duration = Duration::from_secs(30);
/// Redirects followed: GitHub sends a download through one or two.
const REDIRECTS: usize = 5;

/// A response being read.
pub struct Response {
    pub status: u16,
    /// The body's length, when the server says it.
    pub length: Option<u64>,
    /// Where it came from, after any redirects.
    pub url: String,
    body: Incoming,
}

impl Response {
    /// The body's next bytes; none at its end.
    pub async fn chunk(&mut self) -> Result<Option<Bytes>, String> {
        loop {
            let frame = tokio::time::timeout(PATIENCE, self.body.frame())
                .await
                .map_err(|_| format!("{} sent nothing for {} s", self.url, PATIENCE.as_secs()))?;
            match frame {
                None => return Ok(None),
                Some(Err(error)) => return Err(format!("reading {}: {error}", self.url)),
                Some(Ok(frame)) => {
                    if let Ok(data) = frame.into_data()
                        && !data.is_empty()
                    {
                        return Ok(Some(data));
                    }
                }
            }
        }
    }

    /// The whole body, refused past `limit` bytes.
    pub async fn bytes(mut self, limit: usize) -> Result<Vec<u8>, String> {
        let mut all = Vec::new();
        while let Some(chunk) = self.chunk().await? {
            if all.len() + chunk.len() > limit {
                return Err(format!("{} sent more than {limit} bytes", self.url));
            }
            all.extend_from_slice(&chunk);
        }
        Ok(all)
    }
}

/// Gets `url`, following redirects; `from` asks for the body from that
/// byte on. A status other than success is the caller's to read.
pub async fn get(url: &str, accept: &str, from: u64) -> Result<Response, String> {
    let mut url = url.to_string();
    for _ in 0..=REDIRECTS {
        let uri: Uri = url.parse().map_err(|error| format!("{url}: {error}"))?;
        let response = once(&uri, accept, from).await?;
        let status = response.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = response
                .headers()
                .get(hyper::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| format!("{url} redirected without saying where"))?;
            url = resolve(&uri, location)?;
            continue;
        }
        let length = response
            .headers()
            .get(hyper::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok());
        return Ok(Response { status, length, url, body: response.into_body() });
    }
    Err(format!("{url} redirected more than {REDIRECTS} times"))
}

/// Where a redirect's `Location` points from `base`.
fn resolve(base: &Uri, location: &str) -> Result<String, String> {
    if location.starts_with("https://") || location.starts_with("http://") {
        return Ok(location.to_string());
    }
    let authority = base.authority().ok_or("a redirect from a URL without a host")?;
    let scheme = base.scheme_str().unwrap_or("https");
    if location.starts_with('/') {
        return Ok(format!("{scheme}://{authority}{location}"));
    }
    Err(format!("a redirect to {location}, which is not a URL"))
}

async fn once(uri: &Uri, accept: &str, from: u64) -> Result<hyper::Response<Incoming>, String> {
    let host = uri.host().ok_or_else(|| format!("{uri} has no host"))?.trim_matches(['[', ']']).to_string();
    let loopback = host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
    let secure = match uri.scheme_str() {
        Some("https") => true,
        Some("http") if loopback => false,
        _ => return Err(format!("{uri}: only HTTPS is spoken to the web")),
    };
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
    let tcp = tokio::time::timeout(PATIENCE, reach(&host, port, loopback))
        .await
        .map_err(|_| format!("{host}:{port} did not answer within {} s", PATIENCE.as_secs()))??;
    let path = uri.path_and_query().map_or("/", |path| path.as_str());
    if secure {
        let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|error| format!("{host}: {error}"))?;
        let tls = tokio::time::timeout(PATIENCE, tokio_rustls::TlsConnector::from(config()).connect(name, tcp))
            .await
            .map_err(|_| format!("TLS with {host} took longer than {} s", PATIENCE.as_secs()))?
            .map_err(|error| format!("TLS with {host}: {error}"))?;
        request(tls, &host, port, path, accept, from).await
    } else {
        request(tcp, &host, port, path, accept, from).await
    }
}

/// A TCP connection to `host`, through the system's proxy when it names one
/// that `NO_PROXY` does not exempt the host from.
async fn reach(host: &str, port: u16, loopback: bool) -> Result<TcpStream, String> {
    if !loopback && let Some(proxy) = crate::proxy::system_for(host) {
        return crate::proxy::connect_to(&proxy, &format!("{host}:{port}")).await;
    }
    TcpStream::connect((host, port)).await.map_err(|error| format!("connecting to {host}:{port}: {error}"))
}

async fn request<IO>(
    io: IO,
    host: &str,
    port: u16,
    path: &str,
    accept: &str,
    from: u64,
) -> Result<hyper::Response<Incoming>, String>
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(io))
        .await
        .map_err(|error| format!("HTTP with {host}: {error}"))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let authority = if port == 443 || port == 80 { host.to_string() } else { format!("{host}:{port}") };
    let mut builder = Request::get(path)
        .header(hyper::header::HOST, authority)
        .header(hyper::header::USER_AGENT, concat!("fastctx/", env!("CARGO_PKG_VERSION")))
        .header(hyper::header::ACCEPT, accept)
        .header(hyper::header::ACCEPT_ENCODING, "identity");
    if from > 0 {
        builder = builder.header(hyper::header::RANGE, format!("bytes={from}-"));
    }
    let request = builder.body(Empty::<Bytes>::new()).map_err(|error| error.to_string())?;
    tokio::time::timeout(PATIENCE, sender.send_request(request))
        .await
        .map_err(|_| format!("{host} did not answer within {} s", PATIENCE.as_secs()))?
        .map_err(|error| format!("asking {host}: {error}"))
}

/// TLS checked against the public authorities Mozilla trusts, carried in
/// the program, so a server or phone without a certificate store of its own
/// checks GitHub the same way.
pub(crate) fn config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
            let mut config = rustls::ClientConfig::builder_with_provider(crate::tls::provider())
                .with_safe_default_protocol_versions()
                .expect("the ring provider supports the default versions")
                .with_root_certificates(roots)
                .with_no_client_auth();
            config.alpn_protocols = vec![b"http/1.1".to_vec()];
            Arc::new(config)
        })
        .clone()
}
