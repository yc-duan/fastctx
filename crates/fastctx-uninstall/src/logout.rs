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
    open_saying(world, directory, None).await
}

/// Tells the hub of the membership in `directory`, which took this machine
/// off, what it could not do of the uninstall it asked. The hub keeps it and
/// refuses the session.
pub async fn tell_left(world: &WorldDir, directory: &Path, left: &str) -> anyhow::Result<()> {
    match open_saying(world, directory, Some(left)).await? {
        None => Ok(()),
        Some(_) => bail!("the hub let this machine in again; it was not taken off"),
    }
}

async fn open_saying(
    world: &WorldDir,
    directory: &Path,
    left: Option<&str>,
) -> anyhow::Result<Option<dial::HubSocket>> {
    let membership =
        crate::read::<Membership>(&directory.join("membership.json"))?.context("hub membership is absent")?;
    let letters = crate::read::<LettersFile>(&directory.join("letters.json"))?
        .context("the saved letter numbering is absent; it cannot be invented during uninstall")?;
    let key = sign::SecretKey::load(&world.device_key())?;
    let view = crate::read::<fastctx_net::directory::View>(&directory.join("view.json"))?;
    // Its own line first, then through each machine that allows direct
    // connections, as the link reaches the hub.
    let upstreams: Vec<dial::Upstream> = std::iter::once(dial::Upstream::Hub(membership.addresses.clone()))
        .chain(
            view.iter()
                .flat_map(|view| view.forwarders.iter().filter(|forwarder| forwarder.name != membership.name))
                .cloned()
                .map(dial::Upstream::Through),
        )
        .collect();
    let mut failures = Vec::new();
    let mut reached = None;
    for upstream in &upstreams {
        match dial::dial_upstream(
            upstream,
            &membership.id,
            membership.cert,
            wire::CONTROL_PATH,
            wire::MAX_CONTROL_FRAME,
        )
        .await
        {
            Ok(connection) => {
                reached = Some((connection, upstream.through().map(ToString::to_string)));
                break;
            }
            Err(error) => failures.push(format!("{upstream}: {error}")),
        }
    }
    let (mut connection, through) = reached.ok_or_else(|| anyhow::anyhow!("{}", failures.join("; ")))?;
    let intent = wire::Intent::Session {
        key: key.public(),
        proof: key.sign(sign::Domain::Session, &connection.binding),
        letters: wire::Letters { epoch: letters.epoch, taken: letters.taken },
        through,
        direct: false,
        interfaces: Vec::new(),
        renames: false,
        afoot: false,
        forgets: false,
        roles: true,
        versions: false,
        due: false,
        project_deletes: false,
        project_entry: false,
        conversation_deletions: false,
        profiles: false,
        account_records: false,
        conversation_plans: false,
        scratch_locations: false,
        left: left.map(str::to_string),
        dialed: Some(connection.address),
        any_names: true,
        cancels: false,
        presence: false,
        parts: false,
        call_details: false,
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

/// The hub's answer to `question`. While its node is down, this session is
/// the machine's on the hub, which sends it what it sends the machine:
/// calls of other machines, letters, what changed. None of it is the
/// answer, and none of it is taken up; a call left unanswered is lost to
/// its caller when the session ends.
async fn ask(socket: &mut dial::HubSocket, id: u64, question: wire::Question) -> anyhow::Result<Option<wire::Told>> {
    dial::send(socket, &wire::Up::Ask { id, question }).await.map_err(anyhow::Error::msg)?;
    let deadline = tokio::time::Instant::now() + wire::SILENCE;
    loop {
        let heard = tokio::time::timeout_at(deadline, dial::hear(socket))
            .await
            .context("the hub did not answer the withdrawal question")?
            .map_err(anyhow::Error::msg)?;
        match heard {
            Some(dial::Heard::Frame(frame)) => match *frame {
                wire::Down::Told { id: received, answer } if received == id => {
                    return answer.map(Some).map_err(anyhow::Error::msg);
                }
                wire::Down::Refused { reason: wire::Refusal::Removed { .. } | wire::Refusal::NotMember } => {
                    return Ok(None);
                }
                wire::Down::Refused { reason } => return Err(reason.into()),
                _ => {}
            },
            Some(dial::Heard::Unread) => {}
            None => bail!("the hub connection ended without confirming withdrawal"),
        }
    }
}

pub(crate) async fn logout(world: &WorldDir, directory: &Path, account: Option<&AccountName>) -> anyhow::Result<()> {
    let Some(mut socket) = open(world, directory).await? else { return Ok(()) };
    let Some(wire::Told::Directory { view }) = ask(&mut socket, 1, wire::Question::Directory).await? else {
        bail!("the hub did not provide this machine's logins");
    };
    let me = view.me().context("the hub omitted this member")?;
    // A managed machine is logged in to nothing: it leaves the account it
    // serves (`world-network.md`, 受管机器).
    let managed = me.managed;
    let logins = if managed { me.home.iter().cloned().collect() } else { me.logins.clone() };
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
                            && view.me().is_none_or(|me| !managed && !me.logins.contains(account))
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
