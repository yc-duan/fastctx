//! HTTP releases and the existing WebSockets share the root's TLS listener.

use std::convert::Infallible;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use fastctx_net::wire;
use fastctx_types::kernel::{AARCH64_ARCHIVE, RELEASE_PATH, SUMS, X86_64_ARCHIVE, stable_version};
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

type Body = UnsyncBoxBody<Bytes, io::Error>;
type TlsStream = tokio_rustls::server::TlsStream<TcpStream>;

pub(crate) async fn serve(hub: Arc<Hub>, tls: tokio_rustls::TlsAcceptor, tcp: TcpStream, peer: SocketAddr) {
    // Keep the existing initial handshake budget. File transfer itself has
    // no deadline: a slow download must not be cut off by a session policy.
    let deadline = Instant::now() + wire::SILENCE;
    let stream = match tokio::time::timeout_at(deadline, tls.accept(tcp)).await {
        Ok(Ok(stream)) => stream,
        _ => return,
    };
    let mut binding = [0; 32];
    if stream.get_ref().1.export_keying_material(&mut binding, wire::EXPORTER_LABEL, None).is_err() {
        return;
    }
    let service = service_fn(move |request| respond(hub.clone(), request, peer, binding, deadline));
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder.timer(TokioTimer::new()).header_read_timeout(deadline.saturating_duration_since(Instant::now()));
    // keep_alive(false) rewrites Connection: Upgrade and breaks WebSockets.
    let _ = builder.serve_connection(TokioIo::new(stream), service).with_upgrades().await;
}

async fn respond(
    hub: Arc<Hub>,
    mut request: Request<Incoming>,
    peer: SocketAddr,
    binding: [u8; 32],
    deadline: Instant,
) -> Result<Response<Body>, Infallible> {
    let path = request.uri().path().to_owned();
    if path == wire::CONTROL_PATH || path == wire::DATA_PATH {
        let upgrade = hyper::upgrade::on(&mut request);
        let response = match tokio_tungstenite::tungstenite::handshake::server::create_response(&request.map(|_| ())) {
            Ok(response) => response.map(|_| full(Bytes::new())),
            Err(_) => return Ok(text(StatusCode::BAD_REQUEST, "Invalid WebSocket upgrade")),
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
            if let Ok(Ok((socket, hello))) = tokio::time::timeout_at(deadline, opened).await {
                crate::connected(hub, socket, path, binding, hello, peer).await;
            }
        });
        return Ok(response);
    }
    if request.uri().query().is_some() || !matches!(*request.method(), Method::GET | Method::HEAD) {
        return Ok(text(StatusCode::NOT_FOUND, "Not found"));
    }
    let Some((version, name)) = path.strip_prefix(&format!("{RELEASE_PATH}/v")).and_then(|rest| rest.split_once('/'))
    else {
        return Ok(text(StatusCode::NOT_FOUND, "Not found"));
    };
    if !stable_version(version) {
        return Ok(text(StatusCode::NOT_FOUND, "Not found"));
    }
    let gzip = match name {
        SUMS => false,
        X86_64_ARCHIVE | AARCH64_ARCHIVE => true,
        _ => return Ok(text(StatusCode::NOT_FOUND, "Not found")),
    };
    let (version, name) = (version.to_owned(), name.to_owned());
    let releases = hub.releases.clone();
    let opened = tokio::task::spawn_blocking(move || releases.file(&version, &name)).await;
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
    let body = if *request.method() == Method::HEAD {
        full(Bytes::new())
    } else {
        let stream = ReaderStream::new(tokio::fs::File::from_std(file)).map_ok(Frame::data);
        StreamBody::new(stream).boxed_unsync()
    };
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", if gzip { "application/gzip" } else { "text/plain; charset=utf-8" })
        .header("Content-Length", length)
        .body(body)
        .expect("fixed release response headers"))
}

fn full(bytes: Bytes) -> Body {
    Full::new(bytes).map_err(|never| match never {}).boxed_unsync()
}

fn text(status: StatusCode, text: &'static str) -> Response<Body> {
    Response::builder().status(status).body(full(Bytes::from_static(text.as_bytes()))).expect("fixed HTTP status")
}
