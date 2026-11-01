//! Indexed records of work performed for a conversation. One writer owns
//! durable changes; readers take only a shard's short in-memory guard.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use fastctx_types::{Address, MachineName};
use tokio::sync::{mpsc, oneshot};

use crate::store::{
    rows::{Changes, Put},
    shelf::Shelf,
};

type Relation = (MachineName, Address);
type Shard = Mutex<HashMap<fastctx_types::ResidentId, HashSet<Relation>>>;
const SHARDS: usize = 64;

pub(crate) struct Worked {
    index: Arc<[Shard; SHARDS]>,
    send: Option<mpsc::Sender<Command>>,
}

enum Edit {
    Note(Relation),
    Delete(Address),
    Rename(fastctx_net::directory::Directory),
}
struct Command {
    edit: Edit,
    reply: oneshot::Sender<std::io::Result<()>>,
}

impl Default for Worked {
    fn default() -> Self {
        Self { index: Arc::new(std::array::from_fn(|_| Mutex::default())), send: None }
    }
}

fn shard(address: &Address) -> usize {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    address.resident.hash(&mut hasher);
    hasher.finish() as usize % SHARDS
}

impl Worked {
    pub(crate) fn new(shelf: Shelf, mut rows: BTreeSet<Relation>) -> std::io::Result<Arc<Self>> {
        let mut indexed: [HashMap<fastctx_types::ResidentId, HashSet<Relation>>; SHARDS] =
            std::array::from_fn(|_| HashMap::new());
        for (machine, address) in &rows {
            indexed[shard(address)]
                .entry(address.resident.clone())
                .or_default()
                .insert((machine.clone(), address.clone()));
        }
        let index = Arc::new(indexed.map(Mutex::new));
        let (send, mut receive) = mpsc::channel::<Command>(4096);
        let worker = index.clone();
        let runtime = tokio::runtime::Handle::current();
        std::thread::Builder::new().name("hub-work-records".into()).spawn(move || {
            let _entered = runtime.enter();
            while let Some(Command { edit, reply }) = receive.blocking_recv() {
                let (removed, added): (Vec<_>, Vec<_>) = match edit {
                    Edit::Note(relation) => {
                        (Vec::new(), (!rows.contains(&relation)).then_some(relation).into_iter().collect())
                    }
                    Edit::Delete(address) => {
                        (rows.iter().filter(|(_, at)| *at == address).cloned().collect(), Vec::new())
                    }
                    Edit::Rename(directory) => {
                        let index = fastctx_net::directory_index::Index::new(&directory);
                        let directory = fastctx_net::directory_index::Indexed::new(&directory, &index);
                        let mut removed = Vec::new();
                        let mut added = Vec::new();
                        for relation in &rows {
                            let renamed = (
                                directory.canonical(&relation.0).clone(),
                                Address {
                                    machine: directory.canonical(&relation.1.machine).clone(),
                                    resident: relation.1.resident.clone(),
                                },
                            );
                            if renamed.0.as_str() != relation.0.as_str()
                                || renamed.1.machine.as_str() != relation.1.machine.as_str()
                            {
                                removed.push(relation.clone());
                                added.push(renamed);
                            }
                        }
                        (removed, added)
                    }
                };
                let result = (|| {
                    let mut changes = Changes::default();
                    for row in &removed {
                        changes.deleted.push(("worked", crate::store::rows::composite(row).into()));
                    }
                    for row in &added {
                        changes.put.push(Put {
                            kind: "worked",
                            key: crate::store::rows::composite(row).into(),
                            position: 0,
                            body: crate::store::rows::body(row)?,
                        });
                    }
                    runtime.block_on(shelf.write_async(changes))?;
                    for (machine, address) in removed {
                        rows.remove(&(machine.clone(), address.clone()));
                        let mut index = worker[shard(&address)].lock().expect("work index");
                        if let Some(relations) = index.get_mut(&address.resident) {
                            relations.remove(&(machine, address.clone()));
                            if relations.is_empty() {
                                index.remove(&address.resident);
                            }
                        }
                    }
                    for (machine, address) in added {
                        rows.insert((machine.clone(), address.clone()));
                        worker[shard(&address)]
                            .lock()
                            .expect("work index")
                            .entry(address.resident.clone())
                            .or_default()
                            .insert((machine, address));
                    }
                    Ok(())
                })();
                let _ = reply.send(result);
            }
        })?;
        Ok(Arc::new(Self { index, send: Some(send) }))
    }

    pub(crate) fn contains(&self, (machine, address): &Relation) -> bool {
        self.index[shard(address)]
            .lock()
            .expect("work index")
            .get(&address.resident)
            .is_some_and(|relations| relations.contains(&(machine.clone(), address.clone())))
    }

    pub(crate) fn for_resident(&self, address: &Address) -> Vec<Relation> {
        self.index[shard(address)]
            .lock()
            .expect("work index")
            .get(&address.resident)
            .map(|relations| relations.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn edit(&self, edit: Edit) -> std::io::Result<()> {
        let (reply, receive) = oneshot::channel();
        let sender = self.send.as_ref().ok_or_else(|| std::io::Error::other("work writer not started"))?;
        sender.blocking_send(Command { edit, reply }).map_err(std::io::Error::other)?;
        receive.blocking_recv().map_err(std::io::Error::other)?
    }
    pub(crate) fn note(&self, machine: &MachineName, address: Address) -> std::io::Result<()> {
        if self.contains(&(machine.clone(), address.clone())) {
            return Ok(());
        }
        self.edit(Edit::Note((machine.clone(), address)))
    }
    pub(crate) fn delete(&self, address: &Address) -> std::io::Result<()> {
        self.edit(Edit::Delete(address.clone()))
    }
    pub(crate) fn rename(&self, directory: fastctx_net::directory::Directory) -> std::io::Result<()> {
        self.edit(Edit::Rename(directory))
    }
}
