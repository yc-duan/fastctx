//! One transfer of an immutable artifact; later viewports and searches are local.

use fastctx_types::access::Written;
use fastctx_types::lines::{Decoding, Reply, Request, Result as Answer};
use std::sync::Arc;

use crate::{
    Error, World,
    artifact_sink::{ArtifactSink, Pending, SourceOwner},
    cache::{
        Cache,
        lines::{self, Frozen},
    },
};

impl World {
    pub(crate) async fn open_frozen_lines(
        &self,
        cache: Arc<Cache>,
        frozen: Frozen,
        machine: Written,
        path: String,
        decoding: Decoding,
    ) -> Result<Reply, Error> {
        if frozen.body.is_some() {
            return self.open_frozen_output(cache, frozen, machine, path, decoding).await;
        }
        let source_generation = self.line_sources.lock().expect("line sources").generation();
        let locked = cache.clone();
        let key = frozen.key.clone();
        let (request, generation, found) = blocking(move || {
            let request = locked.request(&format!("lines:{key}"))?;
            let turn = locked.turn()?;
            Ok((request, turn.generation(), lines::index(locked.root(), &key)?))
        })
        .await?;
        let index = match found {
            Some(index) => index,
            None => {
                let reply = self
                    .open_network_lines(
                        Some(machine.clone()),
                        path.clone(),
                        Decoding::Immutable {
                            content: frozen.content,
                            policy: None,
                            annotation: None,
                            decoding: Box::new(decoding),
                        },
                    )
                    .await?;
                let Answer::Opened { source, extent, .. } = reply.result else {
                    return Err(Error::Unexpected("opened immutable line source".into()));
                };
                let _source_owner = SourceOwner {
                    sources: self.line_sources.clone(),
                    machine: machine.clone(),
                    source: source.clone(),
                };
                if !extent.complete {
                    return Err(Error::Unexpected("an immutable artifact is still growing".into()));
                }
                let random = fastctx_net::sign::random::<16>().map_err(Error::Kept)?;
                let folder = cache.root().join(".incoming").join(crate::cache::key(&format!("{random:?}")));
                tokio::fs::create_dir_all(&folder).await.map_err(Error::Kept)?;
                let pending = Pending(folder.clone());
                let mut sink = ArtifactSink::create(&folder, extent, None).await?;
                let transfer =
                    self.visit_lines(machine.clone(), source.clone(), 0, sink.index.extent.length, &mut sink).await;
                let closed = self.lines(machine.clone(), Request::Close { source }).await;
                let reply = transfer?;
                closed?;
                if reply.result != (Answer::ReadComplete { start: 0, count: sink.index.extent.length })
                    || sink.index.rows.len() as u64 != sink.index.extent.length
                    || sink.from != sink.offset
                {
                    return Err(Error::Unexpected("immutable line completion disagrees with its parts".into()));
                }
                sink.sync().await?;
                let ArtifactSink { output, index, .. } = sink;
                drop(output);
                let locked = cache.clone();
                let key = frozen.key.clone();
                let stored = serde_json::to_vec(&index).map_err(|error| Error::Other(error.to_string()))?;
                blocking(move || {
                    let _pending = pending;
                    let turn = locked.turn()?;
                    if turn.generation() != generation {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Interrupted,
                            "the viewed cache was cleared during the artifact read",
                        ));
                    }
                    fastctx_platform::publish(
                        &folder.join("index.json"),
                        &stored,
                        fastctx_platform::Permissions::Private,
                    )?;
                    fastctx_platform::ensure_private_dir(&locked.root().join("blobs"))?;
                    std::fs::rename(&folder, locked.root().join("blobs").join(key))
                })
                .await?;
                index
            }
        };
        drop(request);
        let source = format!(
            "kept-{}",
            crate::cache::key(&format!("{:?}", fastctx_net::sign::random::<16>().map_err(Error::Kept)?))
        );
        self.line_sources.lock().expect("line sources").keep(
            source_generation,
            machine.clone(),
            source.clone(),
            cache,
            frozen.key,
        )?;
        Ok(Reply { machine: machine.name, result: Answer::Opened { source, path, extent: index.extent } })
    }

    pub(crate) async fn kept_lines(
        &self,
        cache: Arc<Cache>,
        key: String,
        machine: Written,
        request: Request,
    ) -> Result<Reply, Error> {
        let result = blocking(move || {
            let _turn = cache.turn()?;
            match request {
                Request::Read { start, count, .. } => {
                    let (lines, notes) = lines::read(cache.root(), &key, start, count)?;
                    Ok(Answer::Lines { start, lines, notes })
                }
                Request::Find { query, position, direction, .. } => {
                    Ok(Answer::Found { found: lines::find(cache.root(), &key, &query, &position, direction)? })
                }
                _ => Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "the request is not a kept line read")),
            }
        })
        .await?;
        Ok(Reply { machine: machine.name, result })
    }
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> std::io::Result<T> + Send + 'static) -> Result<T, Error> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| Error::Kept(std::io::Error::other(error)))?
        .map_err(Error::Kept)
}
