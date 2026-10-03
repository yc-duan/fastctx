//! Large call results use the same authenticated data path as file copies.
use super::{
    client::WorldClient,
    crypto, messages,
    stream::{self, Channel, Control, Record},
    stream_ticket::Ticket,
    wire::StreamRole,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Description {
    pub(crate) ticket: Ticket,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}
pub(crate) async fn send(
    client: &WorldClient,
    ticket: Ticket,
    bytes: Vec<u8>,
) -> Result<(), String> {
    let dialed = stream::open(client, &ticket, StreamRole::Source).await?;
    let mut channel = Channel::new(client, dialed, ticket, StreamRole::Source)?;
    for chunk in bytes.chunks(stream::CHUNK_BYTES) {
        channel.send(Record::Data(chunk.to_vec())).await?;
    }
    channel
        .send(Record::Control(Control::End {
            files: 0,
            bytes: bytes.len() as u64,
        }))
        .await?;
    match channel.recv().await? {
        Record::Control(Control::Done {
            bytes: received, ..
        }) if received == bytes.len() as u64 => Ok(()),
        _ => Err("Result stream has no valid receipt".into()),
    }
}
pub(crate) async fn receive(
    client: &WorldClient,
    description: Description,
    author: &str,
) -> Result<messages::CallResult, String> {
    if description.ticket.source != author || description.ticket.sink != client.name() {
        return Err("Result stream endpoints differ from the call".into());
    }
    let dialed = stream::open(client, &description.ticket, StreamRole::Sink).await?;
    let mut channel = Channel::new(client, dialed, description.ticket, StreamRole::Sink)?;
    let mut bytes = Vec::new();
    loop {
        match channel.recv().await? {
            Record::Data(data) => {
                bytes.extend(data);
                if bytes.len() as u64 > description.bytes {
                    return Err("Result stream exceeded its signed length".into());
                }
            }
            Record::Control(Control::End { bytes: count, .. }) if count == description.bytes => {
                break;
            }
            _ => return Err("Unexpected result stream record".into()),
        }
    }
    if bytes.len() as u64 != description.bytes
        || hex::encode(crypto::sha256(&bytes)) != description.sha256
    {
        return Err("Result stream digest mismatch".into());
    }
    let result = messages::decode(&bytes, "streamed call result")?;
    channel
        .send(Record::Control(Control::Done {
            copied: 0,
            bytes: description.bytes,
            skipped: 0,
            links_skipped: 0,
            failed: Vec::new(),
        }))
        .await?;
    Ok(result)
}
