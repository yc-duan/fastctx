//! Single-destination JSON requests using the World's web transport.

use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Uri};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};

use super::{Failure, PATIENCE, Response, config, reach};

/// Posts JSON to exactly `url`. Redirects are returned to the caller, never
/// followed with credentials. TLS roots and proxy selection match web downloads.
pub async fn json(url: &str, headers: &[(&str, &str)], bytes: Vec<u8>) -> Result<Response, Failure> {
    let uri: Uri = url.parse().map_err(|error| format!("invalid URL: {error}"))?;
    let host = uri.host().ok_or("URL has no host")?.trim_matches(['[', ']']).to_string();
    let loopback = host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
    let secure = match uri.scheme_str() {
        Some("https") => true,
        Some("http") if loopback => false,
        _ => return Err("only HTTPS is spoken to the web".into()),
    };
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
    let authority = uri.authority().ok_or("URL has no authority")?.as_str();
    let path = uri.path_and_query().map_or("/", |value| value.as_str());
    let request = async {
        let tcp = reach(&host, port, loopback).await?;
        if secure {
            let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|e| e.to_string())?;
            let tls = tokio_rustls::TlsConnector::from(config()).connect(name, tcp).await.map_err(|e| e.to_string())?;
            send(tls, authority, path, headers, bytes).await
        } else {
            send(tcp, authority, path, headers, bytes).await
        }
    };
    let response = tokio::time::timeout(PATIENCE, request).await.map_err(|_| "web request timed out")??;
    Ok(Response {
        status: response.status().as_u16(),
        length: response
            .headers()
            .get(hyper::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok()),
        url: url.into(),
        headers: response.headers().clone(),
        body: response.into_body(),
    })
}

async fn send<IO>(
    io: IO,
    authority: &str,
    path: &str,
    headers: &[(&str, &str)],
    bytes: Vec<u8>,
) -> Result<hyper::Response<hyper::body::Incoming>, Failure>
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(TokioIo::new(io)).await.map_err(|e| e.to_string())?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let mut request = Request::post(path)
        .header(hyper::header::HOST, authority)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .header(hyper::header::ACCEPT, "application/json")
        .header(hyper::header::USER_AGENT, concat!("fastctx/", env!("CARGO_PKG_VERSION")));
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    sender
        .send_request(request.body(Full::new(Bytes::from(bytes))).map_err(|e| e.to_string())?)
        .await
        .map_err(|e| e.to_string().into())
}
