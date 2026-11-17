//! A native composer subscription. Output acknowledges accepted updates; callers
//! retain their replica and repair both directions after reconnecting.
use crate::{CallError, Error, World};
use fastctx_types::{ResidentId, access::Written, composer::sync, session::Watched};

pub struct Documents {
    watching: Transport,
}
enum Transport {
    Local(crate::Watching),
    Network(fastctx_net::link::Watching),
}
#[derive(Clone)]
pub struct Input {
    transport: InputTransport,
}
#[derive(Clone)]
enum InputTransport {
    Local(crate::local_watch::Input),
    Network(fastctx_net::link::WatchInput),
}
impl Input {
    pub fn send(&self, input: sync::Input) -> Result<(), CallError> {
        match &self.transport {
            InputTransport::Local(sender) => sender.send(input),
            InputTransport::Network(sender) => sender.send(input).map_err(CallError::Failed),
        }
    }
}
impl Documents {
    pub fn input(&self) -> Option<Input> {
        let transport = match &self.watching {
            Transport::Local(watching) => match &watching.transport {
                crate::WatchingTransport::LocalMany(watching) => InputTransport::Local(watching.input()),
                _ => return None,
            },
            Transport::Network(watching) => InputTransport::Network(watching.input()?),
        };
        Some(Input { transport })
    }
    pub async fn next(&mut self) -> Result<Option<sync::Frame>, Error> {
        let piece = match &mut self.watching {
            Transport::Local(watching) => watching.next().await?,
            Transport::Network(watching) => watching.next().await.map_err(CallError::Failed)?,
        };
        match piece {
            Some(Watched::Documents { frame }) => Ok(Some(*frame)),
            None => Ok(None),
            _ => Err(crate::local_watch::older_documents().into()),
        }
    }
}
impl World {
    pub async fn watch_documents(
        &self,
        resident: ResidentId,
        machine: Option<Written>,
        name: String,
        subscription: sync::Subscription,
    ) -> Result<Documents, Error> {
        let watching = if let Some(device) = self.device() {
            Transport::Network(device.watch_documents(resident, machine, name, subscription).await?)
        } else {
            Transport::Local(
                self.local_watches
                    .watch(
                        self,
                        fastctx_types::watching::local::Subscription {
                            resident,
                            machine,
                            composer: Some(name),
                            documents: Some(subscription),
                            held: None,
                            now: false,
                            live_only: false,
                            goal: None,
                            collaboration: false,
                        },
                    )
                    .await?,
            )
        };
        Ok(Documents { watching })
    }
}
