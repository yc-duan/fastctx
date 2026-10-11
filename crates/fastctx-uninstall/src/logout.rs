//! A short, authenticated withdrawal connection. Unlike Link::start this
//! neither reserves message counters nor creates an identity or a node.

use anyhow::{Context as _, bail};
use fastctx_net::link::state::{LettersFile, Membership};
use fastctx_net::{dial, sign, wire};
use fastctx_types::{AccountName, layout::WorldDir};
use std::path::Path;

async fn frame(socket: &mut dial::HubSocket) -> anyhow::Result<Option<wire::Down>> {
    tokio::time::timeout(wire::SILENCE, dial::recv(socket))
        .await
        .context("the hub did not answer within its protocol silence interval")?
        .map_err(anyhow::Error::msg)
}

async fn open(world: &WorldDir, directory: &Path) -> anyhow::Result<Option<dial::HubSocket>> {
    let membership =
        crate::read::<Membership>(&directory.join("membership.json"))?.context("hub membership is absent")?;
    let letters = crate::read::<LettersFile>(&directory.join("letters.json"))?
        .context("the saved letter numbering is absent; it cannot be invented during uninstall")?;
    let key = sign::SecretKey::load(&world.device_key())?;
    let view = crate::read::<fastctx_net::directory::View>(&directory.join("view.json"))?;
    let mut addresses = view
        .as_ref()
        .and_then(|view| view.me())
        .map(|me| me.relays.iter().flat_map(|relay| relay.addresses.iter().copied()).collect::<Vec<_>>())
        .unwrap_or_default();
    addresses.extend(membership.addresses.iter().copied());
    let mut connection = dial::dial(&addresses, Some(membership.cert), wire::CONTROL_PATH, wire::MAX_CONTROL_FRAME)
        .await
        .map_err(anyhow::Error::msg)?;
    let intent = wire::Intent::Session {
        key: key.public(),
        proof: key.sign(sign::Domain::Session, &connection.binding),
        letters: wire::Letters { epoch: letters.epoch, taken: letters.taken },
        through: None,
        dialed: Some(connection.address),
    };
    dial::send(
        &mut connection.socket,
        &wire::Up::Hello { protocol: wire::PROTOCOL, version: env!("CARGO_PKG_VERSION").into(), intent },
    )
    .await
    .map_err(anyhow::Error::msg)?;
    match frame(&mut connection.socket).await? {
        Some(wire::Down::Welcome { hub, name, .. }) if hub == membership.id && name == membership.name => {
            Ok(Some(connection.socket))
        }
        Some(wire::Down::Refused { reason: wire::Refusal::NotMember | wire::Refusal::Removed { .. } }) => Ok(None),
        Some(wire::Down::Refused { reason }) => Err(reason.into()),
        _ => bail!("the hub did not authenticate this membership"),
    }
}

async fn ask(socket: &mut dial::HubSocket, id: u64, question: wire::Question) -> anyhow::Result<Option<wire::Told>> {
    dial::send(socket, &wire::Up::Ask { id, question }).await.map_err(anyhow::Error::msg)?;
    let deadline = tokio::time::Instant::now() + wire::SILENCE;
    loop {
        match tokio::time::timeout_at(deadline, frame(socket))
            .await
            .context("the hub did not answer the withdrawal question")??
        {
            Some(wire::Down::Told { id: received, answer }) if received == id => {
                return answer.map(Some).map_err(anyhow::Error::msg);
            }
            Some(wire::Down::Refused { reason: wire::Refusal::Removed { .. } | wire::Refusal::NotMember }) => {
                return Ok(None);
            }
            Some(wire::Down::Refused { reason }) => return Err(reason.into()),
            Some(
                wire::Down::DirectoryChanged
                | wire::Down::Deliver { .. }
                | wire::Down::Pong { .. }
                | wire::Down::LedgerChanged { .. },
            ) => {}
            None => bail!("the hub connection ended without confirming withdrawal"),
            Some(_) => bail!("the hub sent an unexpected withdrawal reply"),
        }
    }
}

pub(crate) async fn logout(world: &WorldDir, directory: &Path, account: Option<&AccountName>) -> anyhow::Result<()> {
    let Some(mut socket) = open(world, directory).await? else { return Ok(()) };
    let Some(wire::Told::Directory { view }) = ask(&mut socket, 1, wire::Question::Directory).await? else {
        bail!("the hub did not provide this machine's logins");
    };
    let logins = view.me().context("the hub omitted this member")?.logins.clone();
    let accounts: Vec<_> = match account {
        Some(account) => logins.into_iter().filter(|login| login == account).collect(),
        None => logins.into_iter().collect(),
    };
    for (index, account) in accounts.iter().enumerate() {
        let result = ask(
            &mut socket,
            index as u64 + 2,
            wire::Question::Manage { account: account.clone(), change: wire::Change::Logout },
        )
        .await;
        match result {
            Ok(Some(wire::Told::Done)) | Ok(None) => {}
            Ok(Some(_)) => bail!("the hub did not confirm logout of {account}"),
            Err(error) => {
                // A last logout removes the member and may close the session
                // before its Done reaches us. A fresh authenticated query is
                // evidence of the result, not a second mutation or blind retry.
                match open(world, directory).await {
                    Ok(None) => return Ok(()),
                    Ok(Some(mut confirmation)) => {
                        if let Some(wire::Told::Directory { view }) =
                            ask(&mut confirmation, 1, wire::Question::Directory).await?
                            && view.me().is_none_or(|me| !me.logins.contains(account))
                        {
                            socket = confirmation;
                            continue;
                        }
                        return Err(error).context(format!("logout of {account} was not confirmed"));
                    }
                    Err(confirm) => {
                        return Err(error).context(format!("logout of {account} was not confirmed: {confirm:#}"));
                    }
                }
            }
        }
    }
    Ok(())
}
