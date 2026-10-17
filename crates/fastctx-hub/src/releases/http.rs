//! HTTP releases and the existing WebSockets share the root's TLS listener.

use std::convert::Infallible;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use fastctx_net::wire;
use fastctx_types::kernel::{
    AARCH64_ARCHIVE, DARWIN_AARCH64_ARCHIVE, DARWIN_X86_64_ARCHIVE, RELEASE_PATH, SUMS, WINDOWS_X86_64_ARCHIVE,
    X86_64_ARCHIVE, release_version,
};
use futures_util::TryStreamExt as _;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt as _, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_util::io::ReaderStream;

use crate::Hub;

/// Who answers what comes in on the hub's port: the hub, or the new place
/// of a move standing by (`crate::moving::standby`), which serves no
/// releases and takes only the old hub.
#[derive(Clone)]
pub(crate) enum Host {
    Hub(Arc<Hub>),
    Standby(Arc<crate::moving::standby::Standby>),
}

impl Host {
    /// Where what went wrong with a connection is written.
    fn log(&self) -> &crate::log::Log {
        match self {
            Host::Hub(hub) => &hub.log,
            Host::Standby(standby) => standby.log(),
        }
    }
}

type Body = UnsyncBoxBody<Bytes, io::Error>;
type TlsStream = tokio_rustls::server::TlsStream<fastctx_net::pulse::Pulsing<TcpStream>>;

/// The hub's two ways of TLS: TLS 1.3 for everything, and TLS 1.2 for the
/// release files alone, to a downloader that offers nothing newer.
#[derive(Clone)]
pub(crate) struct Tls {
    session: std::sync::Arc<tokio_rustls::rustls::ServerConfig>,
    download: std::sync::Arc<tokio_rustls::rustls::ServerConfig>,
}

impl Tls {
    pub(crate) fn of(certificate: &fastctx_net::tls::HubCertificate) -> Result<Self, String> {
        Ok(Self { session: certificate.server_config()?, download: certificate.download_config()? })
    }
}

pub(crate) async fn serve(host: Host, tls: Tls, tcp: TcpStream, peer: SocketAddr) {
    // Keep the existing initial handshake budget. File transfer itself has
    // no deadline: a slow download must not be cut off by a session policy.
    let deadline = Instant::now() + wire::SILENCE;
    // A connection that never becomes a session or a download is logged
    // with where it came from and why, so a machine that could not get in
    // leaves a trace here as well as in its own log.
    let handshake = async {
        let start = tokio_rustls::LazyConfigAcceptor::new(
            tokio_rustls::rustls::server::Acceptor::default(),
            fastctx_net::pulse::Pulsing::new(tcp),
        )
        .await?;
        // A downloader that stops at TLS 1.2, as Windows 10's own tools
        // do, is answered with the release files and nothing else.
        let session = fastctx_net::tls::offers_tls13(&start.client_hello());
        let config = if session { tls.session.clone() } else { tls.download.clone() };
        start.into_stream(config).await.map(|stream| (stream, session))
    };
    let (stream, session) = match tokio::time::timeout_at(deadline, handshake).await {
        Ok(Ok(accepted)) => accepted,
        Ok(Err(error)) => {
            host.log().line(format_args!("{peer}: the TLS handshake failed: {error}"));
            return;
        }
        Err(_) => {
            host.log().line(format_args!("{peer}: no TLS handshake within {} s", wire::SILENCE.as_secs()));
            return;
        }
    };
    // Only a session is bound to its connection; a download needs no
    // binding, and TLS 1.2 may not give one.
    let mut binding = [0; 32];
    if session && let Err(error) = stream.get_ref().1.export_keying_material(&mut binding, wire::EXPORTER_LABEL, None) {
        host.log().line(format_args!("{peer}: the TLS exporter failed: {error}"));
        return;
    }
    let log = host.clone();
    let service = service_fn(move |request| respond(host.clone(), request, peer, binding, deadline, session));
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder.timer(TokioTimer::new()).header_read_timeout(deadline.saturating_duration_since(Instant::now()));
    // keep_alive(false) rewrites Connection: Upgrade and breaks WebSockets.
    if let Err(error) = builder.serve_connection(TokioIo::new(stream), service).with_upgrades().await {
        log.log().line(format_args!("{peer}: the connection ended before a complete request: {error}"));
    }
}

