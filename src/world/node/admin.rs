//! The daemon's local admin channel: `fastctx world …` and `fastctx node status` ask the
//! running daemon over a private IPC endpoint instead of opening a second hub connection,
//! which would replace the daemon's session.

use crate::runtime::local_ipc::{self, BoxedStream, Listener, LocalEndpoint};
use crate::world::client::{LinkStatus, NodeView, WorldClient};
use crate::world::dispatch::{NodeCall, call_node};
use crate::world::messages::{CallBudget, CallOrigin, WireResponse};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

/// How long a control command waits for the daemon's answer.
pub(crate) const ADMIN_TIMEOUT: Duration = Duration::from_secs(40);

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum AdminRequest {
    Network {
        request: crate::world::resident::Request,
    },
    Status,
    /// Reads the node's in-memory control view without contacting the hub.
    Snapshot,
    Nodes,
    /// Runs one verb on one machine on behalf of `fastctx world run` / `copy`: the script door.
    /// The answer is the outcome as the tool surface would see it, plus its facts.
    Call {
        verb: String,
        node: String,
        args: serde_json::Value,
        timeout_ms: u64,
    },
    Invite {
        name: Option<String>,
        ttl_hours: u32,
        hubs: Option<Vec<String>>,
    },
    Revoke {
        name: String,
    },
    Events {
        since: u64,
        limit: u32,
    },
    Refresh,
    Reconnect,
    /// Saves this machine's user-authored description and queues signed inventory.
    SetDescription {
        text: String,
    },
    /// Stops this daemon, including a node started without a service registration.
    Shutdown,
    Leave,
    /// Publishes a grant, or removes the one named by `id` when `delete` is set.
    Grant {
        id: Option<String>,
        principal: String,
        nodes: Vec<String>,
        verbs: Vec<String>,
        expires: Option<String>,
        delete: bool,
        /// A TUI confirmation is valid only for the revision the user reviewed.
        #[serde(default)]
        expected_revision: Option<u64>,
    },
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct AdminResponse {
    pub(crate) ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    #[serde(default)]
    pub(crate) data: serde_json::Value,
}

/// What `AdminRequest::Call` answers with: a `NodeOutcome` on the wire.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct CallOutcome {
    pub(crate) node: String,
    pub(crate) status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) response: Option<WireResponse>,
    #[serde(default)]
    pub(crate) facts: BTreeMap<String, serde_json::Value>,
}

/// What `status` returns: link plus identity facts.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct NodeStatus {
    pub(crate) name: String,
    pub(crate) world_id: String,
    pub(crate) hub: Vec<String>,
    pub(crate) hub_key: String,
    pub(crate) fingerprint: String,
    pub(crate) pid: u32,
    /// The OS creation token disambiguates a live process from a reused status-file PID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) process_identity: Option<crate::process_identity::ProcessIdentity>,
    pub(crate) version: String,
    pub(crate) started_at: String,
    pub(crate) written_at: String,
    pub(crate) link: LinkStatus,
    pub(crate) members: usize,
    pub(crate) members_online: usize,
    /// Revision of the grant snapshot in force; 0 when none was ever published.
    pub(crate) grant_revision: u64,
    /// A newer snapshot is known to exist; calls from other members are refused until it lands.
    pub(crate) grant_stale: bool,
    pub(crate) running_calls: usize,
    pub(crate) engine_hosted: bool,
    /// Local agent transport connections; older node builds omit this counter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent_connections: Option<usize>,
    /// Collection or publication failed; the previous inventory remains in force.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) inventory_error: Option<String>,
}

pub(crate) struct AdminServer {
    client: Arc<WorldClient>,
    executor: Arc<super::executor::Executor>,
    engine_hosted: Arc<std::sync::atomic::AtomicBool>,
}

