//! Length-delimited control-center handshake, framed requests, and a raw MCP answer stream.

use crate::process_identity::ProcessIdentity;
use crate::server::ServerOptions;
use crate::session::SessionEnvironment;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const PROTOCOL_VERSION: u32 = 2;
const MAX_HANDSHAKE_BYTES: usize = 16 * 1024 * 1024;
/// Largest payload the proxy places in one request frame, and the buffer the control center reads
/// it back into.
pub(crate) const MAX_REQUEST_FRAME_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Handshake {
    protocol_version: u32,
    pub(crate) options: ServerOptions,
    pub(crate) environment: SessionEnvironment,
    /// The host application this session belongs to, when the proxy could identify it. The control
    /// center keeps its runtime warm while any named host is still running.
    pub(crate) host: Option<ProcessIdentity>,
}

impl Handshake {
    pub(crate) fn new(
        options: ServerOptions,
        environment: SessionEnvironment,
        host: Option<ProcessIdentity>,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            options,
            environment,
            host,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(format!(
                "Unsupported control-center protocol version {}; expected {}.",
                self.protocol_version, PROTOCOL_VERSION
            ));
        }
        if !self.environment.cwd().is_absolute() {
            return Err(
                "The control-center handshake contained a non-absolute working directory."
                    .to_string(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct HandshakeResponse {
    error: Option<String>,
}

pub(crate) async fn write_handshake(
    stream: &mut (impl AsyncWrite + Unpin),
    handshake: &Handshake,
) -> Result<(), String> {
    write_frame(stream, handshake, "control-center handshake").await
}

pub(crate) async fn read_handshake(
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<Handshake, String> {
    let handshake: Handshake = read_frame(stream, "control-center handshake").await?;
    handshake.validate()?;
    Ok(handshake)
}

pub(crate) async fn write_handshake_success(
    stream: &mut (impl AsyncWrite + Unpin),
) -> Result<(), String> {
    write_frame(
        stream,
        &HandshakeResponse { error: None },
        "control-center handshake response",
    )
    .await
}

pub(crate) async fn write_handshake_error(
    stream: &mut (impl AsyncWrite + Unpin),
    error: String,
) -> Result<(), String> {
    write_frame(
        stream,
        &HandshakeResponse { error: Some(error) },
        "control-center handshake response",
    )
    .await
}

pub(crate) async fn read_handshake_response(
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<(), String> {
    let response: HandshakeResponse =
        read_frame(stream, "control-center handshake response").await?;
    match response.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Sends one chunk of MCP request bytes to the control center as a length-prefixed frame.
///
/// A raw byte stream cannot say "no more requests" on every platform: Unix half-closes a socket,
/// Windows named pipes have no equivalent. Without that mark the control center holds a finished
/// session open, and a proxy whose stdin closed would have to guess how long to wait for the
/// answers it is still owed — a guess that is either too short to deliver them or too long to
/// shut down promptly. The explicit end-of-input frame removes the guess. Both ends always come
/// from the same build, because the endpoint name carries the build id, so the framing needs no
/// negotiation.
pub(crate) async fn write_request_frame(
    output: &mut (impl AsyncWrite + Unpin),
    payload: &[u8],
) -> Result<(), FrameWriteFailure> {
    let mut reached_engine = false;
    for chunk in payload.chunks(MAX_REQUEST_FRAME_BYTES) {
        let length =
            u32::try_from(chunk.len()).expect("a request frame never exceeds the frame size limit");
        if let Err(error) = output.write_all(&length.to_be_bytes()).await {
            return Err(FrameWriteFailure {
                reached_engine,
                error,
            });
        }
        if let Err(error) = output.write_all(chunk).await {
            // The length prefix landed, so the engine saw part of this payload.
            return Err(FrameWriteFailure {
                reached_engine: true,
                error,
            });
        }
        reached_engine = true;
    }
    output.flush().await.map_err(|error| FrameWriteFailure {
        reached_engine,
        error,
    })
}

/// A frame write that failed, and whether any of its bytes reached the engine.
///
/// The distinction is what lets a closed-out call say "nothing ran" instead of the weaker "this
/// may or may not have finished", which matters most for the calls that have side effects.
pub(crate) struct FrameWriteFailure {
    pub(crate) reached_engine: bool,
    pub(crate) error: std::io::Error,
}

/// Marks the end of input, which is what makes the control center stop expecting requests.
pub(crate) async fn write_end_of_input(
    output: &mut (impl AsyncWrite + Unpin),
) -> std::io::Result<()> {
    output.write_all(&0_u32.to_be_bytes()).await?;
    output.flush().await
}

/// Reassembles framed requests into the plain byte stream the MCP server reads.
///
/// Returns once the proxy marks the end of input, which is what makes the server observe EOF and
/// end the session.
pub(crate) async fn receive_requests(
    input: &mut (impl AsyncRead + Unpin),
    output: &mut (impl AsyncWrite + Unpin),
) -> Result<(), String> {
    let mut header = [0_u8; 4];
    let mut payload = vec![0_u8; MAX_REQUEST_FRAME_BYTES];
    loop {
        match input.read_exact(&mut header).await {
            Ok(_) => {}
            // A proxy that died without marking the end of input tells the server the same thing:
            // nothing more is coming.
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(format!("Cannot read the request frame length: {error}")),
        }
        let length = u32::from_be_bytes(header) as usize;
        if length == 0 {
            return Ok(());
        }
        if length > MAX_REQUEST_FRAME_BYTES {
            return Err(format!(
                "A {length}-byte request frame exceeds the {MAX_REQUEST_FRAME_BYTES}-byte limit."
            ));
        }
        input
            .read_exact(&mut payload[..length])
            .await
            .map_err(|error| format!("Cannot read a {length}-byte request frame: {error}"))?;
        output
            .write_all(&payload[..length])
            .await
            .map_err(|error| format!("Cannot hand a request frame to the MCP server: {error}"))?;
        output
            .flush()
            .await
            .map_err(|error| format!("Cannot flush a request frame to the MCP server: {error}"))?;
    }
}

async fn write_frame<T: Serialize>(
    stream: &mut (impl AsyncWrite + Unpin),
    value: &T,
    label: &str,
) -> Result<(), String> {
    let bytes =
        serde_json::to_vec(value).map_err(|error| format!("Cannot encode the {label}: {error}"))?;
    if bytes.len() > MAX_HANDSHAKE_BYTES {
        return Err(format!(
            "Cannot send the {label}: {} bytes exceeds the {}-byte safety limit.",
            bytes.len(),
            MAX_HANDSHAKE_BYTES
        ));
    }
    let length = u32::try_from(bytes.len())
        .map_err(|_| format!("Cannot send the {label}: its length cannot be represented."))?;
    stream
        .write_all(&length.to_be_bytes())
        .await
        .map_err(|error| format!("Cannot write the {label} length: {error}"))?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|error| format!("Cannot write the {label} body: {error}"))?;
    stream
        .flush()
        .await
        .map_err(|error| format!("Cannot flush the {label}: {error}"))
}

async fn read_frame<T: for<'de> Deserialize<'de>>(
    stream: &mut (impl AsyncRead + Unpin),
    label: &str,
) -> Result<T, String> {
    let mut length = [0_u8; 4];
    stream
        .read_exact(&mut length)
        .await
        .map_err(|error| format!("Cannot read the {label} length: {error}"))?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_HANDSHAKE_BYTES {
        return Err(format!(
            "Cannot read the {label}: {length} bytes exceeds the {MAX_HANDSHAKE_BYTES}-byte safety limit."
        ));
    }
    let mut bytes = vec![0_u8; length];
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(|error| format!("Cannot read the {label} body: {error}"))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("Cannot parse the {label}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{
        Handshake, read_handshake, read_handshake_response, write_handshake,
        write_handshake_success,
    };
    use crate::server::ServerOptions;
    use crate::server_manifest::EnabledTools;
    use crate::session::SessionEnvironment;
    use std::ffi::OsString;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn handshake_is_framed_without_consuming_following_mcp_bytes() {
        let (mut client, mut server) = tokio::io::duplex(64 * 1024);
        let handshake = Handshake::new(
            ServerOptions {
                tools: EnabledTools::all(),
            },
            SessionEnvironment::new(
                std::env::current_dir().unwrap(),
                vec![(OsString::from("PATH"), OsString::from("sentinel"))],
            ),
            None,
        );
        let client_task = tokio::spawn(async move {
            write_handshake(&mut client, &handshake).await.unwrap();
            client.write_all(b"mcp\n").await.unwrap();
            read_handshake_response(&mut client).await.unwrap();
        });

        let decoded = read_handshake(&mut server).await.unwrap();
        assert!(decoded.options.tools.shell_enabled());
        let mut tail = [0_u8; 4];
        server.read_exact(&mut tail).await.unwrap();
        assert_eq!(&tail, b"mcp\n");
        write_handshake_success(&mut server).await.unwrap();
        client_task.await.unwrap();
    }
}
