//! Metadata hints refresh only skill folders already held on this computer.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) struct Held {
    account: HubAccount,
    name: String,
    dirty: AtomicBool,
    running: AtomicBool,
}
pub(super) fn register(node: &Arc<Node>, account: &HubAccount, name: &str) -> Arc<Held> {
    node.skill_copies
        .held
        .lock()
        .expect("never poisoned")
        .entry((account.clone(), name.into()))
        .or_insert_with(|| {
            Arc::new(Held {
                account: account.clone(),
                name: name.into(),
                dirty: AtomicBool::new(false),
                running: AtomicBool::new(false),
            })
        })
        .clone()
}
fn request(node: &Arc<Node>, held: Arc<Held>) {
    held.dirty.store(true, Ordering::Release);
    if held.running.swap(true, Ordering::AcqRel) {
        return;
    }
    let weak = Arc::downgrade(node);
    tokio::spawn(async move {
        let mut retry = crate::again::Again::default();
        let mut previous = String::new();
        loop {
            held.dirty.store(false, Ordering::Release);
            let Some(node) = weak.upgrade() else {
                return;
            };
            let asker = Asker {
                machine: node.machine(),
                hub: held.account.hub.clone(),
                account: held.account.account.clone(),
                hands: true,
            };
            match prepare(&node, &asker, None, &held.name, None).await {
                Ok(_) => {
                    previous.clear();
                    retry = Default::default();
                }
                Err(error) => {
                    if error.message != previous {
                        node.log.line(format_args!("synchronizing skill {}: {}", held.name, error.message));
                        previous = error.message;
                    }
                    if !node
                        .skill_copies
                        .held
                        .lock()
                        .expect("never poisoned")
                        .contains_key(&(held.account.clone(), held.name.clone()))
                    {
                        break;
                    }
                    drop(node);
                    tokio::time::sleep(retry.next()).await;
                    continue;
                }
            }
            held.running.store(false, Ordering::Release);
            if !held.dirty.load(Ordering::Acquire) || held.running.swap(true, Ordering::AcqRel) {
                return;
            }
        }
        held.running.store(false, Ordering::Release);
    });
}
pub(crate) fn changed(
    node: &Arc<Node>,
    hub: &fastctx_types::access::HubId,
    change: &fastctx_types::account_records::Changed,
) {
    if change.metadata.kind != fastctx_types::skills::KIND {
        return;
    }
    let held = node
        .skill_copies
        .held
        .lock()
        .expect("never poisoned")
        .get(&(HubAccount { hub: hub.clone(), account: change.account.clone() }, change.metadata.key.clone()))
        .cloned();
    if let Some(held) = held {
        request(node, held);
    }
}
pub(crate) fn refresh(node: &Arc<Node>, hub: &fastctx_types::access::HubId) {
    let held: Vec<_> = node
        .skill_copies
        .held
        .lock()
        .expect("never poisoned")
        .values()
        .filter(|held| held.account.hub == *hub)
        .cloned()
        .collect();
    for held in held {
        request(node, held);
    }
}
pub(crate) fn start(node: &Arc<Node>) {
    let result = (|| -> Result<(), Failure> {
        let directories = match std::fs::read_dir(node.world.path().join("skill-copies")) {
            Ok(directories) => directories,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(failure(error)),
        };
        for directory in directories {
            let directory = directory.map_err(failure)?.path();
            if !directory.is_dir() {
                continue;
            }
            // One account's folders that cannot be restored leave the others theirs.
            let restored = (|| -> Result<(Identity, Kept), Failure> {
                let identity: Identity =
                    serde_json::from_slice(&std::fs::read(directory.join("account.json")).map_err(failure)?)
                        .map_err(failure)?;
                if root(node, &identity.account, identity.opened)? != directory {
                    return Err(failure("skill-copy identity differs from its directory"));
                }
                Ok((identity, load(&directory)?))
            })();
            match restored {
                Ok((identity, kept)) => {
                    for generation in kept.generations.values() {
                        request(node, register(node, &identity.account, &generation.name));
                    }
                }
                Err(error) => node.log.line(format_args!("restoring skill folders in {}: {}", directory.display(), error.message)),
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        node.log.line(format_args!("restoring skill folders: {}", error.message));
    }
}
