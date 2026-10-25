//! Host-specific selection and operations. The policy core has no host names.

use super::ReleaseReason;
use crate::Node;
use fastctx_types::{ipc::Failure, residents::About};
use std::sync::Arc;

pub(crate) enum Adapter {
    Claude,
    Codex,
}

pub(crate) enum Candidate {
    Worker(fastctx_hosts::claude::lifecycle::IdleWorker),
    Subscription,
}

impl Adapter {
    pub(crate) fn for_conversation(about: &About) -> Option<Self> {
        match about.host.agent.as_str() {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    pub(crate) async fn wake(
        &self,
        node: &Arc<Node>,
        about: &About,
        prompt: Option<(fastctx_types::NoticeId, String)>,
    ) -> Result<Option<String>, super::wake::WakeFailure> {
        let session = about.session.as_deref().ok_or_else(|| Failure::new("the host session is unknown"))?;
        match self {
            Self::Claude => {
                match prompt {
                    Some((id, text)) => {
                        crate::claude::wake(node, &about.id, about.home.as_deref(), session, text, id).await?;
                    }
                    None => crate::claude::resume::connect(node, &about.id).await?,
                }
                Ok(None)
            }
            Self::Codex => {
                node.codex.of(node, about)?.prepare(session).await.map_err(|error| Failure::new(error).into())
            }
        }
    }

    pub(crate) fn needs_subscription(&self, node: &Node, about: &About) -> Result<bool, Failure> {
        match self {
            Self::Claude => Ok(false),
            Self::Codex => {
                let Some(thread) = about.session.as_deref() else { return Ok(false) };
                Ok(!node.codex.of(node, about)?.subscribed(thread))
            }
        }
    }

    pub(crate) fn delayed(&self) -> bool {
        matches!(self, Self::Claude)
    }

    pub(crate) async fn candidate(&self, node: &Arc<Node>, about: &About) -> Result<Option<Candidate>, Failure> {
        match self {
            Self::Claude => {
                crate::claude::lifecycle::idle(node, about).await.map(|worker| worker.map(Candidate::Worker))
            }
            Self::Codex => {
                let host = node.codex.of(node, about)?;
                Ok(about.session.as_ref().filter(|thread| host.releasable(thread)).map(|_| Candidate::Subscription))
            }
        }
    }

    pub(crate) async fn release(
        &self,
        node: &Arc<Node>,
        about: &About,
        reason: ReleaseReason,
        candidate: Option<&Candidate>,
    ) -> Result<bool, Failure> {
        let Some(session) = &about.session else { return Ok(true) };
        match self {
            Self::Claude => {
                if let Some(Candidate::Worker(worker)) = candidate {
                    return crate::claude::lifecycle::collect(node, about, worker).await;
                }
                match reason {
                    ReleaseReason::Delete => {
                        crate::claude::management::retire(node, about.home.as_deref(), &about.id, session).await?
                    }
                    _ => {
                        crate::claude::management::stop(
                            node,
                            about.home.as_deref(),
                            &about.id,
                            session,
                            matches!(reason, ReleaseReason::Sleep),
                        )
                        .await?
                    }
                }
                Ok(true)
            }
            Self::Codex => {
                let host = node.codex.of(node, about)?;
                match reason {
                    ReleaseReason::Archive => host.archive(session).await.map(|()| true).map_err(Failure::new),
                    ReleaseReason::Delete => host.delete(session).await.map_err(Failure::new),
                    _ => host.sleep(session).await.map_err(Failure::new),
                }
            }
        }
    }
}
