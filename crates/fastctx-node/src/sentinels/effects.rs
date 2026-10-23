//! Node authority and local model/web clients for the home-side runtime.
use crate::Node;
use fastctx_agent::{
    FutureResult,
    model::{Model, client},
};
use fastctx_sentinel::{
    ports::{Look, ReadResult, Web},
    resident::{Capabilities, Runtime},
    web::{Audit, PublicWeb, Search, Tavily},
};
use fastctx_types::{MachineName, access::Acting, sentinel::Kind, sentinel_control::Registration};
use std::{
    path::Path,
    sync::{Arc, Weak},
};

pub(super) struct Effects(pub Weak<Node>);
impl Effects {
    fn node(&self) -> Result<Arc<Node>, String> {
        self.0.upgrade().ok_or("the sentinel node has stopped".into())
    }
}
impl Runtime for Effects {
    fn now(&self) -> i64 {
        jiff::Timestamp::now().as_millisecond()
    }
    fn assigned<'a>(&'a self, registration: &'a Registration) -> FutureResult<'a, bool> {
        Box::pin(async move {
            let node = self.node()?;
            let link = node.link_of(&registration.scope.hub).ok_or("the sentinel account's hub is not connected")?;
            let view = crate::authorization::current_view(&link).map_err(|e| e.message)?;
            if !view.is_me(&registration.definition.home) {
                return Ok(false);
            }
            node.authorize_here(&Acting::one(registration.scope.hub.clone(), registration.scope.account.clone()))
                .map_err(|e| e.message)?;
            settings(&node, registration).await?;
            Ok(true)
        })
    }
    fn online<'a>(&'a self, registration: &'a Registration, machine: &'a MachineName) -> FutureResult<'a, bool> {
        Box::pin(async move {
            let node = self.node()?;
            let link = node.link_of(&registration.scope.hub).ok_or("the observation account's hub is not connected")?;
            let view = crate::authorization::current_view(&link).map_err(|e| e.message)?;
            let member = view.member(machine).ok_or("the observed machine is not in this account's view")?;
            if !view.rights(&registration.scope.account, machine).hands {
                return Err("this account may no longer use the observed machine".into());
            }
            Ok(member.online)
        })
    }
    fn capabilities<'a>(
        &'a self,
        registration: &'a Registration,
        directory: &'a Path,
    ) -> FutureResult<'a, Capabilities> {
        Box::pin(async move {
            let node = self.node()?;
            if registration.definition.kind == Kind::Web {
                Ok(Capabilities {
                    look: Box::new(Disabled),
                    web: Box::new(PublicWeb {
                        audit: Audit::new(directory.join("web-audit")),
                        search: Box::new(NodeSearch {
                            node: Arc::downgrade(&node),
                            registration: registration.clone(),
                            directory: directory.to_path_buf(),
                        }),
                    }),
                    machines: Vec::new(),
                })
            } else {
                // Every machine its account works on, as the agents of that account are given them.
                let link = node.link_of(&registration.scope.hub).ok_or("the observation account's hub is not connected")?;
                let view = crate::authorization::current_view(&link).map_err(|e| e.message)?;
                let account = &registration.scope.account;
                let machines = view
                    .machines()
                    .filter(|member| view.rights(account, &member.name).hands)
                    .map(|member| fastctx_sentinel::ports::Machine { name: member.name.clone(), online: member.online })
                    .collect();
                Ok(Capabilities {
                    look: Box::new(super::look::MachineLook {
                        node,
                        scope: registration.scope.clone(),
                        directory: directory.join("observations"),
                    }),
                    web: Box::new(Disabled),
                    machines,
                })
            }
        })
    }
    fn model<'a>(&'a self, registration: &'a Registration) -> FutureResult<'a, Box<dyn Model>> {
        Box::pin(async move {
            let node = self.node()?;
            let settings = settings(&node, registration).await?;
            let model = settings.model.ok_or("the sentinel model is not configured")?;
            let credentials = node.sentinels.credentials.get().ok_or("sentinel credential service is not connected")?;
            let key = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                credentials.model_key(&registration.scope, &registration.definition),
            )
            .await
            .map_err(|_| "reading the sentinel model key timed out")??;
            Ok(client(model, key))
        })
    }
}
async fn settings(
    node: &Node,
    registration: &Registration,
) -> Result<fastctx_types::sentinel_settings::Settings, String> {
    let records = node.sentinels.records().map_err(|e| e.message)?;
    let (_, settings) =
        fastctx_sentinel::settings::read(records, &registration.scope).await.map_err(|e| e.to_string())?;
    if settings.home.as_ref().is_none_or(|home| !node.is_me(home)) {
        return Err(
            "the sentinel home setting has changed or is disabled; withdraw this sentinel or restore its home".into()
        );
    }
    Ok(settings)
}
struct NodeSearch {
    node: Weak<Node>,
    registration: Registration,
    directory: std::path::PathBuf,
}
impl Search for NodeSearch {
    fn search<'a>(&'a self, query: &'a str) -> FutureResult<'a, ReadResult> {
        Box::pin(async move {
            let node = self.node.upgrade().ok_or("the sentinel node has stopped")?;
            let settings = settings(&node, &self.registration).await?;
            let search = settings.search.ok_or("sentinel search is not configured")?;
            let credentials = node.sentinels.credentials.get().ok_or("sentinel credential service is not connected")?;
            let key = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                credentials.search_key(&self.registration.scope, &self.registration.definition),
            )
            .await
            .map_err(|_| "reading the sentinel search key timed out")??;
            Tavily { endpoint: search.endpoint, key, audit: Audit::new(self.directory.join("web-audit")) }
                .search(query)
                .await
        })
    }
}
struct Disabled;
impl Look for Disabled {
    fn look<'a>(
        &'a self,
        _: &'a fastctx_types::sentinel::Definition,
        _: &'a MachineName,
        _: &'a str,
    ) -> FutureResult<'a, ReadResult> {
        Box::pin(async { Err("machine observations are not available to this sentinel".into()) })
    }
}
impl Web for Disabled {
    fn fetch<'a>(&'a self, _: &'a str) -> FutureResult<'a, ReadResult> {
        Box::pin(async { Err("web access is not available to this sentinel".into()) })
    }
    fn search<'a>(&'a self, _: &'a str) -> FutureResult<'a, ReadResult> {
        Box::pin(async { Err("web access is not available to this sentinel".into()) })
    }
}
