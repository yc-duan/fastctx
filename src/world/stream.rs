//! Data connections on the member side: the record every `copy` byte travels in, sealed end to
//! end, and opening one end of a stream to the hub (`design-transport.md` §5.2, §9).
//!
//! A record is one WebSocket message: `[index u64][nonce 24][ciphertext]`, the ciphertext an
//! XChaCha20-Poly1305 seal of `[type u8][payload]` under the World key's `blob` sub-key with
//! the stream id, the sender's role, and the index as additional data. Each direction numbers
//! its records from zero and the reader insists on the next number, so the hub can neither
//! reorder nor replay a record it cannot read.

use super::client::WorldClient;
use super::crypto::{self, Key32, NONCE_LEN};
use super::link::{self, Dialed, WsTarget};
use super::wire::{Frame, Intent, StreamRole};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// HTTP path the hub upgrades data connections on.
pub(crate) const STREAM_PATH: &str = "/fastctx/world/v1/stream";
/// Bytes of file content one data record carries.
pub(crate) const CHUNK_BYTES: usize = 1024 * 1024;
/// Largest sealed record.
pub(crate) const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;
/// WebSocket message limit on a data connection: a record plus its framing.
pub(crate) const MAX_MESSAGE_BYTES: usize = MAX_RECORD_BYTES + 4096;
/// Dial, handshake, and the hub's wait for the other end.
pub(crate) const ATTACH_TIMEOUT: Duration = Duration::from_secs(45);

const RECORD_CONTROL: u8 = 1;
const RECORD_DATA: u8 = 2;
const AAD_LABEL: &[u8] = b"stream";

/// One file the sink could not finish, as reported in `done`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Failure {
    pub(crate) path: String,
    pub(crate) reason: String,
}

/// Control records of a `copy` stream (`design-protocol.md` §7). The source sends the first
/// six, the sink the last three.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "r", rename_all = "snake_case")]
pub(crate) enum Control {
    File {
        index: u64,
        /// Relative to the copied root, `/`-separated; empty for a single-file copy.
        path: String,
        size: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<u32>,
        /// Modification time, seconds since the Unix epoch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mtime: Option<i64>,
        sha256: String,
    },
    Dir {
        path: String,
    },
    LinkSkipped {
        path: String,
    },
    FileBegin {
        index: u64,
    },
    FileEnd {
        index: u64,
    },
    End {
        files: u64,
        bytes: u64,
    },
    Want {
        index: u64,
    },
    Skip {
        index: u64,
    },
    /// The destination verified and moved one file into place.
    Received {
        index: u64,
        bytes: u64,
    },
    Done {
        copied: u64,
        bytes: u64,
        skipped: u64,
        links_skipped: u64,
        failed: Vec<Failure>,
    },
}

/// One record on a data connection.
#[derive(Debug)]
pub(crate) enum Record {
    Control(Control),
    Data(Vec<u8>),
}

fn aad(stream: u64, role: StreamRole, index: u64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(AAD_LABEL.len() + 8 + 1 + 8);
    bytes.extend_from_slice(AAD_LABEL);
    bytes.extend_from_slice(&stream.to_be_bytes());
    bytes.push(match role {
        StreamRole::Source => 0,
        StreamRole::Sink => 1,
    });
    bytes.extend_from_slice(&index.to_be_bytes());
    bytes
}

/// Seals the records one end sends, numbering them.
pub(crate) struct Sealer {
    key: Key32,
    stream: u64,
    role: StreamRole,
    next: u64,
}

impl Sealer {
    pub(crate) fn new(key: Key32, stream: u64, role: StreamRole) -> Self {
        Self {
            key,
            stream,
            role,
            next: 0,
        }
    }

