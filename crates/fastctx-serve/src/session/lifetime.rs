use super::*;
use fastctx_platform::ipc::{Connection, Endpoint, Listener};
use fastctx_types::ipc::{ClientFrame, Hello, NodeFrame, Welcome};
use serde_json::json;
use tokio::sync::oneshot;

async fn request(connection: &mut Connection) -> (u64, Call) {
    let bytes = connection.recv().await.unwrap().expect("the session must retain its arrival");
    let ClientFrame::Call { id, call } = serde_json::from_slice(&bytes).unwrap() else { panic!("expected call") };
    (id, call)
}

async fn reply(connection: &mut Connection, id: u64, reply: Reply) {
    connection.send(&serde_json::to_vec(&NodeFrame::Reply { id, reply: Ok(reply) }).unwrap()).await.unwrap();
}

#[tokio::test]
async fn cancelling_a_call_during_its_probe_keeps_the_original_arrival() {
    let temporary = tempfile::tempdir().unwrap();
    let world = WorldDir::at(temporary.path());
    let mut listener = Listener::bind(&Endpoint::for_world(world.path()).unwrap()).unwrap();
    let (probing, observed) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let (probe_finished, probe_result) = oneshot::channel();
    let (answered, answer) = oneshot::channel();
    let node = tokio::spawn(async move {
        let mut connection = listener.accept().await.unwrap();
        let hello = connection.recv().await.unwrap().unwrap();
        let _: Hello = serde_json::from_slice(&hello).unwrap();
        connection
            .send(
                &serde_json::to_vec(&Welcome::Accepted {
                    goal_runtime: false,
                    sentinels: false,
                    goal_commands: false,
                    profiles: false,
                    account_records: false,
                    conversation_plans: false, scratch_locations: false,
                    machine: "fixture".parse().unwrap(),
                    version: env!("CARGO_PKG_VERSION").into(),
                    node_lifetime: None,
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let (id, call) = request(&mut connection).await;
        assert!(matches!(call, Call::Arrive { .. }));
        reply(&mut connection, id, Reply::Arrived(Whereabouts { resident: "codex-cafe".parse().unwrap(), town: None }))
            .await;
        let (id, call) = request(&mut connection).await;
        assert!(matches!(call, Call::Machines { .. }));
        probing.send(()).unwrap();
        released.await.unwrap();
        let machines = NodeFrame::Reply {
            id,
            reply: Ok(Reply::Machines {
                scratch: Default::default(),
                groups: vec![],
                here: Some("fixture".to_string()),
                others: vec![],
                collab: None,
                speaks: false,
            }),
        };
        let sent = connection.send(&serde_json::to_vec(&machines).unwrap()).await.map_err(|error| error.to_string());
        let retained = sent.is_ok();
        probe_finished.send(sent).unwrap();
        if !retained {
            return;
        }
        let next = request(&mut connection).await;
        assert!(matches!(next.1, Call::Machines { .. }), "a cancelled tool must not register a new arrival");
        reply(
            &mut connection,
            next.0,
            Reply::Machines {
                scratch: Default::default(),
                here: Some("fixture".to_string()),
                others: vec![],
                groups: vec![],
                collab: None,
                speaks: false,
            },
        )
        .await;
        answered.send(()).unwrap();
        assert!(connection.recv().await.unwrap().is_none());
    });
    let session = Arc::new(Session::new(world, false).unwrap());
    session.introduce(&json!({"name":"codex-private-fixture","version":"1"}));
    assert_eq!(session.whereabouts(None).await.unwrap().resident.as_str(), "codex-cafe");
    let checking = session.clone();
    let call = tokio::spawn(async move { checking.whereabouts(None).await });
    observed.await.unwrap();
    call.abort();
    assert!(call.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    probe_result.await.unwrap().expect("cancelling the tool must not close its session connection");
    assert_eq!(session.whereabouts(None).await.unwrap().resident.as_str(), "codex-cafe");
    answer.await.unwrap();
    drop(session);
    node.await.unwrap();
}
