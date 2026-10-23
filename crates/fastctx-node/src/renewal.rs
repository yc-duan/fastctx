//! A replacement keeps the service startup gate while serving outside the old guardian, so
//! restarting that guardian can only end waiting, unused node processes.

use std::io;
use std::path::Path;
use std::sync::Arc;

use fastctx_install::renewal as store;
use fastctx_platform::FileLock;
use fastctx_types::layout::WorldDir;
use fastctx_types::renewal::{Action, Build, MAINTENANCE, Request, Response, Running};

use crate::Node;

pub(crate) struct Startup {
    pub build: Build,
    pub bridge: bool,
    pub successor: bool,
    service_gate: Option<FileLock>,
    manager_gate: Option<FileLock>,
}

impl Startup {
    pub async fn prepare(root: &Path, successor: Option<&str>, coordinated: bool) -> anyhow::Result<Self> {
        let root = root.to_owned();
        let successor = successor.map(str::to_owned);
        tokio::task::spawn_blocking(move || {
            let build = store::build()?;
            let directory = store::directory(&root);
            fastctx_platform::ensure_private_dir(&directory)?;
            if let Some(digest) = &successor {
                anyhow::ensure!(*digest == build.digest, "replacement is not the requested executable");
                if !coordinated {
                    store::write_json(
                        &directory.join(format!("prepared-{digest}.json")),
                        &fastctx_platform::current_identity()?,
                    )?;
                }
            }
            let kept = WorldDir::under(&root).node_service().exists();
            let service_gate = if kept && !coordinated {
                Some(FileLock::acquire(&directory.join("service-gate.lock"))?)
            } else {
                None
            };
            let bridge = kept && successor.is_some();
            let manager_gate =
                if kept && !bridge { Some(FileLock::acquire(&directory.join("manager-gate.lock"))?) } else { None };
            Ok(Self { build, bridge, successor: successor.is_some(), service_gate, manager_gate })
        })
        .await?
    }

    pub fn acquired(&mut self) {
        self.manager_gate.take();
        if !self.bridge {
            self.service_gate.take();
        }
    }
}

pub(crate) fn answer(node: &Arc<Node>, request: Request) -> io::Result<Response> {
    if request.maintenance != MAINTENANCE {
        return Ok(Response::Refused { reason: "unsupported local maintenance envelope".into() });
    }
    Ok(match request.action {
        Action::Status => Response::Status {
            node: Running {
                build: node.renewal.build.clone(),
                executable: node.fastctx.display().to_string(),
                process: fastctx_platform::current_identity()?,
                yielding: node.handoff.yielding(),
                bridge: node.renewal.bridge,
                supervised: node.supervised,
            },
        },
        Action::Yield => {
            crate::handoff::hand_over(node.clone());
            Response::Done
        }
        Action::Cancel => {
            if node.handoff.cancel() {
                crate::speech::resume(node, &tokio::runtime::Handle::current());
                Response::Done
            } else {
                Response::Refused { reason: "handover already committed".into() }
            }
        }
    })
}

/// Resumes an interrupted handover and retries failed attempts after their delay.
pub(crate) fn resume(node: &Arc<Node>) {
    let weak = Arc::downgrade(node);
    tokio::spawn(async move {
        let mut first = true;
        loop {
            let Some(node) = weak.upgrade() else { return };
            let root = node.root.clone();
            let build = node.renewal.build.clone();
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                if let Some(plan) = store::read(&root)? {
                    let failed = plan.phase == fastctx_types::renewal::Phase::Failed;
                    if !failed
                        && (matches!(
                            plan.phase,
                            fastctx_types::renewal::Phase::Staged
                                    | fastctx_types::renewal::Phase::Preparing
                                    | fastctx_types::renewal::Phase::WaitingForIdle
                                    | fastctx_types::renewal::Phase::UpdatingGuardian
                                    | fastctx_types::renewal::Phase::ReturningToService
                                    // Left by an earlier release; this one
                                    // waits for nobody and goes on.
                                    | fastctx_types::renewal::Phase::AwaitingAdministrator
                        ) || (first
                            && (plan.phase != fastctx_types::renewal::Phase::Complete
                                || plan.candidate.build != build)))
                    {
                        store::request(&root, false)?;
                    }
                }
                Ok(())
            })
            .await;
            if let Err(error) = result.unwrap_or_else(|error| Err(error.into())) {
                node.log.line(format_args!("could not resume executable renewal: {error:#}"));
            }
            first = false;
            drop(node);
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });
}
