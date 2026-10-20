//! A long-lived stdio MCP endpoint across node and network restarts. IDs
//! belong to one connection generation; tool calls are never replayed.

use fastctx_client::{
    Client,
    mcp_relay::{McpReader, McpWriter},
};
use fastctx_types::{layout::WorldDir, mcp_message};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, Read, Write};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};

#[derive(Default)]
struct State {
    epoch: u64,
    serial: u64,
    sending: Option<mpsc::Sender<Vec<u8>>>,
    requests: BTreeMap<String, Value>,
    reverse: BTreeMap<String, Value>,
    initialize: Option<Value>,
    initialized: Option<Value>,
}
fn error(id: Value, why: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":why}})
}
fn messages(value: Value) -> Vec<Value> {
    match value {
        Value::Array(values) => values,
        value => vec![value],
    }
}
async fn output(send: &mpsc::Sender<Vec<u8>>, value: &Value) -> Result<(), String> {
    send.send(serde_json::to_vec(value).map_err(|e| e.to_string())?).await.map_err(|_| "MCP stdout closed".into())
}

pub(crate) fn run(name: String) -> anyhow::Result<()> {
    fastctx_types::profile::name_of(&name).map_err(anyhow::Error::msg)?;
    let world = WorldDir::under(&fastctx_platform::fastctx_root()?);
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?.block_on(async move {
        let (input, mut host) = mpsc::channel(1);
        let (out, mut printed) = mpsc::channel::<Vec<u8>>(1);
        let (done, mut closed) = watch::channel(false);
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin().lock();
            loop {
                let mut line = Vec::new();
                match (&mut stdin).take((mcp_message::MAX_LINE + 2) as u64).read_until(b'\n', &mut line) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) => {
                        let _ = input.blocking_send(Err(e.to_string()));
                        break;
                    }
                }
                if line.last() == Some(&b'\n') {
                    line.pop();
                }
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                let value = mcp_message::parse(&line);
                let invalid = value.is_err();
                if input.blocking_send(value).is_err() || invalid {
                    break;
                }
            }
        });
        let writer_done = done.clone();
        std::thread::spawn(move || {
            let mut stdout = std::io::stdout().lock();
            while let Some(line) = printed.blocking_recv() {
                if stdout.write_all(&line).and_then(|()| stdout.write_all(b"\n")).and_then(|()| stdout.flush()).is_err()
                {
                    break;
                }
            }
            writer_done.send_replace(true);
        });
        let shared = Arc::new(Mutex::new(State::default()));
        let (changed, connected) = watch::channel(0u64);
        let network = connections(world, name, shared.clone(), out.clone(), changed, closed.clone());
        let host_side = async {
            let mut connected = connected;
            while let Some(value) = host.recv().await {
                for mut message in messages(value?) {
                    let initializing = message["method"] == "initialize";
                    if initializing {
                        let waiting = async {
                            loop {
                                if shared.lock().expect("never poisoned").sending.is_some() {
                                    break;
                                }
                                connected.changed().await.map_err(|_| "MCP reconnect stopped".to_string())?;
                            }
                            Ok::<(), String>(())
                        };
                        let _ = tokio::time::timeout(std::time::Duration::from_secs(25), waiting).await;
                    }
                    let (sender, answer) = {
                        let mut state = shared.lock().expect("never poisoned");
                        if initializing {
                            state.initialize = Some(message.clone());
                        }
                        if message["method"] == "notifications/initialized" {
                            state.initialized = Some(message.clone());
                        }
                        let sender = state.sending.clone();
                        let mut answer = None;
                        if message.get("method").is_some() {
                            if let Some(id) = message.get("id").cloned() {
                                if sender.is_some() {
                                    state.serial += 1;
                                    let wire = format!("fastctx-{}-{}", state.epoch, state.serial);
                                    state.requests.insert(wire.clone(), id);
                                    message["id"] = wire.into();
                                } else {
                                    answer = Some(error(id, "the MCP is reconnecting; this request was not sent"));
                                }
                            }
                        } else if let Some(id) = message.get("id").and_then(Value::as_str) {
                            match state.reverse.remove(id) {
                                Some(id) => message["id"] = id,
                                None => continue,
                            }
                        }
                        (sender, answer)
                    };
                    if let Some(answer) = answer {
                        output(&out, &answer).await?;
                    } else if let Some(sender) = sender {
                        let _ = sender.send(serde_json::to_vec(&message).unwrap()).await;
                    }
                }
            }
            done.send_replace(true);
            Ok::<(), String>(())
        };
        tokio::select! {
            result=host_side=>result,
            result=network=>result,
            _=closed.wait_for(|v|*v)=>Ok(()),
        }
        .map_err(anyhow::Error::msg)
    })
}

