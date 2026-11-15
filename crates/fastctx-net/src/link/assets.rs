//! The second version's asset sync (`fastctx_types::assets`), asked of a
//! hub whose view declares an `asset_sync` this machine still speaks.

use super::Link;
use crate::wire::{Question, Told};
use fastctx_types::assets::{ChunkHead, OLDEST, Reply, Request, VERSION};

impl Link {
    /// Asks the hub `request`; an error where it speaks only an asset sync
    /// older than any this machine speaks, or none.
    pub async fn assets(&self, request: Request) -> Result<Reply, String> {
        if self.view().asset_sync < OLDEST {
            return Err(format!("the hub speaks asset sync {}, this machine {OLDEST} to {VERSION}", self.view().asset_sync));
        }
        match self.inner.ask(Question::Assets { request }).await? {
            Told::Assets { reply } => Ok(reply),
            _ => Err("the hub did not answer the asset request".into()),
        }
    }
}

impl Link {
    /// The asset sync this machine speaks with the hub: the newest both
    /// speak (`fastctx_types::assets::VERSION`).
    pub fn asset_version(&self) -> u32 {
        self.view().asset_sync.min(VERSION)
    }

    /// Opens the data connection that carries an object's chunks to the hub
    /// under its `ticket` (`Request::Put`).
    pub async fn put_object(&self, ticket: u64) -> Result<Outgoing, String> {
        Ok(Outgoing { stream: self.object_stream(ticket).await? })
    }

    /// Opens the data connection that brings an object's chunks from the hub
    /// under its `ticket` (`Request::Get`).
    pub async fn get_object(&self, ticket: u64) -> Result<Incoming, String> {
        Ok(Incoming { stream: self.object_stream(ticket).await? })
    }

    async fn object_stream(&self, ticket: u64) -> Result<crate::data::Stream, String> {
        let crate::data::Endpoint::Hub { upstreams, hub, cert } = self.inner.data_endpoint() else {
            return Err("objects travel with the hub itself".into());
        };
        crate::data::Stream::to_hub(&upstreams, &hub, cert, &self.inner.key, move |key, proof| crate::wire::Intent::Asset {
            key,
            proof,
            ticket,
        })
        .await
    }
}

/// An object's chunks on their way to the hub, each a `ChunkHead` then its
/// bytes.
pub struct Outgoing {
    stream: crate::data::Stream,
}

impl Outgoing {
    /// Sends a large object's chunk digests and hears which chunks the hub
    /// wants, as ranges [start, end).
    pub async fn manifest(&mut self, digests: &[[u8; 32]]) -> Result<Vec<(u64, u64)>, String> {
        self.stream.say(&fastctx_types::assets::ManifestHead { count: digests.len() as u64 }).await?;
        let flat: Vec<u8> = digests.iter().flatten().copied().collect();
        self.stream.send(&flat).await?;
        let wanted: fastctx_types::assets::Wanted = self.stream.hear().await?;
        Ok(wanted.ranges)
    }

    /// Sends chunk `index`.
    pub async fn chunk(&mut self, index: u64, bytes: &[u8]) -> Result<(), String> {
        self.stream.say(&ChunkHead { index, size: bytes.len() as u64 }).await?;
        self.stream.send(bytes).await
    }

    /// Waits for the hub to say it took the object.
    pub async fn finish(self) -> Result<(), String> {
        self.stream.finish().await
    }
}

/// An object's chunks coming from the hub.
pub struct Incoming {
    stream: crate::data::Stream,
}

impl Incoming {
    /// A large object's chunk digests, `count` of them, as the hub sends
    /// them ahead of its chunks.
    pub async fn manifest(&mut self, count: u64) -> Result<Vec<[u8; 32]>, String> {
        // A step due, not a quiet stream: pings alone do not keep it waiting.
        let head: fastctx_types::assets::ManifestHead = self.stream.hear_step("object's digests").await?;
        if head.count != count {
            return Err("the hub's digests are not one a chunk".into());
        }
        let mut flat = Vec::with_capacity((count * 32) as usize);
        self.stream
            .take(count * 32, |piece| {
                flat.extend_from_slice(piece);
                Ok(())
            })
            .await?;
        Ok(flat.chunks_exact(32).map(|digest| digest.try_into().expect("32 bytes")).collect())
    }

    /// The next chunk: its index and bytes.
    pub async fn chunk(&mut self) -> Result<(u64, Vec<u8>), String> {
        // The hub sends the next head as soon as it read the chunk: a head
        // that does not come within the step's bound, however lively the
        // line's pings, ends the transfer, to be asked for again.
        let head: ChunkHead = self.stream.hear_step("next chunk").await?;
        if head.size > fastctx_types::assets::CHUNK_BYTES {
            return Err("a chunk is at most 1 MiB".into());
        }
        let mut bytes = Vec::with_capacity(head.size as usize);
        self.stream
            .take(head.size, |piece| {
                bytes.extend_from_slice(piece);
                Ok(())
            })
            .await?;
        Ok((head.index, bytes))
    }

    /// Says this machine has every chunk it was to take.
    pub async fn done(self) -> Result<(), String> {
        self.stream.done().await
    }
}
