//! A one-shot URL MCP check from the calling machine. Session traffic never
//! uses this client or a World connection; hosts connect their URLs directly.

use bytes::Bytes;
use fastctx_types::{mcp::Tested, mcp_message::MAX_LINE, profile::RemoteMcp};
use http_body_util::{BodyExt as _, Full};
use hyper::{
    Request,
    body::Incoming,
    http::{HeaderMap, Uri},
};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};

pub async fn test(server: &RemoteMcp) -> Tested {
    let result = tokio::time::timeout(std::time::Duration::from_secs(60), trial(server)).await;
    match result {
        Ok(Ok(tools)) => Tested { tools, problem: None, log: None },
        Ok(Err(problem)) => Tested { tools: vec![], problem: Some(problem), log: None },
        Err(_) => Tested {
            tools: vec![],
            problem: Some("the MCP did not initialize and list tools within 60 s".into()),
            log: None,
        },
    }
}

async fn trial(server: &RemoteMcp) -> Result<Vec<String>, String> {
    let mut client = Client { server, url: server.url.clone(), session: None, events: None, buffer: Vec::new() };
    if server.sse {
        let response = client.request("GET", None).await?;
        check_status(response.status().as_u16())?;
        client.events = Some(response.into_body());
        let (event, data) = client.event().await?;
        if event != "endpoint" {
            return Err("the SSE MCP did not supply a message endpoint".into());
        }
        let base: Uri = server.url.parse().map_err(|e| format!("invalid MCP URL: {e}"))?;
        let url = if data.starts_with('/') {
            format!(
                "{}://{}{}",
                base.scheme_str().unwrap_or("http"),
                base.authority().ok_or("MCP URL has no authority")?,
                data
            )
        } else {
            data
        };
        let target: Uri = url.parse().map_err(|e| format!("invalid MCP message endpoint: {e}"))?;
        if base.scheme() != target.scheme() || base.authority() != target.authority() {
            return Err("the SSE message endpoint is on a different origin".into());
        }
        client.url = url;
    }
    let initialized=client.call(1,"initialize",json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"fastctx","version":env!("CARGO_PKG_VERSION")}})).await?;
    let _ = initialized;
    client.notification("notifications/initialized").await?;
    let mut tools = Vec::new();
    let mut cursor = None;
    let mut cursors = std::collections::HashSet::new();
    loop {
        let params = cursor.as_ref().map(|cursor| json!({"cursor":cursor})).unwrap_or(json!({}));
        let result = client.call(2, "tools/list", params).await?;
        let listed = result["tools"].as_array().ok_or("tools/list returned no tools array")?;
        for tool in listed {
            tools.push(tool["name"].as_str().ok_or("an MCP tool has no name")?.to_owned());
        }
        cursor = result["nextCursor"].as_str().map(str::to_owned);
        if let Some(cursor) = &cursor {
            if !cursors.insert(cursor.clone()) {
                return Err("tools/list repeated its cursor".into());
            }
        } else {
            break;
        }
    }
    if !server.sse && client.session.is_some() {
        let _ = client.request("DELETE", None).await;
    }
    Ok(tools)
}

