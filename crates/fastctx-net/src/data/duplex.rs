//! Independent directions of a data connection. Dropping its reader ends
//! both directions, including a writer blocked on a peer that stopped reading.

use super::{Pace, Stream, WsMessage, line};
use serde::{Serialize, de::DeserializeOwned};

pub struct ReadHalf(Stream);
pub struct WriteHalf {
    line: line::Sender,
    pace: Pace,
}

impl Stream {
    /// Separates reading and writing without buffering an unbounded stream.
    pub fn split(self) -> (ReadHalf, WriteHalf) {
        let writer = WriteHalf { line: self.line.sender(), pace: Pace::default() };
        (ReadHalf(self), writer)
    }
}

impl ReadHalf {
    pub async fn hear<T: DeserializeOwned>(&mut self) -> Result<T, String> {
        self.0.hear().await
    }
    pub async fn hear_or_end<T: DeserializeOwned>(&mut self) -> Result<Option<T>, String> {
        self.0.hear_or_end().await
    }
    pub async fn take(&mut self, count: u64, take: impl FnMut(&[u8]) -> Result<(), String>) -> Result<(), String> {
        self.0.take(count, take).await
    }
}

impl WriteHalf {
    pub async fn say(&mut self, word: &impl Serialize) -> Result<(), String> {
        self.line.send(WsMessage::Text(serde_json::to_string(word).expect("words serialize").into())).await
    }
    pub async fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut rest = bytes;
        while !rest.is_empty() {
            let (piece, after) = rest.split_at(rest.len().min(self.pace.chunk()));
            let began = std::time::Instant::now();
            self.line.send(WsMessage::Binary(piece.to_vec().into())).await?;
            self.pace.wrote(piece.len(), began.elapsed());
            rest = after;
        }
        Ok(())
    }
}
