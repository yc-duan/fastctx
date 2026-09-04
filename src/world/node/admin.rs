//! The daemon's local admin channel: `fastctx world …` and `fastctx node status` ask the
//! running daemon over a private IPC endpoint instead of opening a second hub connection,
//! which would replace the daemon's session.

use crate::runtime::local_ipc::{self, BoxedStream, Listener, LocalEndpoint};
use crate::world::client::{LinkStatus, NodeView, WorldClient};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

const MAX_ADMIN_FRAME: usize = 16 * 1024 * 1024;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum AdminRequest {
    Status,
    Nodes,
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
    Leave,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct AdminResponse {
    pub(crate) ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    #[serde(default)]
    pub(crate) data: serde_json::Value,
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
    pub(crate) version: String,
    pub(crate) started_at: String,
    pub(crate) written_at: String,
    pub(crate) link: LinkStatus,
    pub(crate) members: usize,
    pub(crate) members_online: usize,
    pub(crate) grant_version: u64,
    pub(crate) running_calls: usize,
    pub(crate) engine_hosted: bool,
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
        NodeStatus {
            name: config.name.clone(),
            world_id: config.world_id.clone(),
            hub: config.hub.clone(),
            hub_key: config.hub_key.clone(),
            fingerprint: self.client.identity.fingerprint().to_string(),
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            started_at: self.client.started_at.clone(),
            written_at: crate::world::now_rfc3339(),
            link: self.client.link(),
            members: members.members.len(),
            members_online: members
                .members
                .values()
                .filter(|member| member.is_online())
                .count(),
            grant_version: self.client.grants.read().version,
            running_calls: self.executor.running_calls(),
            engine_hosted: self
                .engine_hosted
                .load(std::sync::atomic::Ordering::Relaxed),
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
            Ok(request) => self.handle(request).await,
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
            AdminRequest::Status => {
                serde_json::to_value(self.status()).map_err(|error| error.to_string())
            }
            AdminRequest::Nodes => {
                let _ = self.client.refresh_members().await;
                let _ = self.client.refresh_inventories().await;
                serde_json::to_value(self.client.nodes()).map_err(|error| error.to_string())
            }
            AdminRequest::Invite {
                name,
                ttl_hours,
                hubs,
            } => {
                if !self.client.is_connected() {
                    Err(format!(
                        "hub_unreachable: {}",
                        self.client.unreachable_error()
                    ))
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
                .map(|epoch| serde_json::json!({ "epoch": epoch })),
            AdminRequest::Events { since, limit } => self
                .client
                .fetch_events(since, limit)
                .await
                .and_then(|events| serde_json::to_value(events).map_err(|error| error.to_string())),
            AdminRequest::Refresh => {
                let members = self.client.refresh_members().await;
                let inventories = self.client.refresh_inventories().await;
                members
                    .and(inventories)
                    .map(|updated| serde_json::json!({ "inventories_updated": updated }))
            }
            AdminRequest::Reconnect => {
                self.client.wake.notify_one();
                Ok(serde_json::Value::Null)
            }
            AdminRequest::Leave => {
                let header = crate::world::envelope::Header::new(
                    crate::world::messages::kind::LEAVE,
                    &self.client.name(),
                    crate::world::HUB_NAME,
                    0,
                );
                self.client
                    .send_reliable(header, &serde_json::json!({}), false, false)
                    .map(|_| serde_json::Value::Null)
            }
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
    if length > MAX_ADMIN_FRAME {
        return Err("the admin frame is too large".to_string());
    }
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

/// Sends one request to the running daemon; `Ok(None)` when no daemon answers.
pub(crate) async fn call(
    endpoint: &LocalEndpoint,
    request: &AdminRequest,
) -> Result<Option<AdminResponse>, String> {
    let mut stream = match local_ipc::connect(endpoint).await {
        Ok(stream) => stream,
        Err(_) => return Ok(None),
    };
    let bytes = serde_json::to_vec(request).map_err(|error| error.to_string())?;
    write_frame(&mut stream, &bytes).await?;
    let response =
        tokio::time::timeout(std::time::Duration::from_secs(40), read_frame(&mut stream))
            .await
            .map_err(|_| "The node service did not answer within 40 seconds.".to_string())??;
    serde_json::from_slice(&response)
        .map(Some)
        .map_err(|error| format!("Unreadable answer from the node service: {error}"))
}
