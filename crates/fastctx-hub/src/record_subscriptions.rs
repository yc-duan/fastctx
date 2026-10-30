//! Modern computers follow plan and skill records they actually read. Phones
//! and older computers retain their account-wide metadata stream.

use crate::Hub;
use fastctx_net::sign::PublicKey;
use fastctx_types::{AccountName, MachineName, account_records::Metadata};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
static COUNT: AtomicUsize = AtomicUsize::new(0);

type Key = (AccountName, String, String);
#[derive(Default)]
pub(crate) struct Records {
    keys: HashSet<Key>,
    watches: HashMap<(AccountName, String), HashSet<String>>,
}

impl Drop for Records {
    fn drop(&mut self) {
        COUNT.fetch_sub(self.keys.len() + self.watches.values().map(HashSet::len).sum::<usize>(), Ordering::AcqRel);
    }
}

fn selective(kind: &str) -> bool {
    matches!(kind, fastctx_types::plan::KIND | fastctx_types::placement::KIND | fastctx_types::skills::KIND)
}

impl Records {
    pub(crate) fn wants(&self, account: &AccountName, metadata: &Metadata) -> bool {
        !selective(&metadata.kind)
            || self.keys.contains(&(account.clone(), metadata.kind.clone(), metadata.key.clone()))
            || self.watches.get(&(account.clone(), metadata.kind.clone())).is_some_and(|watches| !watches.is_empty())
    }
}

pub(crate) fn follow(
    hub: &Hub,
    from: &MachineName,
    key: &PublicKey,
    account: &AccountName,
    kind: &str,
    name: &str,
) -> Result<(), String> {
    if !selective(kind) {
        return Ok(());
    }
    let maximum =
        usize::try_from((fastctx_platform::memory::physical().map_err(|error| error.to_string())? / 16 / 1024).max(1))
            .unwrap_or(usize::MAX);
    let mut sessions = hub.sessions();
    let Some(session) = sessions.get_mut(from).filter(|session| session.key == *key) else {
        return Err("the record reader disconnected".into());
    };
    let Some(records) = &mut session.record_keys else {
        return Ok(());
    };
    let identity = (account.clone(), kind.into(), name.into());
    if records.keys.contains(&identity) {
        return Ok(());
    }
    COUNT
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| (count < maximum).then_some(count + 1))
        .map_err(|_| "record subscriptions reached this hub process's memory allowance".to_string())?;
    records.keys.insert(identity);
    Ok(())
}

pub(crate) fn deleted(hub: &Hub, from: &MachineName, account: &AccountName, kind: &str, name: &str) {
    if let Some(keys) = hub.sessions().get_mut(from).and_then(|session| session.record_keys.as_mut())
        && keys.keys.remove(&(account.clone(), kind.into(), name.into()))
    {
        COUNT.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) fn folder_reader(hub: &Hub, from: &MachineName, key: &PublicKey) -> bool {
    hub.sessions().get(from).is_some_and(|session| session.key == *key && session.skill_files)
}

/// Explicit listing watches are distinct from the individual copies a machine
/// holds. Each watch has an identity, so closing one cannot cancel another.
pub(crate) fn watch(
    hub: &Hub,
    from: &MachineName,
    key: &PublicKey,
    account: &AccountName,
    kind: &str,
    id: &str,
    following: bool,
) -> Result<(), String> {
    id.parse::<fastctx_types::NoticeId>().map_err(|error| error.to_string())?;
    if !selective(kind) {
        return Ok(());
    }
    let maximum =
        usize::try_from((fastctx_platform::memory::physical().map_err(|error| error.to_string())? / 16 / 1024).max(1))
            .unwrap_or(usize::MAX);
    let mut sessions = hub.sessions();
    let session =
        sessions.get_mut(from).filter(|session| session.key == *key).ok_or("the record reader disconnected")?;
    let Some(records) = &mut session.record_keys else {
        return Ok(());
    };
    let identity = (account.clone(), kind.into());
    if following {
        let watches = records.watches.entry(identity).or_default();
        if !watches.contains(id) {
            COUNT
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| (count < maximum).then_some(count + 1))
                .map_err(|_| "record subscriptions reached this hub process's memory allowance".to_string())?;
            watches.insert(id.into());
        }
    } else if let Some(watches) = records.watches.get_mut(&identity) {
        if watches.remove(id) {
            COUNT.fetch_sub(1, Ordering::AcqRel);
        }
        if watches.is_empty() {
            records.watches.remove(&identity);
        }
    }
    Ok(())
}
