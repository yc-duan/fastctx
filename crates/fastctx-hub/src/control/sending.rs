//! Rechecks queued disclosures at the socket boundary, below the split sink's buffer.

use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use fastctx_net::{directory_index::Indexed, sign::PublicKey};
use fastctx_types::{MachineName, access::TownId};
use futures_util::{Sink, Stream};
use tokio_tungstenite::tungstenite::{Error, Message};

use crate::{
    owned_records::hints::{self, Hint},
    store::Snapshot,
};

pub(super) enum Guard {
    Project(TownId),
    Record { hint: Box<Hint>, writer: Option<(MachineName, Vec<u8>)> },
}

impl Guard {
    fn message(
        self,
        directory: &Indexed<'_>,
        name: &MachineName,
        key: &PublicKey,
        message: Message,
    ) -> Option<Message> {
        match self {
            Self::Project(town) => (directory.member(name).is_some_and(|member| member.key == *key)
                && directory.member_in_project(name, &town))
            .then_some(message),
            Self::Record { hint, writer } => {
                if !hint.allows(directory, name, key) {
                    return None;
                }
                if let Some((by, redacted)) = writer
                    && !hints::sees_writer(directory, name, &by)
                {
                    return Some(Message::Binary(redacted.into()));
                }
                Some(message)
            }
        }
    }
}

pub(super) struct Outgoing {
    pub message: Message,
    pub guard: Option<Guard>,
}

impl From<Message> for Outgoing {
    fn from(message: Message) -> Self {
        Self { message, guard: None }
    }
}

pub(super) struct Socket<S> {
    inner: S,
    directory: Arc<arc_swap::ArcSwap<Snapshot>>,
    name: MachineName,
    key: PublicKey,
}

impl<S> Socket<S> {
    pub(super) fn new(inner: S, hub: &crate::Hub, name: MachineName, key: PublicKey) -> Self {
        Self { inner, directory: hub.catalog.reader(), name, key }
    }
}

impl<S: Sink<Message, Error = Error> + Unpin> Sink<Outgoing> for Socket<S> {
    type Error = Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.inner).poll_ready(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, outgoing: Outgoing) -> Result<(), Error> {
        let message = if let Some(guard) = outgoing.guard {
            let snapshot = self.directory.load();
            guard.message(&snapshot.indexed(), &self.name, &self.key, outgoing.message)
        } else {
            Some(outgoing.message)
        };
        match message {
            Some(message) => Pin::new(&mut self.inner).start_send(message),
            None => Ok(()),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }
}

impl<S: Stream + Unpin> Stream for Socket<S> {
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}