impl AdminServer {
    pub(crate) fn new(
        client: Arc<WorldClient>,
        executor: Arc<super::executor::Executor>,
        engine_hosted: Arc<std::sync::atomic::AtomicBool>,
    ) -> Arc<Self> {
        Arc::new(Self {
            client,
            executor,
            engine_hosted,
        })
    }

    pub(crate) fn status(&self) -> NodeStatus {
        let config = self.client.config.read().clone();
        let members = self.client.members.read();
        let activity = self.client.activity(self.executor.running_calls());
        NodeStatus {
            name: config.name.clone(),
            world_id: config.world_id.clone(),
            hub: config.hub.clone(),
            hub_key: config.hub_key.clone(),
            fingerprint: self.client.identity.fingerprint().to_string(),
            pid: std::process::id(),
            process_identity: crate::process_identity::process_identity(std::process::id()),
            version: env!("CARGO_PKG_VERSION").to_string(),
            started_at: self.client.started_at.clone(),
            written_at: crate::world::now_rfc3339(),
            link: self.client.link(),
            members: members
                .members
                .values()
                .filter(|member| member.is_current())
                .count(),
            members_online: members
                .members
                .values()
                .filter(|member| member.is_online())
                .count(),
            grant_revision: self.client.grant_revision(),
            grant_stale: self.client.is_grant_stale(),
            running_calls: self.executor.running_calls(),
            engine_hosted: self
                .engine_hosted
                .load(std::sync::atomic::Ordering::Relaxed)
                && activity.engine_hosted,
            agent_connections: Some(activity.agent_connections),
            inventory_error: self.client.inventory_error.read().clone(),
        }
    }

