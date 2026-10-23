//! Requested resumption of a native Codex history, retaining its original thread.
use crate::Node;
use fastctx_hosts::codex::{ThreadSpec, native_tools};
use fastctx_types::{
    Address, ResidentId,
    ipc::Failure,
    residents::{About, PendingState},
};
use std::sync::Arc;

pub(super) fn spec(node: &Node, about: &About) -> ThreadSpec {
    let owner = Address { machine: node.machine(), resident: about.id.clone() };
    let env = vec![
        ("FASTCTX_HOME".into(), node.root.to_string_lossy().into_owned()),
        ("FASTCTX_RESIDENT".into(), owner.tag()),
        (
            "FASTCTX_AGENT_HOME".into(),
            about.home.as_ref().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default(),
        ),
    ];
    ThreadSpec {
        native_resume: true,
        user_mcp: Default::default(),
        owner,
        // Native resume omits cwd; the response supplies the thread's actual directory.
        cwd: node.world.conversation(&about.id).path().to_path_buf(),
        developer_instructions: String::new(),
        config: native_tools(
            &node.fastctx,
            &env,
            &["run", "ask_colleague", "message_colleague"],
            fastctx_hands::run::window::BUDGET,
        ),
        name: None,
        choices: node.world.conversation(&about.id).choices(),
    }
}
/// An open native frontend keeps taking its inbox at tool boundaries. Only a
/// closed frontend is adopted here; a loaded World runtime is already usable.
pub(crate) fn requested(node: &Arc<Node>, about: &About) -> Result<bool, Failure> {
    let thread = about.session.as_ref().ok_or_else(|| Failure::not_run("the native Codex thread is not identified"))?;
    let host = node.codex.of(node, about)?;
    if !super::native_lifetime::closed(&node.world, &about.id, host.process().as_ref()).map_err(Failure::not_run)? {
        observe(node, &about.id);
        return Ok(false);
    }
    if host.loaded(thread) {
        return Ok(true);
    }
    if !host.holds(thread) {
        host.adopt(thread, spec(node, about));
    }
    node.residents.set_native_resumed(&node.world, &about.id).map_err(|e| Failure::new(e.to_string()))?;
    Ok(true)
}
/// A native frontend may close after its requested observation entered the inbox.
pub(crate) fn left(node: &Arc<Node>, id: &ResidentId) {
    let (node, id) = (node.clone(), id.clone());
    tokio::spawn(async move {
        let Some((about, _)) = node.residents.about(&id) else {
            return;
        };
        if about.host.agent.as_str() != "codex" || about.world_started {
            return;
        }
        let directory = node.world.conversation(&id);
        let result = async {
            let pending = crate::inbox::pending(&directory)
                .map_err(|e| Failure::new(e.to_string()))?
                .into_iter()
                .find(|p| p.state == PendingState::Waiting && p.delivery.notice.wakes_closed());
            if let Some(pending) = pending {
                crate::speech::hand_over(&node, &id, &about, &directory, pending).await?;
            }
            Ok::<(), Failure>(())
        }
        .await;
        if let Err(error) = result {
            node.log.line(format_args!("resuming requested native observation for {id}: {error}"));
        }
    });
}

fn observe(node: &Arc<Node>, id: &ResidentId) {
    if !node.sentinels.native_watches.lock().expect("never poisoned").insert(id.clone()) {
        return;
    }
    let (node, id) = (node.clone(), id.clone());
    tokio::spawn(async move {
        let mut again = crate::again::Again::default();
        loop {
            if node.residents.active(&id).is_err() {
                break;
            }
            let wait = async {
                let controlled = node
                    .residents
                    .about(&id)
                    .map(|(about, _)| node.codex.of(&node, &about))
                    .transpose()
                    .map_err(|e| e.message)?
                    .and_then(|host| host.process());
                for process in super::native_lifetime::processes(&node.world, &id)? {
                    if controlled.as_ref() != Some(&process) {
                        fastctx_platform::host_process::exited(&process).await.map_err(|e| e.to_string())?;
                    }
                }
                Ok::<(), String>(())
            };
            tokio::pin!(wait);
            let result = loop {
                tokio::select! {
                    result=&mut wait=>break result,
                    ()=tokio::time::sleep(std::time::Duration::from_secs(60))=>{
                        if node.residents.active(&id).is_err() {break Ok(());}
                        match crate::inbox::pending(&node.world.conversation(&id)) {
                            Ok(pending) if !pending.iter().any(|p|p.state==PendingState::Waiting && p.delivery.notice.wakes_closed())=>break Ok(()),
                            Ok(_)=>{},
                            Err(error)=>break Err(error.to_string()),
                        }
                    }
                }
            };
            match result {
                Ok(()) => break,
                Err(error) => crate::sentinels::report(&node, &format!("native lifetime {id}"), Some(error)),
            }
            tokio::time::sleep(again.next()).await;
        }
        node.sentinels.native_watches.lock().expect("never poisoned").remove(&id);
        left(&node, &id);
    });
}
