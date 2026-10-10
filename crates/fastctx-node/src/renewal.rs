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