async fn respond(
    host: Host,
    mut request: Request<Incoming>,
    peer: SocketAddr,
    binding: [u8; 32],
    deadline: Instant,
    session: bool,
) -> Result<Response<Body>, Infallible> {
    let path = request.uri().path().to_owned();
    if !session && (path == wire::CONTROL_PATH || path == wire::DATA_PATH) {
        host.log().line(format_args!("{peer}: refused a session over TLS 1.2 on {path}"));
        return Ok(text(StatusCode::UPGRADE_REQUIRED, "Sessions need TLS 1.3"));
    }
    if path == wire::CONTROL_PATH || path == wire::DATA_PATH {
        let upgrade = hyper::upgrade::on(&mut request);
        let response = match tokio_tungstenite::tungstenite::handshake::server::create_response(&request.map(|_| ())) {
            Ok(response) => response.map(|_| full(Bytes::new())),
            Err(error) => {
                host.log().line(format_args!("{peer}: refused a WebSocket upgrade on {path}: {error}"));
                return Ok(text(StatusCode::BAD_REQUEST, "Invalid WebSocket upgrade"));
            }
        };
        tokio::spawn(async move {
            let opened = async {
                let upgraded = upgrade.await.map_err(|error| error.to_string())?;
                let parts = upgraded.downcast::<TokioIo<TlsStream>>().map_err(|_| "unexpected upgraded transport")?;
                let limit = wire::MAX_DATA_MESSAGE + wire::MAX_CONTROL_FRAME;
                let mut socket = WebSocketStream::from_partially_read(
                    parts.io.into_inner(),
                    parts.read_buf.to_vec(),
                    Role::Server,
                    Some(fastctx_net::dial::config(limit)),
                )
                .await;
                let hello = crate::recv(&mut socket).await?.ok_or("closed before saying hello")?;
                Ok::<_, String>((socket, hello))
            };
            match tokio::time::timeout_at(deadline, opened).await {
                Ok(Ok((socket, hello))) => match host {
                    Host::Hub(hub) => crate::connected(hub, socket, path, binding, hello, peer).await,
                    Host::Standby(standby) => {
                        crate::moving::standby::connected(standby, socket, path, binding, hello, peer).await;
                    }
                },
                Ok(Err(error)) => {
                    host.log().line(format_args!("{peer}: the WebSocket on {path} did not open: {error}"))
                }
                Err(_) => host.log().line(format_args!(
                    "{peer}: no hello on {path} within {} s of the connection",
                    wire::SILENCE.as_secs()
                )),
            }
        });
        return Ok(response);
    }
    let Host::Hub(hub) = host else {
        return Ok(text(StatusCode::NOT_FOUND, "Not found"));
    };
    if request.uri().query().is_some() || !matches!(*request.method(), Method::GET | Method::HEAD) {
        return Ok(text(StatusCode::NOT_FOUND, "Not found"));
    }
    let Some((version, name)) = path.strip_prefix(&format!("{RELEASE_PATH}/v")).and_then(|rest| rest.split_once('/'))
    else {
        return Ok(text(StatusCode::NOT_FOUND, "Not found"));
    };
    if !release_version(version) {
        return Ok(text(StatusCode::NOT_FOUND, "Not found"));
    }
    let kind = match name {
        SUMS => "text/plain; charset=utf-8",
        X86_64_ARCHIVE | AARCH64_ARCHIVE | DARWIN_AARCH64_ARCHIVE | DARWIN_X86_64_ARCHIVE => "application/gzip",
        WINDOWS_X86_64_ARCHIVE => "application/zip",
        _ => return Ok(text(StatusCode::NOT_FOUND, "Not found")),
    };
    let (version, name) = (version.to_owned(), name.to_owned());
    let releases = hub.releases.clone();
    // A Windows or Mac archive placed by hand comes first, as Linux ones do;
    // else the one held among the update packages, checked when it came.
    let held = crate::updates::held_archive(&hub, &version, &name);
    let opened = tokio::task::spawn_blocking(move || match releases.file(&version, &name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound && held.is_some() => {
            std::fs::File::open(held.expect("checked above"))
        }
        opened => opened,
    })
    .await;
    let file = match opened {
        Ok(Ok(file)) => file,
        Ok(Err(error)) => {
            hub.log.line(format_args!("{peer}: kernel download not ready: {path}: {error}"));
            return Ok(text(StatusCode::NOT_FOUND, "Release is not ready"));
        }
        Err(error) => {
            hub.log.line(format_args!("{peer}: kernel download failed: {error}"));
            return Ok(text(StatusCode::INTERNAL_SERVER_ERROR, "Reading release failed"));
        }
    };
    let length = match file.metadata() {
        Ok(metadata) => metadata.len(),
        Err(_) => return Ok(text(StatusCode::INTERNAL_SERVER_ERROR, "Reading release failed")),
    };
    // A download that broke asks for the rest (`world-network.md`, 断开不是
    // 结束): one range from a byte to the end or to a byte. Anything else
    // asked gets the whole file.
    let range = request.headers().get(hyper::header::RANGE).and_then(|asked| asked.to_str().ok()).and_then(range_of);
    let (status, start, end) = match range {
        Some((start, _)) if start >= length => {
            return Ok(Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header("Content-Range", format!("bytes */{length}"))
                .body(full(Bytes::new()))
                .expect("fixed release response headers"));
        }
        Some((start, end)) => (StatusCode::PARTIAL_CONTENT, start, end.map_or(length - 1, |end| end.min(length - 1))),
        None => (StatusCode::OK, 0, length.saturating_sub(1)),
    };
    let sending = if length == 0 { 0 } else { end - start + 1 };
    let body = if *request.method() == Method::HEAD || sending == 0 {
        full(Bytes::new())
    } else {
        use std::io::{Seek as _, SeekFrom};
        use tokio::io::AsyncReadExt as _;
        let mut file = file;
        if file.seek(SeekFrom::Start(start)).is_err() {
            return Ok(text(StatusCode::INTERNAL_SERVER_ERROR, "Reading release failed"));
        }
        let stream = ReaderStream::new(tokio::fs::File::from_std(file).take(sending)).map_ok(Frame::data);
        StreamBody::new(stream).boxed_unsync()
    };
    let mut response = Response::builder()
        .status(status)
        .header("Content-Type", kind)
        .header("Content-Length", sending)
        .header("Accept-Ranges", "bytes");
    if status == StatusCode::PARTIAL_CONTENT {
        response = response.header("Content-Range", format!("bytes {start}-{end}/{length}"));
    }
    Ok(response.body(body).expect("fixed release response headers"))
}

/// The one range `bytes=START-` or `bytes=START-END` asks for; none for any
/// other form, which is answered with the whole file.
fn range_of(asked: &str) -> Option<(u64, Option<u64>)> {
    let (start, end) = asked.strip_prefix("bytes=")?.split_once('-')?;
    let start = start.trim().parse().ok()?;
    let end = match end.trim() {
        "" => None,
        end => Some(end.parse().ok()?),
    };
    if end.is_some_and(|end| end < start) {
        return None;
    }
    Some((start, end))
}

fn full(bytes: Bytes) -> Body {
    Full::new(bytes).map_err(|never| match never {}).boxed_unsync()
}

fn text(status: StatusCode, text: &'static str) -> Response<Body> {
    Response::builder().status(status).body(full(Bytes::from_static(text.as_bytes()))).expect("fixed HTTP status")
}
