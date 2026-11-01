//! Durable mail is serialized per member identity, independently of catalog
//! edits. Each member owns its counters, mailbox and connection generation.

mod member;
mod storage;

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock, Weak};

use arc_swap::{ArcSwap, ArcSwapOption};
use fastctx_net::directory::Directory;
use fastctx_net::envelope::Envelope;
use fastctx_net::sign::PublicKey;
use fastctx_types::MachineName;
use tokio::sync::{mpsc, oneshot};

use crate::store::{Letters, shelf::Shelf};

pub(crate) type Result<T> = std::result::Result<T, String>;

pub(crate) struct Post {
    dir: std::path::PathBuf,
    shelf: Shelf,
    members: ArcSwap<HashMap<PublicKey, Arc<Member>>>,
    hub: Arc<OnceLock<Weak<crate::Hub>>>,
    runtime: tokio::runtime::Handle,
}

pub(crate) struct Member {
    pub record: Arc<ArcSwapOption<Letters>>,
    pub generation: Arc<std::sync::atomic::AtomicU64>,
    pub retired: Arc<std::sync::atomic::AtomicBool>,
    pub backlog: Arc<member::Backlog>,
    send: mpsc::Sender<Queued>,
    hub: Arc<OnceLock<Weak<crate::Hub>>>,
}

#[derive(Clone, Default)]
pub(crate) struct Read {
    named: Arc<BTreeMap<MachineName, Arc<Member>>>,
}

impl Read {
    pub(crate) fn get(&self, name: &MachineName) -> Option<Arc<Letters>> {
        self.named.get(name)?.record.load_full()
    }
    pub(crate) fn member(&self, name: &MachineName) -> Option<Arc<Member>> {
        self.named.get(name).cloned()
    }
    pub(crate) fn keys(&self) -> impl Iterator<Item = &MachineName> {
        self.named.keys()
    }
}

pub(super) struct Queued {
    command: Command,
    _active: Option<crate::moving::Active>,
    _pending: Pending,
}

