//! Share authoritative catalogs before each caller redacts their provenance.

mod authority;
use super::Link;
use crate::directory::View;
use fastctx_types::{
    AccountName, Address,
    access::TownId,
    account_records::Changed,
    history::{Catalog, Scope},
    owned_records::{Conversation, Request, Response},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::{OnceCell, watch};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Source {
    Project(TownId),
    Conversation(Address),
}
type Key = (AccountName, Source);
type CatalogCell = Arc<OnceCell<Arc<Catalog>>>;

struct State {
    cell: CatalogCell,
    owner: Option<Conversation>,
    ownership: Arc<OnceCell<Conversation>>,
    authority: authority::Fingerprint,
}
struct Entry {
    state: Mutex<State>,
    changes: watch::Sender<u64>,
}
impl Entry {
    fn invalidate(&self, state: &mut State) {
        state.cell = Arc::new(OnceCell::new());
        self.changes.send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    fn check(&self, catalogs: &Catalogs, key: &Key, view: &Arc<View>) -> CatalogCell {
        let mut state = self.state.lock().expect("history catalog");
        let authority = catalogs.authority(view, key, state.owner.as_ref());
        let changed = authority != state.authority;
        state.authority = authority;
        if changed {
            self.invalidate(&mut state);
        }
        state.cell.clone()
    }
}

#[derive(Default)]
pub(super) struct Catalogs {
    entries: Mutex<BTreeMap<Key, Arc<Entry>>>,
    authority: Mutex<authority::Cache>,
}
impl Catalogs {
    fn authority(&self, view: &Arc<View>, key: &Key, owner: Option<&Conversation>) -> authority::Fingerprint {
        self.authority.lock().expect("catalog authority").get(view, key, owner)
    }
    fn entry(&self, key: &Key, view: &Arc<View>) -> Arc<Entry> {
        self.entries
            .lock()
            .expect("history catalogs")
            .entry(key.clone())
            .or_insert_with(|| {
                Arc::new(Entry {
                    state: Mutex::new(State {
                        cell: Arc::new(OnceCell::new()),
                        owner: None,
                        ownership: Arc::new(OnceCell::new()),
                        authority: self.authority(view, key, None),
                    }),
                    changes: watch::channel(0).0,
                })
            })
            .clone()
    }
    pub(super) fn project(&self, town: &TownId) {
        self.invalidate(|(_, source)| matches!(source, Source::Project(found) if found == town));
    }
    pub(super) fn ownership(&self) {
        for ((_, source), entry) in self.entries.lock().expect("history catalogs").iter() {
            if matches!(source, Source::Conversation(_)) {
                entry.state.lock().expect("history catalog").ownership = Arc::new(OnceCell::new());
                entry.changes.send_modify(|revision| *revision = revision.wrapping_add(1));
            }
        }
    }

    pub(super) fn record(&self, change: &Changed) {
        let owner = match change.metadata.kind.as_str() {
            fastctx_types::history::RECORD_KIND => change.metadata.key.split_once('/').map(|(owner, _)| owner),
            fastctx_types::owned_records::KIND => Some(change.metadata.key.as_str()),
            _ => None,
        };
        let Some(owner) = owner else { return };
        for ((_, source), entry) in self.entries.lock().expect("history catalogs").iter() {
            if !matches!(source, Source::Conversation(address) if address.resident.as_str() == owner) {
                continue;
            }
            let mut state = entry.state.lock().expect("history catalog");
            if state.owner.as_ref().is_some_and(|owner| owner.account != change.account) {
                continue;
            }
            if change.metadata.kind == fastctx_types::owned_records::KIND {
                state.ownership = Arc::new(OnceCell::new());
            }
            entry.invalidate(&mut state);
        }
    }
    pub(super) fn invalidate(&self, matches: impl Fn(&Key) -> bool) {
        for (key, entry) in self.entries.lock().expect("history catalogs").iter() {
            if matches(key) {
                let mut state = entry.state.lock().expect("history catalog");
                entry.invalidate(&mut state);
            }
        }
    }
    pub(super) fn view(&self, view: &Arc<View>) {
        for (key, entry) in self.entries.lock().expect("history catalogs").iter() {
            entry.check(self, key, view);
        }
    }
}

impl Link {
    fn history_source(&self, account: &AccountName, scope: &Scope) -> Result<Source, String> {
        Ok(match scope {
            Scope::Project { project } => Source::Project(
                self.view().project_named(account, project).ok_or("this account cannot use that project")?.town.clone(),
            ),
            Scope::Conversation { resident, machine } => {
                Source::Conversation(Address { resident: resident.clone(), machine: machine.clone() })
            }
        })
    }

    /// Candidate signed-in readers, without resolving a conversation's ownership.
    pub fn history_readers(&self, scope: &Scope) -> Vec<AccountName> {
        let view = self.view();
        view.me()
            .into_iter()
            .flat_map(|member| &member.logins)
            .filter(|account| match scope {
                Scope::Project { project } => view.project_named(account, project).is_some(),
                Scope::Conversation { .. } => true,
            })
            .cloned()
            .collect()
    }

    /// Subscribe before reading. Only this scope's records or authority wake it.
    pub fn history_catalog_changes(
        &self,
        account: &AccountName,
        scope: &Scope,
    ) -> Result<watch::Receiver<u64>, String> {
        let key = (account.clone(), self.history_source(account, scope)?);
        Ok(self.inner.history_catalogs.entry(&key, &self.view()).changes.subscribe())
    }

    /// The account's catalog is shared before callers apply their own reader scope.
    pub async fn shared_history_catalog(&self, account: &AccountName, town: &TownId) -> Result<Arc<Catalog>, String> {
        self.catalog((account.clone(), Source::Project(town.clone()))).await
    }

    /// Conversation aliases remain separate keys; record hints invalidate all of them.
    pub async fn shared_conversation_history_catalog(
        &self,
        account: &AccountName,
        address: &Address,
    ) -> Result<Arc<Catalog>, String> {
        self.catalog((account.clone(), Source::Conversation(address.clone()))).await
    }

    async fn catalog(&self, key: Key) -> Result<Arc<Catalog>, String> {
        let entry = self.inner.history_catalogs.entry(&key, &self.view());
        loop {
            if !self.view_current() {
                return Err("the hub's current account view is unavailable".into());
            }
            let ownership = entry.state.lock().expect("history catalog").ownership.clone();
            if let Source::Conversation(address) = &key.1 {
                let owner = ownership
                    .get_or_try_init(|| async {
                        match self.owned_records(&key.0, Request::Resolve { conversation: address.clone() }).await? {
                            Response::Conversation { conversation } => Ok(conversation),
                            _ => Err("the history's conversation ownership is unavailable".to_string()),
                        }
                    })
                    .await?;
                let mut state = entry.state.lock().expect("history catalog");
                if !Arc::ptr_eq(&ownership, &state.ownership) {
                    continue;
                }
                if state.owner.as_ref() != Some(owner) {
                    if state.owner.is_some() {
                        entry.invalidate(&mut state);
                    }
                    state.owner = Some(owner.clone());
                    state.authority = self.inner.history_catalogs.authority(&self.view(), &key, state.owner.as_ref());
                }
            }
            let cell = entry.check(&self.inner.history_catalogs, &key, &self.view());
            let catalog = cell
                .get_or_try_init(|| async {
                    let mut catalog = match &key.1 {
                        Source::Project(town) => self.history_catalog(&key.0, town).await?,
                        Source::Conversation(address) => self.conversation_history_catalog(&key.0, address).await?,
                    };
                    catalog.sort();
                    Ok::<_, String>(Arc::new(catalog))
                })
                .await?;
            let current = entry.check(&self.inner.history_catalogs, &key, &self.view());
            let owner_current = Arc::ptr_eq(&ownership, &entry.state.lock().expect("history catalog").ownership);
            if self.view_current() && owner_current && Arc::ptr_eq(&cell, &current) {
                return Ok(catalog.clone());
            }
        }
    }
}
