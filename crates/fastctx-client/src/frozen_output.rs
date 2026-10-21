//! Completed output shares its canonical text; each job keeps its own annotations.

use crate::{
    Error, World,
    artifact_sink::{ArtifactSink, Pending, SourceOwner},
    cache::{
        Cache,
        lines::{self, Frozen, Index},
    },
};
use fastctx_types::access::Written;
use fastctx_types::lines::{Decoding, Part, Reply, Request, Result as Answer};
use std::sync::Arc;

impl World {
    pub(crate) async fn open_frozen_output(
        &self,
        cache: Arc<Cache>,
        frozen: Frozen,
        machine: Written,
        path: String,
        decoding: Decoding,
    ) -> Result<Reply, Error> {
        let source_generation = self.line_sources.lock().expect("line sources").generation();
        let reading = cache.clone();
        let key = frozen.key.clone();
        let found = blocking(move || {
            let _turn = reading.turn()?;
            lines::index(reading.root(), &key)
        })
        .await?;
        let index = match found {
            Some(index) => index,
            None => self.fetch_frozen_output(cache.clone(), &frozen, &machine, &path, decoding).await?,
        };
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

    async fn fetch_frozen_output(
        &self,
        cache: Arc<Cache>,
        frozen: &Frozen,
        machine: &Written,
        path: &str,
        decoding: Decoding,
    ) -> Result<Index, Error> {
        let body = frozen.body.clone().expect("an output has a body");
        let locked = cache.clone();
        let (body_key, key) = (body.clone(), frozen.key.clone());
        let (request, generation, found, known_body) = blocking(move || {
            let request = locked.request(&format!("output-body:{body_key}"))?;
            let turn = locked.turn()?;
            Ok((
                request,
                turn.generation(),
                lines::index(locked.root(), &key)?,
                lines::index(locked.root(), &body_key)?,
            ))
        })
        .await?;
        if let Some(index) = found {
            return Ok(index);
        }
        let reply = self
            .open_network_lines(
                Some(machine.clone()),
                path.into(),
                Decoding::Immutable {
                    content: frozen.content.clone(),
                    policy: frozen.policy.clone(),
                    annotation: frozen.annotation.clone(),
                    decoding: Box::new(decoding),
                },
            )
            .await?;
        let Answer::Opened { source, extent, .. } = reply.result else {
            return Err(Error::Unexpected("opened immutable output source".into()));
        };
        let _owner =
            SourceOwner { sources: self.line_sources.clone(), machine: machine.clone(), source: source.clone() };
        if !extent.complete {
            return Err(Error::Unexpected("an immutable artifact is still growing".into()));
        }
        if let Some(known) = &known_body
            && (known.body.is_some() || known.extent.length != extent.length)
        {
            return Err(Error::Unexpected("the output body disagrees with its history's decoding".into()));
        }
        let random = fastctx_net::sign::random::<16>().map_err(Error::Kept)?;
        let folder = cache.root().join(".incoming").join(crate::cache::key(&format!("{random:?}")));
        let pending = Pending(folder.clone());
        let notes_folder = folder.join("notes");
        let body_folder = folder.join("body");
        let text = if known_body.is_none() {
            Some(ArtifactSink::create(&body_folder, extent.clone(), None).await?)
        } else {
            None
        };
        let mut sink =
            OutputSink { text, notes: ArtifactSink::create(&notes_folder, extent.clone(), Some(body.clone())).await? };
        let transfer = if known_body.is_some() {
            self.visit_line_notes(machine.clone(), source.clone(), extent.length, &mut sink).await
        } else {
            self.visit_lines(machine.clone(), source.clone(), 0, extent.length, &mut sink).await
        };
        let closed = self.lines(machine.clone(), Request::Close { source }).await;
        let reply = transfer?;
        closed?;
        if reply.result != (Answer::ReadComplete { start: 0, count: extent.length })
            || !sink.notes.complete()
            || sink.text.as_ref().is_some_and(|text| !text.complete())
        {
            return Err(Error::Unexpected("immutable line completion disagrees with its parts".into()));
        }
        sink.notes.sync().await?;
        if let Some(text) = &mut sink.text {
            text.sync().await?;
        }
        let OutputSink { text, notes } = sink;
        let ArtifactSink { output, index, .. } = notes;
        drop(output);
        let mut staged = Vec::new();
        if let Some(text) = text {
            let ArtifactSink { output, index, .. } = text;
            drop(output);
            staged.push((
                body.clone(),
                body_folder,
                serde_json::to_vec(&index).map_err(|error| Error::Other(error.to_string()))?,
            ));
        }
        staged.push((
            frozen.key.clone(),
            notes_folder,
            serde_json::to_vec(&index).map_err(|error| Error::Other(error.to_string()))?,
        ));
        let shared = cache.clone();
        blocking(move || {
            let _request = request;
            let _pending = pending;
            let turn = shared.turn()?;
            if turn.generation() != generation {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "the viewed cache was cleared during the output read",
                ));
            }
            if known_body.is_some() && lines::index(shared.root(), &body)?.is_none() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "the kept output body was cleared during its annotation read",
                ));
            }
            fastctx_platform::ensure_private_dir(&shared.root().join("blobs"))?;
            for (key, folder, index) in staged {
                fastctx_platform::publish(&folder.join("index.json"), &index, fastctx_platform::Permissions::Private)?;
                std::fs::rename(&folder, shared.root().join("blobs").join(key))?;
            }
            Ok(())
        })
        .await?;
        Ok(index)
    }
}

struct OutputSink {
    text: Option<ArtifactSink>,
    notes: ArtifactSink,
}
impl crate::line_rows::Sink for OutputSink {
    async fn put(&mut self, mut part: Part) -> Result<(), Error> {
        if let Some(text) = &mut self.text {
            if part.utf16_length.is_some() {
                return Err(Error::Unexpected("output annotations arrived where text was requested".into()));
            }
            let notes = Part {
                line: part.line,
                text: String::new(),
                utf16_length: Some(part.text.encode_utf16().count() as u64),
                ended: part.ended,
                note: part.note.take(),
            };
            text.put(part).await?;
            self.notes.put(notes).await
        } else {
            if part.utf16_length.is_none() || !part.text.is_empty() {
                return Err(Error::Unexpected("output text arrived where only annotations were requested".into()));
            }
            self.notes.put(part).await
        }
    }
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> std::io::Result<T> + Send + 'static) -> Result<T, Error> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| Error::Kept(std::io::Error::other(error)))?
        .map_err(Error::Kept)
}