struct Client<'a> {
    server: &'a RemoteMcp,
    url: String,
    session: Option<String>,
    events: Option<Incoming>,
    buffer: Vec<u8>,
}
impl Client<'_> {
    async fn request(&self, method: &str, message: Option<&Value>) -> Result<hyper::Response<Incoming>, String> {
        let uri: Uri = self.url.parse().map_err(|e| format!("invalid MCP URL: {e}"))?;
        let host = uri.host().ok_or("MCP URL has no host")?.trim_matches(['[', ']']);
        let secure = match uri.scheme_str() {
            Some("https") => true,
            Some("http") => false,
            _ => return Err("MCP requires http or https".into()),
        };
        let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
        let tcp = tokio::net::TcpStream::connect((host, port)).await.map_err(|e| format!("connecting to MCP: {e}"))?;
        let mut headers = HeaderMap::new();
        for (name, value) in &self.server.headers {
            headers.insert(
                hyper::header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| "invalid MCP header name")?,
                value.parse().map_err(|_| "invalid MCP header value")?,
            );
        }
        if let Some(session) = &self.session {
            headers.insert("mcp-session-id", session.parse().map_err(|_| "invalid MCP session id")?);
        }
        let body = message.map(|v| serde_json::to_vec(v).expect("MCP JSON")).unwrap_or_default();
        let mut request = Request::builder()
            .method(method)
            .uri(uri.path_and_query().map_or("/", |p| p.as_str()))
            .header("Host", uri.authority().ok_or("MCP URL has no authority")?.as_str())
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2024-11-05")
            .body(Full::new(Bytes::from(body)))
            .map_err(|e| e.to_string())?;
        request.headers_mut().extend(headers);
        if secure {
            let name = rustls::pki_types::ServerName::try_from(host.to_owned()).map_err(|e| e.to_string())?;
            let stream = tokio_rustls::TlsConnector::from(crate::web::config())
                .connect(name, tcp)
                .await
                .map_err(|e| format!("MCP TLS: {e}"))?;
            send(stream, request).await
        } else {
            send(tcp, request).await
        }
    }
    async fn notification(&mut self, method: &str) -> Result<(), String> {
        let response = self.request("POST", Some(&json!({"jsonrpc":"2.0","method":method}))).await?;
        check_status(response.status().as_u16())
    }
    async fn call(&mut self, id: u64, method: &str, params: Value) -> Result<Value, String> {
        let response =
            self.request("POST", Some(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))).await?;
        check_status(response.status().as_u16())?;
        if let Some(session) = response.headers().get("mcp-session-id") {
            self.session = Some(session.to_str().map_err(|_| "invalid MCP session id")?.to_owned());
        }
        let sse = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        let value = if self.server.sse || sse {
            if sse {
                self.events = Some(response.into_body());
                self.buffer.clear();
            }
            loop {
                let (_, data) = self.event().await?;
                let value: Value = serde_json::from_str(&data).map_err(|e| format!("invalid MCP event: {e}"))?;
                if value["id"] == id {
                    break value;
                }
            }
        } else {
            let mut body = response.into_body();
            let mut bytes = Vec::new();
            while let Some(frame) = body.frame().await {
                if let Ok(data) = frame.map_err(|e| e.to_string())?.into_data() {
                    if bytes.len() + data.len() > MAX_LINE {
                        return Err("MCP response exceeds 32 MiB".into());
                    }
                    bytes.extend(data);
                }
            }
            serde_json::from_slice::<Value>(&bytes).map_err(|e| format!("invalid MCP response: {e}"))?
        };
        if value["jsonrpc"] != "2.0" || value["id"] != id {
            return Err("the MCP response does not match the request".into());
        }
        if let Some(error) = value.get("error") {
            return Err(format!("{method}: {error}"));
        }
        value.get("result").cloned().ok_or_else(|| format!("{method} has no result"))
    }
    async fn event(&mut self) -> Result<(String, String), String> {
        loop {
            if let Some((at, size)) = self
                .buffer
                .windows(2)
                .position(|w| w == b"\n\n")
                .map(|p| (p, 2))
                .or_else(|| self.buffer.windows(4).position(|w| w == b"\r\n\r\n").map(|p| (p, 4)))
            {
                let event = String::from_utf8(self.buffer[..at].to_vec()).map_err(|_| "MCP event is not UTF-8")?;
                self.buffer.drain(..at + size);
                let mut kind = String::new();
                let mut data = Vec::new();
                for line in event.lines() {
                    if let Some(v) = line.strip_prefix("event:") {
                        kind = v.trim().into();
                    }
                    if let Some(v) = line.strip_prefix("data:") {
                        data.push(v.strip_prefix(' ').unwrap_or(v));
                    }
                }
                if !data.is_empty() {
                    return Ok((kind, data.join("\n")));
                }
                continue;
            }
            let body = self.events.as_mut().ok_or("the MCP event stream is not open")?;
            let frame = body.frame().await.ok_or("the MCP event stream ended")?.map_err(|e| e.to_string())?;
            if let Ok(data) = frame.into_data() {
                if self.buffer.len() + data.len() > MAX_LINE {
                    return Err("MCP event exceeds 32 MiB".into());
                }
                self.buffer.extend(data);
            }
        }
    }
}
fn check_status(status: u16) -> Result<(), String> {
    if (200..300).contains(&status) { Ok(()) } else { Err(format!("MCP returned HTTP {status}")) }
}
async fn send<T: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    stream: T,
    request: Request<Full<Bytes>>,
) -> Result<hyper::Response<Incoming>, String> {
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.map_err(|e| e.to_string())?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    sender.send_request(request).await.map_err(|e| e.to_string())
}
