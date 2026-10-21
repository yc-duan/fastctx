//! Live ownership of a line source, and cancellable independent requests.

use crate::{Client, Error, World};
use fastctx_types::access::Written;
use fastctx_types::lines::{Decoding, Reply as LinesReply, Request, Result as Answer};
use fastctx_types::{Call, Reply};
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct Sources {
    generation: u64,
    held: HashMap<(Written, String), Held>,
}
pub(crate) enum Held {
    Local { _client: Client },
    Remote { _lease: fastctx_net::lines::Lease },
    Kept { cache: std::sync::Arc<crate::cache::Cache>, key: String, closed: tokio::sync::watch::Sender<()> },
}

impl World {
    pub fn close_line_sources(&self) {
        let mut sources = self.line_sources.lock().expect("line sources");
        sources.generation += 1;
        sources.held.clear();
    }

    /// Open once, retain the returned machine/source pair, close on leaving
    /// the viewer. Dropping the last World also closes all owned sources.
    pub async fn open_lines(
        &self,
        machine: Option<Written>,
        path: String,
        decoding: Decoding,
    ) -> Result<LinesReply, Error> {
        // 2026-10-21: historical artifact indexes carry no hub, so they cannot
        // identify the cached file of a hub-qualified source.
        if let Some(cache) = self.cache().cloned()
            && let Some(machine) = machine.clone()
            && machine.hub.is_none()
            && let Some(frozen) = cache.frozen(&machine.name, &path, &decoding)
        {
            return self.open_frozen_lines(cache, frozen, machine, path, decoding).await;
        }
        self.open_network_lines(machine, path, decoding).await
    }

    pub(crate) async fn open_network_lines(
        &self,
        machine: Option<Written>,
        path: String,
        decoding: Decoding,
    ) -> Result<LinesReply, Error> {
        let generation = self.line_sources.lock().expect("line sources").generation;
        let selected = machine.clone();
        let (reply, held) = if let Some(device) = self.device() {
            let machine = machine.ok_or_else(|| Error::Other("choose the machine holding the text file".into()))?;
            let (reply, lease) = device.open_lines(machine, path, decoding).await?;
            (reply, Held::Remote { _lease: lease })
        } else {
            let mut client = self.connect().await?;
            let reply = client.call(Call::Lines { machine, request: Request::Open { path, decoding } }).await?;
            let Reply::Lines(reply) = reply else {
                return Err(Error::Unexpected("line source".into()));
            };
            (reply, Held::Local { _client: client })
        };
        let Answer::Opened { source, .. } = &reply.result else {
            return Err(Error::Unexpected("opened line source".into()));
        };
        let mut sources = self.line_sources.lock().expect("line sources");
        if sources.generation != generation {
            return Err(Error::Other("the page owning this line source closed while it opened".into()));
        }
        sources.held.insert((selected.unwrap_or_else(|| reply.machine.clone().into()), source.clone()), held);
        Ok(reply)
    }

    /// Read, find, or await a growth notification. Cancellation drops the
    /// request's separate IPC/network call, leaving the source open.
    pub async fn lines(&self, machine: Written, request: Request) -> Result<LinesReply, Error> {
        let source = match &request {
            Request::Read { source, .. }
            | Request::Find { source, .. }
            | Request::Changed { source, .. }
            | Request::Close { source } => source,
            Request::Open { .. } => return Err(Error::Other("use open_lines to retain its owning connection".into())),
        };
        let closing = matches!(request, Request::Close { .. });
        let kept = {
            let sources = self.line_sources.lock().expect("line sources");
            match sources.held.get(&(machine.clone(), source.clone())) {
                Some(Held::Kept { cache, key, closed }) => Some((cache.clone(), key.clone(), closed.subscribe())),
                _ => None,
            }
        };
        if let Some((cache, key, mut closed)) = kept {
            if closing {
                self.line_sources.lock().expect("line sources").held.remove(&(machine.clone(), source.clone()));
                return Ok(LinesReply { machine: machine.name, result: Answer::Closed });
            }
            if matches!(request, Request::Changed { .. }) {
                let _ = closed.changed().await;
                return Err(Error::Other("the kept line source closed".into()));
            }
            return self.kept_lines(cache, key, machine, request).await;
        }
        if !self.line_sources.lock().expect("line sources").held.contains_key(&(machine.clone(), source.clone())) {
            if closing {
                return Ok(LinesReply { machine: machine.name, result: Answer::Closed });
            }
            return Err(Error::Other("this World does not own the line source".into()));
        }
        let id = source.clone();
        if let Request::Read { source, start, count } = request {
            return self.read_lines(machine, source, start, count).await;
        }
        let answer = self.call(Call::Lines { machine: Some(machine.clone()), request }).await;
        if closing {
            self.line_sources.lock().expect("line sources").held.remove(&(machine, id));
        }
        match answer?.0 {
            Reply::Lines(reply) => Ok(reply),
            _ => Err(Error::Unexpected("line reply".into())),
        }
    }
}

impl Sources {
    pub(crate) fn remove(&mut self, machine: &Written, source: &str) {
        self.held.remove(&(machine.clone(), source.to_string()));
    }
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub(crate) fn keep(
        &mut self,
        generation: u64,
        machine: Written,
        source: String,
        cache: std::sync::Arc<crate::cache::Cache>,
        key: String,
    ) -> Result<(), Error> {
        if self.generation != generation {
            return Err(Error::Other("the page owning this line source closed while it opened".into()));
        }
        let (closed, _) = tokio::sync::watch::channel(());
        self.held.insert((machine, source), Held::Kept { cache, key, closed });
        Ok(())
    }
}