    /// Serves admin requests until the token fires.
    pub(crate) async fn run(
        self: Arc<Self>,
        endpoint: LocalEndpoint,
        shutdown: CancellationToken,
    ) -> Result<(), String> {
        crate::edit::private_storage::ensure_private_directory(
            endpoint.runtime_directory(),
            "node admin",
        )?;
        let mut listener = Listener::bind(&endpoint)?;
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return Ok(()),
                accepted = listener.accept() => match accepted {
                    Ok(stream) => {
                        let server = Arc::clone(&self);
                        tokio::spawn(async move { server.serve(stream).await });
                    }
                    Err(error) => {
                        super::log(format!("admin channel accept failed: {error}"));
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
        }
    }

    async fn serve(&self, mut stream: BoxedStream) {
        let request = match read_frame(&mut stream).await {
            Ok(bytes) => bytes,
            Err(_) => return,
        };
        let response = match serde_json::from_slice::<AdminRequest>(&request) {
            Ok(request) => {
                let mut disconnected = [0];
                tokio::select! {
                    response = self.handle(request) => response,
                    _ = stream.read(&mut disconnected) => return,
                }
            }
            Err(error) => AdminResponse {
                ok: false,
                error: Some(format!("Unreadable admin request: {error}")),
                data: serde_json::Value::Null,
            },
        };
        let _ = write_frame(
            &mut stream,
            &serde_json::to_vec(&response).unwrap_or_default(),
        )
        .await;
    }

    async fn handle(&self, request: AdminRequest) -> AdminResponse {
        let result: Result<serde_json::Value, String> = match request {
            AdminRequest::Network { request } => crate::world::resident::handle(&self.client, request).await,
            AdminRequest::Status => {
                serde_json::to_value(self.status()).map_err(|error| error.to_string())
            }
            AdminRequest::Snapshot => {
                use crate::world::control::{NodeSnapshot, ReadState};
                let mut nodes = self.client.all_node_views();
                let nodes = ReadState::result(
                    crate::world::notes::annotate(&self.client.paths, &mut nodes).map(|()| nodes),
                );
                let grants = self.client.grants.read().clone();
                let snapshot = NodeSnapshot {
                    status: self.status(),
                    nodes,
                    grants: crate::world::grant::GrantSnapshot {
                        revision: grants.revision,
                        grants: grants.grants,
                        published_by: grants.published_by,
                        published_at: grants.published_at,
                    },
                    description: ReadState::result(crate::world::notes::description(
                        &self.client.paths,
                    )),
                    unverified: self
                        .client
                        .members
                        .read()
                        .unverified
                        .iter()
                        .map(|(name, reason)| (name.clone(), reason.clone()))
                        .collect(),
                };
                serde_json::to_value(snapshot).map_err(|error| error.to_string())
            }
            AdminRequest::Nodes => {
                let _ = self.client.refresh_members().await;
                let _ = self.client.refresh_inventories().await;
                let mut nodes = self.client.nodes();
                crate::world::notes::annotate(&self.client.paths, &mut nodes)
                    .and_then(|()| serde_json::to_value(nodes).map_err(|error| error.to_string()))
            }
            AdminRequest::Call {
                verb,
                node,
                args,
                timeout_ms,
            } => {
                let call = NodeCall {
                    verb,
                    node,
                    args,
                    budget: CallBudget::default(),
                    timeout: Duration::from_millis(
                        timeout_ms.clamp(1_000, crate::shell::MAX_SCRIPT_CALL_MS),
                    ),
                    origin: CallOrigin::Script,
                };
                call_node(&self.client, self.executor.session(), call)
                    .await
                    .and_then(|outcome| {
                        serde_json::to_value(CallOutcome {
                            node: outcome.node,
                            status: outcome.status,
                            message: outcome.message,
                            response: outcome.response.as_ref().map(WireResponse::from),
                            facts: outcome.facts,
                        })
                        .map_err(|error| error.to_string())
                    })
            }
            AdminRequest::Invite {
                name,
                ttl_hours,
                hubs,
            } => {
                if !self.client.is_connected() {
                    Err(self.client.unreachable_error())
                } else {
                    self.client
                        .create_invite(
                            name,
                            time::Duration::hours(i64::from(ttl_hours.max(1))),
                            hubs,
                        )
                        .map(serde_json::Value::String)
                }
            }
            AdminRequest::Revoke { name } => self
                .client
                .revoke(&name)
                .await
                .map(|()| serde_json::json!({ "revoked": true })),
            AdminRequest::Events { since, limit } => self
                .client
                .fetch_events(since, limit)
                .await
                .and_then(|events| serde_json::to_value(events).map_err(|error| error.to_string())),
            AdminRequest::Refresh => {
                async {
                    let members = self.client.refresh_members().await?;
                    let (inventories,grants) = tokio::join!(self.client.refresh_inventories(),self.client.refresh_grants());
                    Ok(serde_json::json!({"members_version":members,"inventories_updated":inventories?,"grants_updated":grants?}))
                }.await
            }
            AdminRequest::Reconnect => {
                self.client.wake.notify_one();
                Ok(serde_json::Value::Null)
            }
            AdminRequest::Shutdown => {
                let shutdown = self.client.shutdown.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    shutdown.cancel();
                });
                Ok(serde_json::Value::Null)
            }
            AdminRequest::SetDescription { text } => {
                match crate::world::notes::save_description(&self.client.paths, &text) {
                    Err(error) => Err(error),
                    Ok(()) => {
                        let publication = super::inventory::publish(&self.client, false).await;
                        Ok(serde_json::json!({
                            "saved": true,
                            "queued": publication.is_ok(),
                            "connected": self.client.is_connected(),
                            "error": publication.err(),
                        }))
                    }
                }
            }
            AdminRequest::Grant {
                id,
                principal,
                nodes,
                verbs,
                expires,
                delete,
                expected_revision,
            } => {
                let id = match id {
                    Some(id) => id,
                    None => match crate::world::crypto::random_bytes::<3>() {
                        Ok(bytes) => format!("g-{}",hex::encode(bytes)),
                        Err(error) => return AdminResponse{ok:false,error:Some(error),data:serde_json::Value::Null},
                    },
                };
                let change = if delete {
                    crate::world::grant::GrantChange::Remove(id.clone())
                } else {
                    crate::world::grant::GrantChange::Set(crate::world::grant::GrantEntry {
                        id: id.clone(),
                        grant: crate::world::grant::Grant {
                            principal,
                            nodes,
                            verbs,
                            expires,
                        },
                    })
                };
                self.client
                    .change_grants_at(change, expected_revision)
                    .await
                    .map(|revision| serde_json::json!({ "id": id, "revision": revision }))
            }
            AdminRequest::Leave => self.client.leave().map(|()| serde_json::Value::Null),
        };
        match result {
            Ok(data) => AdminResponse {
                ok: true,
                error: None,
                data,
            },
            Err(error) => AdminResponse {
                ok: false,
                error: Some(error),
                data: serde_json::Value::Null,
            },
        }
    }
}

