//! Share an in-flight replica-path query, never a result retained after its readers leave.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, Weak};

use fastctx_net::directory::View;
use fastctx_net::link::Link;
use fastctx_types::{
    AccountName, MachineName, TownName,
    access::Purpose,
    memory::Folders,
    remote::{RemoteCall, RemoteReply},
};
use tokio::sync::OnceCell;

type Answer = OnceCell<Result<Folders, String>>;

#[derive(Default)]
pub(crate) struct Reads(Mutex<HashMap<Key, Weak<Answer>>>);

pub(crate) struct Query {
    pub link: Arc<Link>,
    pub view: Arc<View>,
    pub account: AccountName,
    pub machine: MachineName,
    pub town: TownName,
    pub ways: u64,
    pub memory: u64,
}

#[derive(Clone)]
struct Key {
    view: Arc<View>,
    account: AccountName,
    machine: MachineName,
    town: TownName,
    ways: u64,
    memory: u64,
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.view, &other.view)
            && self.account == other.account
            && self.machine == other.machine
            && self.town == other.town
            && self.ways == other.ways
            && self.memory == other.memory
    }
}
impl Eq for Key {}
impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.view).hash(state);
        self.account.hash(state);
        self.machine.hash(state);
        self.town.hash(state);
        self.ways.hash(state);
        self.memory.hash(state);
    }
}

struct Reading<'a> {
    reads: &'a Reads,
    key: Key,
    answer: Arc<Answer>,
}

impl Drop for Reading<'_> {
    fn drop(&mut self) {
        let mut reads = self.reads.0.lock().expect("never poisoned");
        if Arc::strong_count(&self.answer) == 1
            && reads.get(&self.key).is_some_and(|answer| Weak::ptr_eq(answer, &Arc::downgrade(&self.answer)))
        {
            reads.remove(&self.key);
        }
    }
}

impl Reads {
    /// Only callers observing the same immutable view and invalidation counters
    /// share work. Each caller must recheck current authorization after this returns.
    pub(crate) async fn fetch(&self, query: Query) -> Result<Folders, String> {
        let key = Key {
            view: query.view,
            account: query.account.clone(),
            machine: query.machine.clone(),
            town: query.town.clone(),
            ways: query.ways,
            memory: query.memory,
        };
        let reading = {
            let mut reads = self.0.lock().expect("never poisoned");
            let answer = reads.get(&key).and_then(Weak::upgrade).unwrap_or_else(|| {
                let answer = Arc::new(OnceCell::new());
                reads.insert(key.clone(), Arc::downgrade(&answer));
                answer
            });
            Reading { reads: self, key, answer }
        };
        reading.answer.get_or_init(|| fetch(&query.link, &query.machine, &query.account, query.town)).await.clone()
    }
}

async fn fetch(link: &Link, machine: &MachineName, account: &AccountName, town: TownName) -> Result<Folders, String> {
    let facts = link
        .call(machine, account, Purpose::Look, RemoteCall::Facts)
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.message)?;
    let RemoteReply::Facts(facts) = facts else { return Err(fastctx_types::memory::words::COMPUTER_UNANSWERED.into()) };
    if !facts.memory_folders {
        return Err(fastctx_types::memory::words::paths_old(machine));
    }
    let reply = link
        .call(machine, account, Purpose::Hands, RemoteCall::MemoryFolders { town })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.message)?;
    let RemoteReply::MemoryFolders(folders) = reply else {
        return Err(fastctx_types::memory::words::PATHS_UNANSWERED.into());
    };
    Ok(folders)
}
