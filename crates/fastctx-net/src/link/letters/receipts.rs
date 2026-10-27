//! Receive receipts use the node's WAL database. Old files remain readable until
//! a verified, committed migration batch removes them.

mod migrate;

use super::super::state::NetDir;
use super::Receipt;
use crate::message::LetterId;
use crate::sign::PublicKey;
use fastctx_storage::receipts::{self, Key, Reference};
use fastctx_types::MachineName;
use std::io;
use std::path::Path;
use std::time::Duration;

const KEEP: Duration = Duration::from_secs(7 * 86400);

fn sender_key(from: &PublicKey) -> String {
    let bytes = serde_json::to_vec(from).expect("public identities serialize");
    crate::sign::sha256(&bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn key(net: &NetDir, namespace: &str, sender: &str, id: &str) -> Key {
    Key::new(namespace, format!("{}:{sender}", net.authority()), id)
}

fn read(net: &NetDir, key: &Key, path: &Path) -> io::Result<Option<Receipt>> {
    let stored = || net.database().read_now(|connection| receipts::get(connection, key)).map_err(io::Error::other);
    let bytes = match stored()? {
        Some(bytes) => Some(bytes),
        None => match std::fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => stored()?,
            Err(error) => return Err(error),
        },
    };
    bytes
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)))
        .transpose()
}

pub async fn receipt(
    net: &NetDir,
    from: &PublicKey,
    names: &[MachineName],
    id: &LetterId,
) -> io::Result<Option<Receipt>> {
    let net = net.clone();
    let sender = sender_key(from);
    let names = names.to_vec();
    let id = id.clone();
    tokio::task::spawn_blocking(move || {
        let path = net.path().join("received-by-key").join(&sender).join(id.as_str());
        if let Some(receipt) = read(&net, &key(&net, "transport-key", &sender, id.as_str()), &path)? {
            return Ok(Some(receipt));
        }
        let mut found: Option<Receipt> = None;
        for name in names {
            let key = key(&net, "transport-name", &name.file_name(), id.as_str());
            for entry in [name.file_name().as_ref(), name.key()] {
                let path = net.path().join("received").join(entry).join(id.as_str());
                let Some(old) = read(&net, &key, &path)? else { continue };
                if found.is_some_and(|kept| kept.digest != old.digest) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "legacy letter receipts disagree across sender aliases",
                    ));
                }
                found = Some(old);
            }
        }
        Ok(found)
    })
    .await
    .map_err(io::Error::other)?
}

/// A ready receive window shares one commit. Every copy keeps its own reference,
/// including duplicates whose epoch or sequence differs from the first delivery.
pub(super) async fn keep_batch(net: &NetDir, batch: Vec<(PublicKey, LetterId, Receipt, Reference)>) -> io::Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let entries = batch
        .into_iter()
        .map(|(from, id, receipt, reference)| {
            let key = key(net, "transport-key", &sender_key(&from), id.as_str());
            Ok((key, serde_json::to_vec(&receipt).map_err(io::Error::other)?, reference))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let bytes =
        entries.iter().map(|(key, value, reference)| key.bytes() + value.len() + reference.authority.len() + 16).sum();
    net.database()
        .write(bytes, move |connection| {
            let now = jiff::Timestamp::now().as_millisecond();
            for (key, value, reference) in entries {
                receipts::put(connection, &key, &value, KEEP, false, Some(&reference), now)?;
            }
            Ok(())
        })
        .await
        .map_err(io::Error::other)
}

/// Call after the cumulative receive position is durable and Ack finished writing
/// to the connection, never after merely enqueueing it.
pub async fn acknowledge(net: &NetDir, epoch: u64, sequence: u64) -> io::Result<()> {
    let through = Reference { authority: net.authority().to_owned(), epoch, sequence };
    net.database()
        .write(through.authority.len() + 16, move |connection| {
            receipts::acknowledge(connection, &through, jiff::Timestamp::now().as_millisecond())?;
            Ok(())
        })
        .await
        .map_err(io::Error::other)
}

pub(in crate::link) fn start(inner: &std::sync::Arc<super::super::Inner>) {
    let weak = std::sync::Arc::downgrade(inner);
    tokio::spawn(async move {
        let mut migrated = false;
        let mut previous = String::new();
        loop {
            let Some(inner) = weak.upgrade() else { return };
            if matches!(*inner.status.borrow(), super::super::Status::Stopped { .. }) {
                return;
            }
            if !migrated {
                let net = inner.dir.clone();
                match tokio::task::spawn_blocking(move || migrate::run(&net)).await {
                    Ok(Ok(count)) => {
                        if count != 0 {
                            inner.node.log(format!("moved {count} letter receipts into the database"));
                        }
                        migrated = true;
                    }
                    result => {
                        let reason = match result {
                            Ok(Err(error)) => error.to_string(),
                            Err(error) => error.to_string(),
                            _ => unreachable!(),
                        };
                        if reason != previous {
                            inner.node.log(format!("moving letter receipts: {reason}"));
                            previous = reason;
                        }
                    }
                }
            }
            if let Err(error) = inner
                .dir
                .database()
                .write(0, |connection| receipts::prune(connection, jiff::Timestamp::now().as_millisecond(), 4096))
                .await
            {
                let reason = error.to_string();
                if reason != previous {
                    inner.node.log(format!("retiring letter receipts: {reason}"));
                    previous = reason;
                }
            }
            drop(inner);
            tokio::time::sleep(Duration::from_secs(if migrated { 60 } else { 10 })).await;
        }
    });
}
