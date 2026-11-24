use super::*;
use fastctx_platform::ipc::{Endpoint, Listener};
use fastctx_types::ipc::{ClientFrame, Hello, Welcome};
use serde_json::json;
use tokio::sync::oneshot;

/// A node that takes the call for the machines and never answers it: the
/// tools are listed without them once the bound passes, and the host is to
/// be told to list them again, rather than the listing waiting on the node.
#[tokio::test]
async fn a_node_slow_to_say_the_machines_lists_the_tools_without_them_in_time() {
    let temporary = tempfile::tempdir().unwrap();
    let world = WorldDir::at(temporary.path());
    let mut listener = Listener::bind(&Endpoint::for_world(world.path()).unwrap()).unwrap();
    let (asked, asking) = oneshot::channel();
    let (done, finished) = oneshot::channel::<()>();
    let node = tokio::spawn(async move {
        let mut connection = listener.accept().await.unwrap();
        let hello = connection.recv().await.unwrap().unwrap();
        let _: Hello = serde_json::from_slice(&hello).unwrap();
        connection
            .send(
                &serde_json::to_vec(&Welcome::Accepted {
                    goal_runtime: false,
                    watch_many: false,
                    composer_documents: false,
                    sentinels: false,
                    goal_commands: false,
                    profiles: false,
                    account_records: false,
                    conversation_plans: false,
                    timeline_reads: false,
                    watch_live: false,
                    topics: false,
                    remote_topics: false,
                    scratch_locations: false,
                    collaboration_watch: false,
                    timer_pages: true,
                    file_changes: false,
                    machine: "fixture".parse().unwrap(),
                    version: env!("CARGO_PKG_VERSION").into(),
                    node_lifetime: None,
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let bytes = connection.recv().await.unwrap().expect("the listing asks for the machines");
        let ClientFrame::Call { call, .. } = serde_json::from_slice(&bytes).unwrap() else { panic!("expected a call") };
        assert!(matches!(call, Call::Machines { .. }));
        asked.send(()).unwrap();
        // The connection stays open, unanswered, while the listing waits.
        let _ = finished.await;
    });
    let session = Arc::new(Session::new(world, false).unwrap());
    session.introduce(&json!({"name":"codex-private-fixture","version":"1"}));
    let (output, _printing) = crate::stdio::Output::start();
    let within = std::time::Duration::from_millis(300);
    let began = std::time::Instant::now();
    let machines = session.machines_to_list(None, &output, within).await;
    let took = began.elapsed();
    tokio::time::timeout(std::time::Duration::from_secs(10), asking)
        .await
        .expect("the node must have been asked for the machines by now")
        .expect("the node must have been asked for the machines");
    assert!(machines.is_none(), "the tools are listed without the machines");
    assert!(took >= within, "the listing waited for the node until the bound, {took:?}");
    assert!(took < std::time::Duration::from_secs(10), "the listing did not wait on the node past the bound, {took:?}");
    assert!(
        session.relisting.load(std::sync::atomic::Ordering::Acquire),
        "the host is to be told to list the tools again"
    );
    done.send(()).unwrap();
    node.await.unwrap();
}
