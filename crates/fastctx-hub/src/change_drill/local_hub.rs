//! Builds the hub's local state without binding or starting network workers.
use crate::{Hub, Store, locks::Mutex};
use fastctx_platform::doing::Rank;
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, atomic::AtomicU64},
};

pub(super) fn open(dir: &Path) -> anyhow::Result<Arc<Hub>> {
    let mut store = Store::open(dir)?;
    let log = Arc::new(crate::log::Log::open(&dir.join("probe.log"))?);
    let certificate = fastctx_net::tls::HubCertificate {
        certificate: fastctx_platform::disk::read(Store::certificate_file(dir))?,
        key: fastctx_platform::disk::read(Store::key_file(dir))?,
    };
    let secrets = fastctx_net::login::HubSecrets::from_bytes(&fastctx_platform::disk::read(Store::secrets_file(dir))?)
        .map_err(anyhow::Error::msg)?;
    let account_records = crate::account_records::Records::open(dir, &store.book.directory)?;
    let ledgers = crate::ledger::Ledgers::open(dir, &store.hub.id)?;
    let updates =
        crate::updates::Updates::new(dir, &store.shelf, &store.hub.due, &mut Vec::new()).map_err(anyhow::Error::msg)?;
    let troubles = crate::troubles::Troubles::open(store.shelf.clone())?;
    let releases = crate::releases::Releases::new(dir, certificate.certificate.clone())?;
    let post = store.enable_runtime()?;
    let (switched, _switch) = tokio::sync::oneshot::channel();
    let hub = Arc::new(Hub {
        releases: Arc::new(releases),
        catalog: crate::catalog::Catalog::new(store)?,
        post,
        this: std::sync::OnceLock::new(),
        network: tokio::runtime::Handle::current(),
        secrets,
        pin: certificate.pin(),
        sessions: Mutex::new("hub sessions", Rank::HUB_SESSIONS, HashMap::new()),
        given: Mutex::new("hub given", Rank::HUB_GIVEN, HashMap::new()),
        trials: Mutex::new("hub trials", Rank::HUB_TRIALS, crate::direct::Trials::default()),
        calls: Mutex::new("hub calls", Rank::HUB_CALLS, crate::control::InFlight::default()),
        data: Mutex::new("hub data", Rank::HUB_DATA, crate::data::Pairing::default()),
        ledgers: Mutex::new("hub ledgers", Rank::HUB_LEDGERS, ledgers),
        limits: Mutex::new("hub limits", Rank::HUB_LIMITS, crate::login::Limits::default()),
        next: AtomicU64::new(0),
        log,
        machine_log_changed: Default::default(),
        moving: crate::moving::Moving::new(switched),
        assists_changed: Default::default(),
        next_assist_end: Default::default(),
        afoots: Default::default(),
        updates: Mutex::new("hub updates", Rank::HUB_UPDATES, updates),
        dissolution: Default::default(),
        directory_changed: Default::default(),
        presence_changed: Default::default(),
        presence: AtomicU64::new(0),
        pending_presence: Mutex::new("hub presence", Rank::HUB_PRESENCE, Default::default()),
        presence_pending: Default::default(),
        health: Default::default(),
        diagnostic_dir: dir.to_path_buf(),
        account_records,
        troubles,
        workbench: Mutex::new("hub workbench", Rank::HUB_WORKBENCH, HashMap::new()),
        foundation: None,
    });
    let _ = hub.this.set(Arc::downgrade(&hub));
    let members = hub.store().book.directory.members.clone();
    for member in members {
        hub.sessions().insert(
            member.name.clone(),
            crate::Session {
                asset_sync: None,
                replicated_records: false,
                key: member.key,
                account_records: false,
                record_keys: None,
                skill_files: false,
                conversation_plans: true,
                scratch_locations: false,
                call_details: false,
                generation: 0,
                out: crate::outq::Outq::new(false),
                since: member.added,
                peer: "127.0.0.1:1".parse()?,
                through: None,
                direct: false,
                interfaces: Vec::new(),
                paths: None,
                strain: None,
                afoot: true,
                forgets: true,
                forgets_accounts: true,
                versions: true,
                version_parts: false,
                cancels: true,
                presence: true,
                project_entry: true,
                view_deltas: true,
            },
        );
    }
    Ok(hub)
}
