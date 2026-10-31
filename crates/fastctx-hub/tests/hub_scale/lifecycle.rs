//! Faults in the member session's durable handoff: a failed new epoch must
//! fence the old session, and concurrent replacements leave one valid owner.

use crate::fixture::Identity;
use fastctx_net::{
    dial::{self, HubSocket},
    ids::CertPin,
    wire::{self, Down, Letters, Up},
};
use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};

pub async fn check(root: &Path, identity: Arc<Identity>, address: SocketAddr, pin: CertPin) -> anyhow::Result<()> {
    let (mut old, first) = connect(&identity, address, pin, 900_000).await?;
    let Down::Welcome { letters, hub, .. } = first else { anyhow::bail!("epoch fixture did not connect") };
    let path = root.join("hub/counts").join(format!("{letters:016x}"));
    let backup = path.with_extension("epoch-test");
    std::fs::rename(&path, &backup)?;
    std::fs::create_dir(&path)?;
    let failed = connect(&identity, address, pin, 900_001).await;
    std::fs::remove_dir(&path)?;
    std::fs::rename(&backup, &path)?;
    let (_, refusal) = failed?;
    anyhow::ensure!(matches!(refusal, Down::Refused { .. }), "the count-path fault did not reject Open: {refusal:?}");
    let _ = dial::send(&mut old, &letter(&identity, hub.clone(), 100)).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (fresh, welcome) = connect(&identity, address, pin, 900_001).await?;
    anyhow::ensure!(
        matches!(welcome, Down::Welcome { taken: 0, .. }),
        "failed epoch change admitted the old sender's count: {welcome:?}"
    );
    drop((old, fresh));

    let mut attempts = tokio::task::JoinSet::new();
    for _ in 0..32 {
        let identity = identity.clone();
        attempts.spawn(async move { connect(&identity, address, pin, 900_002).await });
    }
    let mut opened = Vec::new();
    while let Some(attempt) = attempts.join_next().await {
        match attempt? {
            Ok((socket, Down::Welcome { .. })) => opened.push(socket),
            Ok((_, Down::Refused { reason: wire::Refusal::Replaced })) => {}
            Err(error) if error.to_string() == "the connection was cut" => {}
            Ok((_, reply)) => anyhow::bail!("unexpected replacement response: {reply:?}"),
            Err(error) => return Err(error.context("overlapping Open")),
        }
    }
    anyhow::ensure!(!opened.is_empty(), "no overlapping Open received a welcome");
    tokio::time::sleep(Duration::from_millis(250)).await;
    let mut responses = tokio::task::JoinSet::new();
    for mut socket in opened {
        let frame = letter(&identity, hub.clone(), 1);
        responses.spawn(async move {
            if dial::send(&mut socket, &frame).await.is_err() {
                return Ok::<_, anyhow::Error>(false);
            }
            let received = async {
                loop {
                    match dial::recv(&mut socket).await {
                        Ok(Some(Down::Ack { seq: 1, refused: Some(_) })) => return Ok(true),
                        Ok(Some(Down::Refused { .. }) | None) | Err(_) => return Ok(false),
                        Ok(Some(_)) => {}
                    }
                }
            };
            tokio::time::timeout(Duration::from_secs(30), received).await?
        });
    }
    let mut owners = 0;
    while let Some(response) = responses.join_next().await {
        owners += usize::from(response??);
    }
    anyhow::ensure!(owners == 1, "concurrent Open left {owners} valid letter counters instead of one");
    let (mut counting, opened) = connect(&identity, address, pin, 900_003).await?;
    anyhow::ensure!(matches!(opened, Down::Welcome { taken: 0, .. }), "count fixture did not start at zero");
    std::fs::rename(&path, &backup)?;
    std::fs::create_dir(&path)?;
    dial::send(&mut counting, &letter(&identity, hub.clone(), 1)).await.map_err(anyhow::Error::msg)?;
    let failed = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match dial::recv(&mut counting).await {
                Ok(Some(Down::Refused { .. })) => return Ok(()),
                Ok(Some(Down::Ack { .. })) => anyhow::bail!("a failed counter write was acknowledged"),
                Ok(None) | Err(_) => return Ok(()),
                Ok(Some(_)) => {}
            }
        }
    })
    .await;
    std::fs::remove_dir(&path)?;
    std::fs::rename(&backup, &path)?;
    failed??;
    let (mut recovered, welcome) = connect(&identity, address, pin, 900_003).await?;
    let Down::Welcome { taken, .. } = welcome else { anyhow::bail!("counter recovery did not reconnect") };
    let count: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    anyhow::ensure!(
        taken == 0 && count["taken"].as_u64() == Some(1) && count["refused"].get("1").is_some(),
        "welcome counted a failed write without making it durable: {welcome:?} / {count}"
    );
    // Welcome deliberately stops before an unheard refusal so the sender
    // retries that letter and learns its outcome before discarding it.
    dial::send(&mut recovered, &letter(&identity, hub, 1)).await.map_err(anyhow::Error::msg)?;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match dial::recv(&mut recovered).await.map_err(anyhow::Error::msg)? {
                Some(Down::Ack { seq: 1, refused: Some(_) }) => return Ok::<_, anyhow::Error>(()),
                reply @ (Some(Down::Ack { .. } | Down::Refused { .. }) | None) => {
                    anyhow::bail!("recovered refusal was not replayed: {reply:?}")
                }
                Some(_) => {}
            }
        }
    })
    .await??;
    println!(
        "PASS member lifecycle: failed epoch fenced; 32 overlapping sessions left one counter owner; failed count made durable before welcome and refusal replayed"
    );
    Ok(())
}

async fn connect(
    identity: &Identity,
    address: SocketAddr,
    pin: CertPin,
    epoch: u64,
) -> anyhow::Result<(HubSocket, Down)> {
    let dialed = dial::dial(&[address], Some(pin), wire::CONTROL_PATH, wire::MAX_CONTROL_FRAME)
        .await
        .map_err(anyhow::Error::msg)?;
    let hello = crate::client::greeting(identity, &dialed.binding, Letters { epoch, taken: None }, address);
    let mut socket = dialed.socket;
    dial::send(&mut socket, &hello).await.map_err(anyhow::Error::msg)?;
    let reply = tokio::time::timeout(Duration::from_secs(30), dial::recv(&mut socket))
        .await?
        .map_err(anyhow::Error::msg)?
        .ok_or_else(|| anyhow::anyhow!("session closed before welcome/refusal"))?;
    Ok((socket, reply))
}

fn letter(identity: &Identity, hub: fastctx_net::ids::HubId, seq: u64) -> Up {
    Up::Send {
        seq,
        message: fastctx_net::envelope::Envelope {
            hub,
            from: identity.name.clone(),
            to: "missing".parse().expect("fixture name"),
            n: seq,
            kind: fastctx_net::envelope::Kind::Letter,
            account: Some(identity.account.clone()),
            purpose: Some(fastctx_types::access::Purpose::Echo),
            re: None,
            stream: None,
            body: serde_json::json!({}),
        },
    }
}
