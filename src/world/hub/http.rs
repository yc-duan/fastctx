//! The hub's HTTP face: a WebSocket upgrade on the World path, a small page on `/`, and 404
//! for everything else. To anything that probes it, the hub is a self-hosted web service.

use super::Hub;
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::{
    CONNECTION, CONTENT_TYPE, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, SEC_WEBSOCKET_VERSION,
    UPGRADE,
};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};

const LANDING_PAGE: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>FastCtx World hub</title></head><body style=\"font-family:system-ui;margin:3rem\"><h1>FastCtx World hub</h1><p>This server links one person's machines into a World. It has no public pages.</p></body></html>";

/// WebSocket limits for the control connection.
pub(crate) fn websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(crate::world::wire::MAX_FRAME_BYTES))
        .max_frame_size(Some(crate::world::wire::MAX_FRAME_BYTES))
}

/// Serves one accepted connection until the peer closes it or a WebSocket takes it over.
pub(crate) async fn serve_connection<IO>(hub: Arc<Hub>, io: IO, binding: Vec<u8>, peer: SocketAddr)
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let service = hyper::service::service_fn(move |request| {
        let hub = Arc::clone(&hub);
        let binding = binding.clone();
        async move { Ok::<_, Infallible>(handle(hub, binding, peer, request)) }
    });
    if let Err(error) = hyper::server::conn::http1::Builder::new()
        .keep_alive(true)
        .serve_connection(TokioIo::new(io), service)
        .with_upgrades()
        .await
    {
        // A client that hangs up mid-request is ordinary Internet weather, not a hub fault.
        if !error.is_incomplete_message() {
            super::log(format!("{peer}: http connection ended: {error}"));
        }
    }
}

fn handle(
    hub: Arc<Hub>,
    binding: Vec<u8>,
    peer: SocketAddr,
    mut request: Request<Incoming>,
) -> Response<Full<Bytes>> {
    if request.uri().path() == crate::world::WS_PATH {
        return match websocket_accept_key(&request) {
            Ok(accept) => {
                let on_upgrade = hyper::upgrade::on(&mut request);
                tokio::spawn(async move {
                    match on_upgrade.await {
                        Ok(upgraded) => {
                            let socket = WebSocketStream::from_raw_socket(
                                TokioIo::new(upgraded),
                                Role::Server,
                                Some(websocket_config()),
                            )
                            .await;
                            super::session::serve(hub, socket, binding, peer).await;
                        }
                        Err(error) => {
                            super::log(format!("{peer}: websocket upgrade failed: {error}"))
                        }
                    }
                });
                Response::builder()
                    .status(StatusCode::SWITCHING_PROTOCOLS)
                    .header(UPGRADE, "websocket")
                    .header(CONNECTION, "Upgrade")
                    .header(SEC_WEBSOCKET_ACCEPT, accept)
                    .body(Full::new(Bytes::new()))
                    .expect("a static 101 response builds")
            }
            Err(message) => text_response(StatusCode::BAD_REQUEST, message),
        };
    }
    match (request.method(), request.uri().path()) {
        (&Method::GET, "/") | (&Method::HEAD, "/") => Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Full::new(Bytes::from_static(LANDING_PAGE.as_bytes())))
            .expect("a static 200 response builds"),
        _ => text_response(StatusCode::NOT_FOUND, "Not found"),
    }
}

fn text_response(status: StatusCode, text: impl Into<Bytes>) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(text.into()))
        .expect("a text response builds")
}

/// Validates the upgrade request and derives the `Sec-WebSocket-Accept` value.
fn websocket_accept_key(request: &Request<Incoming>) -> Result<String, String> {
    if request.method() != Method::GET {
        return Err("The World endpoint only accepts a GET upgrade.".to_string());
    }
    let header = |name: &hyper::header::HeaderName| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
    };
    if !header(&UPGRADE).eq_ignore_ascii_case("websocket") {
        return Err("The World endpoint expects a WebSocket upgrade.".to_string());
    }
    if !header(&CONNECTION)
        .split(',')
        .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
    {
        return Err("The World endpoint expects Connection: Upgrade.".to_string());
    }
    if header(&SEC_WEBSOCKET_VERSION).trim() != "13" {
        return Err("The World endpoint expects WebSocket version 13.".to_string());
    }
    let key = header(&SEC_WEBSOCKET_KEY).trim();
    if key.is_empty() {
        return Err("The World endpoint expects Sec-WebSocket-Key.".to_string());
    }
    Ok(tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes()))
}