pub(crate) fn node_views_from(value: serde_json::Value) -> Result<Vec<NodeView>, String> {
    serde_json::from_value(value).map_err(|error| format!("Unreadable node list: {error}"))
}

async fn read_frame(stream: &mut BoxedStream) -> Result<Vec<u8>, String> {
    let mut length = [0_u8; 4];
    stream
        .read_exact(&mut length)
        .await
        .map_err(|error| format!("cannot read the admin frame length: {error}"))?;
    let length = u32::from_be_bytes(length) as usize;
    let mut bytes = vec![0_u8; length];
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(|error| format!("cannot read the admin frame: {error}"))?;
    Ok(bytes)
}

async fn write_frame(stream: &mut BoxedStream, bytes: &[u8]) -> Result<(), String> {
    let length =
        u32::try_from(bytes.len()).map_err(|_| "the admin frame is too large".to_string())?;
    stream
        .write_all(&length.to_be_bytes())
        .await
        .map_err(|error| format!("cannot write the admin frame length: {error}"))?;
    stream
        .write_all(bytes)
        .await
        .map_err(|error| format!("cannot write the admin frame: {error}"))?;
    stream
        .flush()
        .await
        .map_err(|error| format!("cannot flush the admin frame: {error}"))
}

/// Sends one request to the running daemon and waits up to `timeout` for its answer;
/// `Ok(None)` when no daemon answers the endpoint at all.
pub(crate) async fn call(
    endpoint: &LocalEndpoint,
    request: &AdminRequest,
    timeout: Duration,
) -> Result<Option<AdminResponse>, String> {
    let mut stream = match local_ipc::connect(endpoint).await {
        Ok(stream) => stream,
        Err(error)
            if crate::world::control::Failure::new(&error).kind
                == crate::world::control::FailureKind::Permission =>
        {
            return Err(error);
        }
        Err(_) => return Ok(None),
    };
    let bytes = serde_json::to_vec(request).map_err(|error| error.to_string())?;
    write_frame(&mut stream, &bytes).await?;
    let response = tokio::time::timeout(timeout, read_frame(&mut stream))
        .await
        .map_err(|_| {
            format!(
                "The node service did not answer within {} seconds.",
                timeout.as_secs()
            )
        })??;
    serde_json::from_slice(&response)
        .map(Some)
        .map_err(|error| format!("Unreadable answer from the node service: {error}"))
}

/// Resident operations may wait for an event; the caller controls cancellation by closing IPC.
pub(crate) async fn network_call(
    endpoint: &LocalEndpoint,
    request: &crate::world::resident::Request,
) -> Result<AdminResponse, String> {
    let mut stream = local_ipc::connect(endpoint).await?;
    let bytes = serde_json::to_vec(&serde_json::json!({"op": "network", "request": request}))
        .map_err(|e| e.to_string())?;
    write_frame(&mut stream, &bytes).await?;
    serde_json::from_slice(&read_frame(&mut stream).await?).map_err(|e| e.to_string())
}
