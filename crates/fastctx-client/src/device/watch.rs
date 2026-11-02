//! Device watches share one authenticated network stream per account and machine.

use super::Device;
use crate::{CallError, Error};
use fastctx_types::ResidentId;
use fastctx_types::access::Written;
use fastctx_types::ipc::Failure;
use fastctx_types::session::Watched;

pub(crate) struct Watching(fastctx_net::link::Watching);

impl Device {
    pub(crate) async fn watch(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
        composer: Option<String>,
        held: Option<fastctx_types::session::held::Held>,
        now: bool,
        goal: Option<fastctx_types::access::HubAccount>,
    ) -> Result<Watching, Error> {
        self.open_watch(resident, machine, composer, held, now, goal, None, None).await.map(Watching)
    }

    /// Follows a conversation's collaboration on its machine, when that
    /// machine says it can (`MachineFacts::collaboration_watch`).
    pub(crate) async fn watch_collaboration(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
    ) -> Result<Watching, Error> {
        let follow = Some(fastctx_types::collaboration::Follow::Tree);
        self.open_watch(resident, machine, None, None, false, None, None, follow).await.map(Watching)
    }

    /// Requests native documents on the same account/machine observation stream.
    /// A legacy peer still returns its ordinary composer snapshots; callers enable
    /// native input only after a versioned Documents state confirms support.
    pub async fn watch_documents(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
        name: String,
        subscription: fastctx_types::composer::sync::Subscription,
    ) -> Result<fastctx_net::link::Watching, Error> {
        self.open_watch(resident, machine, Some(name), None, false, None, Some(subscription), None).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn open_watch(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
        composer: Option<String>,
        held: Option<fastctx_types::session::held::Held>,
        now: bool,
        goal: Option<fastctx_types::access::HubAccount>,
        documents: Option<fastctx_types::composer::sync::Subscription>,
        collaboration: Option<fastctx_types::collaboration::Follow>,
    ) -> Result<fastctx_net::link::Watching, Error> {
        let machine = self.find(&resident, machine).await?;
        let session = self.reaching(&machine)?;
        let machine = machine.name;
        let scoped = goal.is_some();
        let account = match goal {
            Some(goal) => {
                if goal.hub != session.link.membership().id {
                    return Err(Error::Unexpected("the goal account belongs to another hub".into()));
                }
                session
                    .link
                    .view()
                    .admits(
                        &session.link.membership().name,
                        &goal.account,
                        &machine,
                        fastctx_types::access::Purpose::Agents,
                    )
                    .map_err(|e| Error::Unexpected(e.to_string()))?;
                goal.account
            }
            None => session.account_for(&machine, fastctx_types::access::Purpose::Agents)?,
        };
        if collaboration.is_some() {
            let facts = session
                .link
                .call(
                    &machine,
                    &account,
                    fastctx_types::access::Purpose::Look,
                    fastctx_types::remote::RemoteCall::Facts,
                )
                .await
                .map_err(|error| CallError::Failed(Failure::new(error.to_string())))?
                .map_err(CallError::Failed)?;
            if !matches!(facts, fastctx_types::remote::RemoteReply::Facts(facts) if facts.collaboration_watch) {
                return Err(CallError::Failed(Failure::not_run(fastctx_types::collab_prompt::WATCH_MACHINE_OLD)).into());
            }
        }
        let request = fastctx_types::watching::Subscription {
            resident,
            composer,
            documents,
            held,
            now,
            goal: scoped,
            collaboration,
        };
        session
            .link
            .watch_shared(&machine, &account, request)
            .await
            .map_err(|failure: Failure| CallError::Failed(failure).into())
    }
}

impl Watching {
    pub async fn next(&mut self) -> Result<Option<Watched>, CallError> {
        self.0.next().await.map_err(CallError::Failed)
    }
}