async fn connections(
    world: WorldDir,
    name: String,
    shared: Arc<Mutex<State>>,
    out: mpsc::Sender<Vec<u8>>,
    changed: watch::Sender<u64>,
    mut closed: watch::Receiver<bool>,
) -> Result<(), String> {
    let mut delay = 1u64;
    loop {
        let opening = async {
            let client = Client::connect(&world, fastctx_client::command_peer()).await.map_err(|e| e.to_string())?;
            client.mcp_relay(name.clone()).await.map_err(|e| e.to_string())
        };
        let opened = tokio::select! {
            opened=tokio::time::timeout(std::time::Duration::from_secs(30),opening)=>opened.map_err(|_|"opening MCP relay took more than 30 s".to_string()).and_then(|v|v),
            _=closed.wait_for(|v|*v)=>return Ok(()),
        };
        let result = match opened {
            Ok((mut reader, mut writer)) => {
                let (initialize, initialized, epoch) = {
                    let mut state = shared.lock().expect("never poisoned");
                    state.epoch += 1;
                    (state.initialize.clone(), state.initialized.clone(), state.epoch)
                };
                let replay = async {
                    if let (Some(mut init), Some(initialized)) = (initialize, initialized) {
                        let id = format!("fastctx-handshake-{epoch}");
                        init["id"] = id.clone().into();
                        writer.send(&serde_json::to_vec(&init).unwrap()).await?;
                        loop {
                            let message = mcp_message::parse(&reader.receive().await?)?;
                            if message["id"] == id && message.get("method").is_none() {
                                if message.get("error").is_some() {
                                    return Err(format!("MCP reinitialization failed: {}", message["error"]));
                                }
                                break;
                            }
                            if message.get("method").is_some()
                                && let Some(id) = message.get("id")
                            {
                                writer
                                    .send(
                                        &serde_json::to_vec(&error(
                                            id.clone(),
                                            "the MCP client is restoring its handshake",
                                        ))
                                        .unwrap(),
                                    )
                                    .await?;
                            }
                        }
                        writer.send(&serde_json::to_vec(&initialized).unwrap()).await?;
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                };
                match tokio::time::timeout(std::time::Duration::from_secs(25), replay).await {
                    Ok(Ok(replayed)) => {
                        delay = 1;
                        let (send, receive) = mpsc::channel(1);
                        shared.lock().expect("never poisoned").sending = Some(send);
                        changed.send_modify(|v| *v += 1);
                        if replayed {
                            output(&out, &json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"})).await?;
                        }
                        connected(reader, writer, receive, &shared, &out, &mut closed).await
                    }
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err("MCP reinitialization did not answer within 25 s".into()),
                }
            }
            Err(e) => Err(e),
        };
        let pending = {
            let mut state = shared.lock().expect("never poisoned");
            state.sending = None;
            state.reverse.clear();
            std::mem::take(&mut state.requests)
        };
        for id in pending.into_values() {
            output(&out, &error(id, "the MCP connection ended before this request answered; its outcome is unknown"))
                .await?;
        }
        if let Err(why) = result {
            eprintln!("fastctx mcp-relay: {why}");
        }
        tokio::select! { _=tokio::time::sleep(std::time::Duration::from_secs(delay))=>{}, _=closed.wait_for(|v|*v)=>return Ok(()) }
        delay = (delay * 2).min(30);
    }
}

async fn connected(
    mut reader: McpReader,
    mut writer: McpWriter,
    mut receive: mpsc::Receiver<Vec<u8>>,
    shared: &Mutex<State>,
    out: &mpsc::Sender<Vec<u8>>,
    closed: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let sending = async {
        while let Some(line) = receive.recv().await {
            writer.send(&line).await?;
        }
        Ok::<(), String>(())
    };
    let reading = async {
        loop {
            for mut message in messages(mcp_message::parse(&reader.receive().await?)?) {
                {
                    let mut state = shared.lock().expect("never poisoned");
                    if message.get("method").is_some() {
                        if let Some(id) = message.get("id").cloned() {
                            state.serial += 1;
                            let wire = format!("fastctx-server-{}-{}", state.epoch, state.serial);
                            state.reverse.insert(wire.clone(), id);
                            message["id"] = wire.into();
                        }
                    } else if let Some(wire) = message.get("id").and_then(Value::as_str) {
                        match state.requests.remove(wire) {
                            Some(id) => message["id"] = id,
                            None => continue,
                        }
                    } else {
                        continue;
                    }
                }
                output(out, &message).await?;
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), String>(())
    };
    tokio::select! { result=sending=>result, result=reading=>result, _=closed.wait_for(|v|*v)=>Ok(()) }
}