struct Pending {
    backlog: Arc<member::Backlog>,
    writing: bool,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.backlog.pending.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        if self.writing {
            self.backlog.writing.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

type Register = Box<dyn FnOnce(&Letters) -> Result<()> + Send>;

pub(crate) enum Command {
    Open {
        generation: u64,
        letters: fastctx_net::wire::Letters,
        capabilities: Capabilities,
        out: crate::outq::Outq,
        register: Register,
        reply: oneshot::Sender<Result<Letters>>,
    },
    Close {
        generation: u64,
    },
    Viewed {
        generation: u64,
    },
    Taken {
        generation: u64,
        through: u64,
    },
    Heard {
        generation: u64,
        after: Option<u64>,
        through: u64,
    },
    Count {
        generation: u64,
        seq: u64,
        refused: Option<String>,
        reply: oneshot::Sender<Result<()>>,
    },
    Put {
        message: Box<Envelope>,
        reply: oneshot::Sender<Result<()>>,
    },
    DropFor {
        resident: fastctx_types::ResidentId,
        reply: oneshot::Sender<Result<usize>>,
    },
    Expire {
        before: jiff::Timestamp,
        reply: oneshot::Sender<Result<usize>>,
    },
    Names {
        reply: oneshot::Sender<Result<()>>,
    },
    Retire {
        reply: oneshot::Sender<Result<()>>,
    },
    Flush {
        reply: oneshot::Sender<Result<()>>,
    },
}

#[derive(Clone)]
pub(crate) struct Capabilities {
    pub any_names: bool,
    pub renames: bool,
    pub forgets: bool,
    pub roles: bool,
    pub versions: bool,
    pub due: bool,
    pub project_deletes: bool,
    pub conversation_deletions: bool,
    pub profiles: bool,
}

impl From<&crate::control::Said> for Capabilities {
    fn from(said: &crate::control::Said) -> Self {
        Self {
            any_names: said.any_names,
            renames: said.renames,
            forgets: said.forgets,
            roles: said.roles,
            versions: said.versions,
            due: said.due,
            project_deletes: said.project_deletes,
            conversation_deletions: said.conversation_deletions,
            profiles: said.profiles,
        }
    }
}

impl Post {
    pub(crate) fn new(
        dir: std::path::PathBuf,
        shelf: Shelf,
        directory: &Directory,
        letters: &crate::store::LetterRows,
    ) -> Arc<Self> {
        let post = Arc::new(Self {
            dir,
            shelf,
            members: ArcSwap::from_pointee(HashMap::new()),
            hub: Arc::new(OnceLock::new()),
            runtime: tokio::runtime::Handle::current(),
        });
        let members = directory
            .members
            .iter()
            .map(|member| {
                let state = post.start(member.key, member.name.clone(), letters.get(&member.name).cloned());
                (member.key, state)
            })
            .collect();
        post.members.store(Arc::new(members));
        post
    }

    fn start(&self, key: PublicKey, name: MachineName, initial: Option<Letters>) -> Arc<Member> {
        let (send, receive) = mpsc::channel(256);
        let record = Arc::new(ArcSwapOption::from(initial.clone().map(Arc::new)));
        let generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let backlog = Arc::new(member::Backlog::default());
        let retired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let member = Arc::new(Member {
            record: record.clone(),
            generation: generation.clone(),
            retired: retired.clone(),
            backlog: backlog.clone(),
            send,
            hub: self.hub.clone(),
        });
        fastctx_platform::task::spawn_on(
            &self.runtime,
            member::run(
                member::State::new(
                    self.dir.clone(),
                    self.shelf.clone(),
                    self.hub.clone(),
                    key,
                    name,
                    initial,
                    record,
                    generation,
                    retired,
                    backlog,
                ),
                receive,
            ),
        );
        member
    }

    /// Called by the single catalog writer before publishing its snapshot.
    pub(crate) fn view(&self, directory: &Directory) -> Read {
        let before = self.members.load_full();
        let mut added = None;
        let mut named = BTreeMap::new();
        for member in &directory.members {
            let state = match before.get(&member.key) {
                Some(state) => state.clone(),
                None => {
                    let state = self.start(member.key, member.name.clone(), None);
                    added.get_or_insert_with(|| (*before).clone()).insert(member.key, state.clone());
                    state
                }
            };
            named.insert(member.name.clone(), state);
        }
        // Current names precede aliases, matching Directory::member.
        for member in &directory.members {
            let state = named[&member.name].clone();
            for alias in &member.was {
                named.entry(alias.clone()).or_insert_with(|| state.clone());
            }
        }
        if directory.members.len() != before.len() || added.is_some() {
            let mut next = added.unwrap_or_else(|| (*before).clone());
            let active: std::collections::HashSet<_> = directory.members.iter().map(|member| member.key).collect();
            next.retain(|key, _| active.contains(key));
            self.members.store(Arc::new(next));
        }
        Read { named: Arc::new(named) }
    }

    pub(crate) fn attach(&self, hub: Weak<crate::Hub>) {
        let _ = self.hub.set(hub);
    }

    pub(crate) async fn flush(&self) -> Result<()> {
        for member in self.members.load_full().values() {
            member.request(|reply| Command::Flush { reply }).await?;
        }
        Ok(())
    }
}

impl Member {
    fn queued(&self, command: Command) -> Queued {
        let active = self.hub.get().and_then(Weak::upgrade).map(|hub| crate::moving::Moving::hold(&hub));
        let writing = matches!(&command, Command::Put { .. });
        self.backlog.pending.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if writing {
            self.backlog.writing.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Queued { command, _active: active, _pending: Pending { backlog: self.backlog.clone(), writing } }
    }

    pub(crate) async fn request<T>(&self, make: impl FnOnce(oneshot::Sender<Result<T>>) -> Command) -> Result<T> {
        let (reply, answer) = oneshot::channel();
        let queued = self.queued(make(reply));
        self.send.send(queued).await.map_err(|_| "member mail worker stopped")?;
        answer.await.map_err(|_| "member mail worker stopped")?
    }

    pub(crate) async fn tell(&self, command: Command) -> Result<()> {
        self.send.send(self.queued(command)).await.map_err(|_| "member mail worker stopped".into())
    }

    pub(crate) fn retire(self: &Arc<Self>) {
        self.retired.store(true, std::sync::atomic::Ordering::Release);
        self.generation.store(0, std::sync::atomic::Ordering::Release);
        let (reply, _) = oneshot::channel();
        self.later(Command::Retire { reply });
    }

    pub(crate) fn later(self: &Arc<Self>, command: Command) {
        let member = self.clone();
        let queued = self.queued(command);
        fastctx_platform::task::spawn(async move {
            let _ = member.send.send(queued).await;
        });
    }
}