    pub(crate) fn seal(&mut self, record: &Record) -> Result<Vec<u8>, String> {
        let plaintext = match record {
            Record::Control(control) => {
                let mut bytes = vec![RECORD_CONTROL];
                bytes.extend(
                    serde_json::to_vec(control)
                        .map_err(|error| format!("Cannot encode a stream record: {error}"))?,
                );
                bytes
            }
            Record::Data(data) => {
                let mut bytes = Vec::with_capacity(data.len() + 1);
                bytes.push(RECORD_DATA);
                bytes.extend_from_slice(data);
                bytes
            }
        };
        let index = self.next;
        self.next += 1;
        let nonce = crypto::random_bytes::<NONCE_LEN>()?;
        let ciphertext = crypto::seal_with_nonce(
            &self.key,
            &nonce,
            &aad(self.stream, self.role, index),
            &plaintext,
        )?;
        let mut bytes = Vec::with_capacity(8 + NONCE_LEN + ciphertext.len());
        bytes.extend_from_slice(&index.to_be_bytes());
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(&ciphertext);
        Ok(bytes)
    }
}

/// Opens the records the other end sends, refusing anything out of order.
pub(crate) struct Opener {
    key: Key32,
    stream: u64,
    peer_role: StreamRole,
    next: u64,
}

impl Opener {
    pub(crate) fn new(key: Key32, stream: u64, peer_role: StreamRole) -> Self {
        Self {
            key,
            stream,
            peer_role,
            next: 0,
        }
    }

    pub(crate) fn open(&mut self, bytes: &[u8]) -> Result<Record, String> {
        if bytes.len() < 8 + NONCE_LEN {
            return Err("a stream record is too short".to_string());
        }
        let index = u64::from_be_bytes(bytes[..8].try_into().expect("eight bytes"));
        if index != self.next {
            return Err(format!(
                "stream record {index} arrived where {} was expected",
                self.next
            ));
        }
        let mut nonce = [0_u8; NONCE_LEN];
        nonce.copy_from_slice(&bytes[8..8 + NONCE_LEN]);
        let plaintext = crypto::open(
            &self.key,
            &nonce,
            &aad(self.stream, self.peer_role, index),
            &bytes[8 + NONCE_LEN..],
        )?;
        self.next += 1;
        match plaintext.split_first() {
            Some((&RECORD_CONTROL, payload)) => serde_json::from_slice(payload)
                .map(Record::Control)
                .map_err(|error| format!("Malformed stream record: {error}")),
            Some((&RECORD_DATA, payload)) => Ok(Record::Data(payload.to_vec())),
            _ => Err("a stream record has an unknown type".to_string()),
        }
    }
}

/// The `blob` sub-key of one World key epoch.
pub(crate) fn blob_key(client: &WorldClient, epoch: u32) -> Result<Key32, String> {
    client
        .keys
        .read()
        .subkeys(epoch)
        .map(|subkeys| subkeys.blob)
        .ok_or_else(|| {
            format!("key_epoch_unknown: this member holds no World key for epoch {epoch}.")
        })
}

/// Opens one end of a data connection: dials the hub's stream path over the same network path
/// as the control connection, authenticates as this member, and waits for the hub to pair it.
pub(crate) async fn open(
    client: &Arc<WorldClient>,
    stream: u64,
    role: StreamRole,
) -> Result<Dialed, String> {
    let (plan, verify, config) = super::session::dial_plan(client)?;
    let mut dialed = link::dial(&plan, &verify, WsTarget::STREAM)
        .await
        .map_err(|failure| format!("cannot open a data connection: {failure}"))?;
    let admission = tokio::time::timeout(
        ATTACH_TIMEOUT,
        super::session::authenticate(
            client,
            &mut dialed,
            &config,
            0,
            Intent::Stream { stream, role },
        ),
    )
    .await;
    match admission {
        Ok(Ok(Frame::Attached { stream: attached })) if attached == stream => Ok(dialed),
        Ok(Ok(Frame::Bye { reason })) => {
            dialed.close().await;
            Err(format!("the hub refused the data connection: {reason}"))
        }
        Ok(Ok(_)) => {
            dialed.close().await;
            Err("the hub answered the data connection with an unexpected frame".to_string())
        }
        Ok(Err(failure)) => {
            dialed.close().await;
            Err(failure.into_message())
        }
        Err(_) => {
            dialed.close().await;
            Err(format!(
                "the other end did not attach within {} s",
                ATTACH_TIMEOUT.as_secs()
            ))
        }
    }
}
