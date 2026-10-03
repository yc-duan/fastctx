//! Signed stream records, including a cumulative digest through the final record.
use super::client::WorldClient;
use super::crypto::{self, Key32};
use super::identity::{self, SigningIdentity};
use super::link::{self, Dialed, WsTarget};
use super::stream_ticket::Ticket;
use super::wire::{Frame, Intent, StreamRole};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::Message;
pub(crate) const STREAM_PATH: &str = "/fastctx/world/v2/stream";
pub(crate) const CHUNK_BYTES: usize = 1024 * 1024;
// One chunk plus its type byte, u64 sequence and Ed25519 signature.
pub(crate) const MAX_MESSAGE_BYTES: usize = CHUNK_BYTES + 1 + 8 + 64;
const RECORD_CONTROL: u8 = 1;
const RECORD_DATA: u8 = 2;
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

fn proof(ticket: &Ticket, role: StreamRole, index: u64, digest: &[u8]) -> Result<Vec<u8>, String> {
    super::messages::encode(&(ticket, role, index, crypto::b64_encode(digest)))
}
pub(crate) struct Sealer {
    identity: SigningIdentity,
    ticket: Ticket,
    role: StreamRole,
    next: u64,
    hash: Sha256,
}
impl Sealer {
    pub(crate) fn new(identity: SigningIdentity, ticket: Ticket, role: StreamRole) -> Self {
        Self {
            identity,
            ticket,
            role,
            next: 0,
            hash: Sha256::new(),
        }
    }
    pub(crate) fn seal(&mut self, record: &Record) -> Result<Vec<u8>, String> {
        let mut payload = match record {
            Record::Data(data) => {
                let mut v = vec![RECORD_DATA];
                v.extend(data);
                v
            }
            Record::Control(control) => {
                let mut v = vec![RECORD_CONTROL];
                v.extend(super::messages::encode(control)?);
                v
            }
        };
        if payload.len() + 8 + 64 > MAX_MESSAGE_BYTES {
            return Err("stream record exceeds chunk framing size".into());
        }
        self.hash.update((payload.len() as u64).to_be_bytes());
        self.hash.update(&payload);
        let sig = self.identity.sign(
            "stream_digest",
            &proof(
                &self.ticket,
                self.role,
                self.next,
                &self.hash.clone().finalize(),
            )?,
        );
        let mut bytes = self.next.to_be_bytes().to_vec();
        bytes.extend(sig);
        bytes.append(&mut payload);
        self.next += 1;
        Ok(bytes)
    }
}
pub(crate) struct Opener {
    key: Key32,
    ticket: Ticket,
    role: StreamRole,
    next: u64,
    hash: Sha256,
}
impl Opener {
    pub(crate) fn new(key: Key32, ticket: Ticket, role: StreamRole) -> Self {
        Self {
            key,
            ticket,
            role,
            next: 0,
            hash: Sha256::new(),
        }
    }
    pub(crate) fn open(&mut self, bytes: &[u8]) -> Result<Record, String> {
        if bytes.len() < 73 || bytes.len() > MAX_MESSAGE_BYTES {
            return Err("invalid stream record size".into());
        }
        let index = u64::from_be_bytes(bytes[..8].try_into().expect("eight bytes"));
        if index != self.next {
            return Err("stream record sequence mismatch".into());
        }
        let payload = &bytes[72..];
        self.hash.update((payload.len() as u64).to_be_bytes());
        self.hash.update(payload);
        identity::verify(
            &self.key,
            "stream_digest",
            &proof(
                &self.ticket,
                self.role,
                index,
                &self.hash.clone().finalize(),
            )?,
            &bytes[8..72],
        )?;
        self.next += 1;
        match payload[0] {
            RECORD_DATA => Ok(Record::Data(payload[1..].to_vec())),
            RECORD_CONTROL => {
                super::messages::decode(&payload[1..], "stream control").map(Record::Control)
            }
            _ => Err("unknown stream record type".into()),
        }
    }
}
pub(crate) fn ticket(
    client: &WorldClient,
    sink: &str,
    principal: &str,
    target: &str,
    verb: &str,
) -> Result<Ticket, String> {
    let mut ticket = Ticket {
        world: client.config.read().world_id.clone(),
        stream: u64::from_be_bytes(crypto::random_bytes()?),
        source: client.name(),
        sink: sink.into(),
        principal: principal.into(),
        target: target.into(),
        verb: verb.into(),
        sig: String::new(),
    };
    ticket.sign(client.identity.signing())?;
    Ok(ticket)
}
pub(crate) async fn open(
    client: &WorldClient,
    ticket: &Ticket,
    role: StreamRole,
) -> Result<Dialed, String> {
    if ticket.endpoint(role) != client.name() || ticket.world != client.config.read().world_id {
        return Err("stream ticket endpoint/world mismatch".into());
    }
    let key = client
        .members
        .read()
        .trusted_key(&ticket.source)
        .ok_or("Untrusted stream author")?;
    ticket.verify(&key)?;
    let (plan, verify, config) = super::session::dial_plan(client)?;
    let mut dialed = link::dial(&plan, &verify, WsTarget::STREAM)
        .await
        .map_err(|e| e.to_string())?;
    match super::session::authenticate(
        client,
        &mut dialed,
        &config,
        0,
        Intent::Stream {
            stream: ticket.stream,
            role,
        },
        Some(ticket.clone()),
    )
    .await
    .map_err(|e| e.into_message())?
    {
        Frame::Attached { stream } if stream == ticket.stream => Ok(dialed),
        Frame::Bye { reason } => Err(reason),
        _ => Err("Unexpected stream admission".into()),
    }
}

