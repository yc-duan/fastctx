//! Internal observations use the existing on transport and account admission.
use crate::Node;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use fastctx_agent::FutureResult;
use fastctx_sentinel::ports::{Look, ReadResult};
use fastctx_types::{
    MachineName,
    access::{Acting, HubAccount, Written},
    ipc::Failure,
    on::{OnOrigin, OnPackage, OnPiece, OnRequest, SentinelProbe},
    sentinel::{Definition, Kind},
};
use std::{path::PathBuf, sync::Arc};

pub(crate) struct MachineLook {
    pub node: Arc<Node>,
    pub scope: HubAccount,
    pub directory: PathBuf,
}
impl Look for MachineLook {
    fn look<'a>(
        &'a self,
        definition: &'a Definition,
        machine: &'a MachineName,
        command: &'a str,
    ) -> FutureResult<'a, ReadResult> {
        Box::pin(async move {
            if definition.kind == Kind::Web || command.trim().is_empty() || command.len() > 8192 {
                return Err("invalid sentinel observation".into());
            }
            let scope = &self.scope;
            let link = self.node.link_of(&scope.hub).ok_or("the observation account's hub is not connected")?;
            let acting = Acting::one(scope.hub.clone(), scope.account.clone());
            let written = Written { name: machine.clone(), hub: Some(link.hub_name()) };
            self.node.settled(&written).await;
            self.node.authorize_machine(&acting, &written).map_err(|e| e.message)?;
            let origin = OnOrigin {
                sentinel: Some(SentinelProbe { id: definition.id.clone(), generation: definition.generation }),
                turn: None,
                call_id: None,
                resident: definition.owner.clone(),
                town: None,
                request: None,
                follower: None,
                account: Some(scope.clone()),
            };
            let name = crate::speech::new_id("observation");
            let state = self.directory.join(format!("{name}.state"));
            let script = self.directory.join(format!("{name}.bash"));
            std::fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
            fastctx_platform::publish(&state, b"", fastctx_platform::Permissions::Private)
                .map_err(|e| e.to_string())?;
            fastctx_platform::publish(&script, command.as_bytes(), fastctx_platform::Permissions::Private)
                .map_err(|e| e.to_string())?;
            let request = OnRequest {
                machine: machine.clone(),
                hub: written.hub,
                receipt: true,
                process: None,
                job: None,
                pipe_to: None,
                package: OnPackage {
                    state: state.to_string_lossy().into_owned(),
                    script: script.to_string_lossy().into_owned(),
                    input: None,
                    source: None,
                    writes_off: true,
                },
            };
            let mut following = crate::on::send(&self.node, origin, request).await.map_err(|e| e.message)?;
            let _stop = Stop(following.stop.clone());
            tokio::time::timeout(std::time::Duration::from_secs(45), async {
                let mut bytes = Vec::new();
                while let Some(piece) = following.pieces.recv().await {
                    match piece {
                        crate::on::Piece::Value(Ok(OnPiece::Started { .. })) => {}
                        crate::on::Piece::Value(Ok(OnPiece::Out { base64 } | OnPiece::Err { base64 })) => {
                            let part = STANDARD.decode(base64).map_err(|e| e.to_string())?;
                            if bytes.len() + part.len() > 65_536 {
                                return Err("sentinel observation exceeded 65536 bytes".into());
                            }
                            bytes.extend(part);
                        }
                        crate::on::Piece::End { end, taken } => {
                            let code = end.code();
                            if !matches!(end, fastctx_types::on::OnEnd::Exited { remaining: 0, .. }) {
                                return Err(format!("sentinel observation did not finish: {end:?}"));
                            }
                            let path = self.directory.join(format!("{name}.txt"));
                            fastctx_platform::publish(&path, &bytes, fastctx_platform::Permissions::Private)
                                .map_err(|e| e.to_string())?;
                            let _ = taken.send(());
                            return Ok(ReadResult {
                                text: String::from_utf8_lossy(&bytes).into_owned(),
                                exit_code: Some(code),
                                full_path: Some(path.to_string_lossy().into_owned()),
                            });
                        }
                        crate::on::Piece::Value(Err(failure)) => return Err(failure.message),
                        _ => return Err("unexpected internal observation output".into()),
                    }
                }
                Err("the internal observation stream ended without a result".into())
            })
            .await
            .map_err(|_| "sentinel observation timed out after 45 seconds".to_string())?
        })
    }
}
struct Stop(tokio::sync::watch::Sender<bool>);
impl Drop for Stop {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

pub(crate) async fn admit(node: &Arc<Node>, origin: &OnOrigin) -> Result<Option<std::path::PathBuf>, Failure> {
    if let Some(why) = fastctx_hands::run::sentinel::unavailable() {
        return Err(Failure::not_run(why));
    }
    let probe = origin.sentinel.as_ref().ok_or_else(|| Failure::refused("missing observation identity"))?;
    let scope = origin.account.as_ref().ok_or_else(|| Failure::refused("missing observation account"))?;
    node.authorize_here(&Acting::one(scope.hub.clone(), scope.account.clone()))?;
    let records = node.sentinels.records()?;
    let (_, registration) = fastctx_sentinel::lifecycle::get(records, scope, &probe.id)
        .await
        .map_err(|e| Failure::not_run(e.to_string()))?
        .ok_or_else(|| Failure::not_run("sentinel is no longer registered"))?;
    let d = registration.definition;
    let link = node.link_of(&scope.hub).ok_or_else(|| Failure::not_run("observation hub is disconnected"))?;
    let view = crate::authorization::current_view(&link)?;
    let home_matches = origin.follower.as_ref().is_some_and(|from| {
        view.member(from).zip(view.member(&d.home)).is_some_and(|(from, home)| from.key == home.key)
    });
    if !registration.active
        || d.generation != probe.generation
        || d.owner != origin.resident
        || d.kind == Kind::Web
        || !home_matches
    {
        return Err(Failure::refused("the observation does not match the active sentinel assignment"));
    }
    if d.kind == Kind::Goal {
        return crate::placement::artifacts::read_directory(node, scope, &d.owner);
    }
    Ok(None)
}