pub(crate) struct Channel<'a> {
    client: &'a WorldClient,
    peer: String,
    socket: super::link::ws::Socket,
    sealer: Sealer,
    opener: Opener,
}
impl<'a> Channel<'a> {
    pub(crate) fn new(
        client: &'a WorldClient,
        dialed: Dialed,
        ticket: Ticket,
        role: StreamRole,
    ) -> Result<Self, String> {
        let peer_role = match role {
            StreamRole::Source => StreamRole::Sink,
            StreamRole::Sink => StreamRole::Source,
        };
        let key = client
            .members
            .read()
            .trusted_key(ticket.endpoint(peer_role))
            .ok_or("Stream peer is not trusted")?;
        Ok(Self {
            client,
            peer: ticket.endpoint(peer_role).to_string(),
            socket: dialed.socket,
            sealer: Sealer::new(client.identity.signing().clone(), ticket.clone(), role),
            opener: Opener::new(key, ticket, peer_role),
        })
    }
    pub(crate) async fn send(&mut self, record: Record) -> Result<(), String> {
        self.verify_peer()?;
        self.socket
            .send(Message::Binary(self.sealer.seal(&record)?.into()))
            .await
            .map_err(|e| e.to_string())
    }
    pub(crate) async fn recv(&mut self) -> Result<Record, String> {
        loop {
            match self.socket.next().await {
                Some(Ok(Message::Binary(bytes))) => {
                    self.verify_peer()?;
                    return self.opener.open(&bytes);
                }
                Some(Ok(Message::Ping(bytes))) => self
                    .socket
                    .send(Message::Pong(bytes))
                    .await
                    .map_err(|e| e.to_string())?,
                Some(Ok(Message::Pong(_))) => {}
                Some(Err(e)) => return Err(e.to_string()),
                _ => return Err("Stream closed before its signed end".into()),
            }
        }
    }
    fn verify_peer(&self) -> Result<(), String> {
        if self.client.members.read().trusted_key(&self.peer) != Some(self.opener.key) {
            return Err("Stream peer is revoked or its admission is no longer trusted".into());
        }
        Ok(())
    }
}
