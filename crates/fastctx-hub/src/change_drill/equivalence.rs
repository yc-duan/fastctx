//! Per-change equivalence probe. Not part of the default test suite.
use crate::{Hub, HubFile, Store};
use anyhow::Context;
use fastctx_net::{
    directory::{Level, Share},
    sign::PublicKey,
    wire::Change,
};
use fastctx_types::{collab::Collab, profile::Profile};
use jiff::Timestamp;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

const KINDS: [&str; 13] = [
    "History",
    "Collab",
    "Profile",
    "MemorySettings",
    "Layer",
    "Subagents",
    "Group",
    "RenameGroup",
    "RevokeToken",
    "Settings",
    "Updates",
    "Rollout",
    "Intake",
];

pub(super) fn run() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut root = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--equivalence" => {}
            "--root" => root = args.next().map(PathBuf::from),
            _ => anyhow::bail!("use --equivalence --root PATH"),
        }
    }
    let root = root.context("--root required")?;
    anyhow::ensure!(!fastctx_platform::disk::exists(&root)?, "use a fresh root");
    fastctx_platform::disk::create_dir_all(&root)?;
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let _entered = runtime.enter();
    for seed in 0..30 {
        let base = root.join(format!("seed-{seed}"));
        make_fixture(&base, seed)?;
        for kind in KINDS {
            for round in 0..2 {
                exercise(&root, &base, seed, kind, round)
                    .with_context(|| format!("seed={seed} change={kind} round={round}"))?;
            }
        }
        if seed == 0 {
            negative_control(&root, &base)?;
            super::forwarding::run(&root, &base)?;
        }
        println!("network={seed} changes_checked={}", KINDS.len() * 2);
    }
    for kind in KINDS {
        println!("change={kind} networks=30 changes=60 reopened_pairs=60 persisted_equal=true full_effects_empty=true");
    }
    println!("unproved_full=Plan,Token,PasswordStart,PasswordFinish,UpdateDue");
    Ok(())
}

fn make_fixture(dir: &Path, seed: u64) -> anyhow::Result<()> {
    let mut random = seed + 12345;
    let mut next = || {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        random
    };
    let machines = 8 + 2 * (next() as usize % 9);
    let mut book = super::fixture(machines);
    for account in book.directory.accounts.iter_mut().skip(1) {
        let account = Arc::make_mut(account);
        account.level = if next() % 3 == 0 { Level::Visitor } else { Level::Member };
        account.locked = next() % 7 == 0;
    }
    for member in &mut book.directory.members {
        Arc::make_mut(member).group = Some("before".into());
    }
    for _ in 0..machines {
        book.directory.shares.push(Arc::new(Share {
            machine: super::machine(next() as usize % machines),
            account: super::account(next() as usize % (machines / 2)),
            jobs: None,
        }));
    }
    let at = Timestamp::from_second(1_700_000_000)?;
    book.directory.projects.push(Arc::new(fastctx_net::directory::Project {
        town: fastctx_types::access::TownId::from_bytes([7; 8]),
        name: "probe".parse()?,
        owner: super::account(0),
        granted: Default::default(),
        shared_on: None,
    }));
    for i in 0..2 {
        book.tokens.push(Arc::new(crate::store::TokenRecord {
            key: token_key(i),
            account: super::account(0),
            register: true,
            name: None,
            kind: fastctx_net::directory::Kind::Computer,
            expires: Timestamp::from_second(4_000_000_000)?,
            managed: false,
            uses: 3,
            label: Some("probe".into()),
            revoked: false,
            made_by: super::machine(0),
            made_at: at,
            used_by: None,
            used: Vec::new(),
            way: None,
        }));
    }
    book.forgetting = serde_json::from_value(json!({
        "last": 1, "pending": [{
            "forgetting": {"id": 1, "conversation": {"machine": "m0", "resident": "codex-abcd"}, "account": "a0"},
            "asked_by": "m0", "at": at, "home": true, "waiting": ["m0", "m1"]
        }]
    }))?;
    book.account_forgetting = serde_json::from_value(json!({
        "last": 1, "pending": [{"forgetting": {"id": 1, "account": "deleted", "at": at}, "waiting": ["m0", "m1"]}]
    }))?;
    let certificate = fastctx_net::tls::HubCertificate::generate().map_err(anyhow::Error::msg)?;
    let secrets = fastctx_net::login::HubSecrets::generate();
    let hub = HubFile {
        id: fastctx_net::ids::HubId::from_bytes([42; 8]),
        name: "equivalence".parse()?,
        public: Vec::new(),
        listen: "127.0.0.1:0".parse()?,
        self_registration: false,
        dissolved: None,
        updates: Default::default(),
        due: Vec::new(),
        repairs: Vec::new(),
        package_targets: book
            .directory
            .members
            .iter()
            .map(|member| {
                (member.key, crate::store::PackageTargets { kernel: Some("x86_64-pc-windows-msvc".into()), app: None })
            })
            .collect(),
    };
    Store::create(dir, &hub, &certificate, &secrets.to_bytes(), &book)?;
    let mut store = Store::open(dir)?;
    let mover = store.book.directory.members[0].key;
    let members = store.book.directory.members.iter().map(|member| (member.name.clone(), true)).collect();
    store.moves.push(crate::moving::Incoming {
        id: "probe-move".into(),
        to: super::machine(0),
        addresses: Vec::new(),
        from: Vec::new(),
        by: super::account(0),
        started: at,
        switched: at,
        mover,
        members,
        followed: Default::default(),
        old: fastctx_types::hub_move::OldPlace::Stopped { at, early: false },
    });
    store.save_moves()?;
    store.note_worked(
        &super::machine(0),
        fastctx_types::Address { machine: super::machine(0), resident: "codex-abcd".parse()? },
    )?;
    drop(store);
    wait_closed(dir)?;
    Ok(())
}

fn token_key(i: usize) -> PublicKey {
    let mut key = [231; 32];
    key[0] = i as u8;
    PublicKey::from_bytes(key)
}

pub(super) fn wait_closed(dir: &Path) -> anyhow::Result<()> {
    let keeper = dir.join(format!("{}-keeper", crate::store::shelf::FILE));
    let began = std::time::Instant::now();
    loop {
        if let Some(lock) = fastctx_platform::lock::FileLock::try_acquire(&keeper)? {
            drop(lock);
            return Ok(());
        }
        anyhow::ensure!(
            began.elapsed() < std::time::Duration::from_secs(10),
            "database did not close: {}",
            dir.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

pub(super) fn copy_dir(from: &Path, to: &Path) -> anyhow::Result<()> {
    fastctx_platform::disk::create_dir_all(to)?;
    for entry in fastctx_platform::disk::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fastctx_platform::disk::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn request(kind: &str, round: usize, hub: &Hub) -> Change {
    let store = hub.store();
    let account = store.book.directory.account(&super::account(0)).expect("account");
    let version = account.profile.as_ref().map_or(0, |profile| profile.version);
    match kind {
        "History" => Change::History { on: round == 0 },
        "Collab" => Change::Collab {
            collab: Collab {
                version: account.collab.as_ref().map_or(0, |collab| collab.version),
                on: round == 0,
                rules: format!("probe {round}"),
                ..Default::default()
            },
        },
        "Profile" => {
            Change::Profile { profile: Profile { version, prompt: format!("probe {round}"), ..Default::default() } }
        }
        "MemorySettings" => Change::MemorySettings { version, enabled: round == 0 },
        "Layer" => Change::Layer {
            version,
            layer: if round == 0 {
                fastctx_types::layers::Layer::Workbench
            } else {
                fastctx_types::layers::Layer::Connect
            },
        },
        "Subagents" => Change::Subagents {
            version,
            subagents: if round == 0 {
                Some(fastctx_types::profile::Subagents {
                    agents: Vec::new(),
                    instructions: "probe instructions".into(),
                })
            } else {
                None
            },
        },
        "Group" => {
            Change::Group { machine: super::machine(0), group: if round == 0 { Some("after".into()) } else { None } }
        }
        "RenameGroup" => Change::RenameGroup {
            owner: super::account(0),
            from: if round == 0 { "before" } else { "after" }.into(),
            to: Some(if round == 0 { "after" } else { "last" }.into()),
        },
        "RevokeToken" => Change::RevokeToken { token: token_key(round) },
        "Settings" => Change::Settings { self_registration: round == 0 },
        "Updates" => {
            let mut policy = store.hub.updates.clone();
            policy.minimum = Some(format!("1.0.{}", round + 1));
            Change::Updates { policy }
        }
        "Rollout" => Change::Rollout {
            rollout: if round == 0 {
                Some(fastctx_types::update::Rollout {
                    version: "1.0.2".into(),
                    first: ["m0".into()].into_iter().collect(),
                    share: 23,
                    paused: false,
                    stopped: None,
                })
            } else {
                None
            },
            withdrawn: [format!("1.0.{}", round + 1)].into_iter().collect(),
        },
        "Intake" => Change::Intake { rate: if round == 0 { Some(fastctx_types::update::INTAKE) } else { None } },
        _ => unreachable!(),
    }
}

fn effects(hub: &Hub) -> anyhow::Result<Value> {
    let store = hub.store();
    Ok(json!({
        "records": hub.account_records.health(), "updates": crate::updates::change_probe::snapshot(hub),
        "targets": store.hub.package_targets, "calls": crate::control::change_probe::snapshot(hub),
        "streams": crate::data::change_probe::snapshot(hub), "stream_counts": crate::data::counts(hub), "moves": serde_json::to_value(&store.moves)?,
        "forgetting": store.book.forgetting, "account_forgetting": store.book.account_forgetting,
        "afoot": crate::afoot::change_probe::snapshot(hub),
    }))
}

fn exercise(root: &Path, base: &Path, seed: u64, kind: &str, round: usize) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let entered = runtime.enter();
    let left_dir = root.join(format!("{seed}-{kind}-narrow"));
    let right_dir = root.join(format!("{seed}-{kind}-full"));
    if round == 0 {
        copy_dir(base, &left_dir)?;
        copy_dir(base, &right_dir)?;
    }
    let left = super::local_hub::open(&left_dir)?;
    let right = super::local_hub::open(&right_dir)?;
    let mut receivers = Vec::new();
    for hub in [&left, &right] {
        let standings = crate::control::change_probe::seed(hub);
        receivers.extend(crate::data::change_probe::seed(hub, &standings));
        crate::updates::change_probe::seed(hub);
        crate::updates::change_probe::flush(hub);
        crate::afoot::change_probe::seed(hub);
        hub.changed();
    }
    let mut left_projection = crate::afoot::change_probe::Projection::default();
    let mut right_projection = crate::afoot::change_probe::Projection::default();
    left_projection.refresh(&left)?;
    right_projection.refresh(&right)?;
    let activity = left_projection.snapshot();
    anyhow::ensure!(
        activity == right_projection.snapshot() && activity.to_string().contains("j-abcdef"),
        "nonempty identical activity projection"
    );
    let left_slots = slots(&left);
    let right_slots = slots(&right);
    let origin = crate::origin::Origin { name: super::machine(0), key: left.store().book.directory.members[0].key };
    let change = request(kind, round, &left);
    anyhow::ensure!(crate::manage_edit::configuration(&change), "retained configuration");
    let before_left = effects(&left)?;
    let before_right = effects(&right)?;
    anyhow::ensure!(before_left == before_right, "same initial runtime state");
    let (answer_left, ended_left) = crate::manage::change(&left, &origin, &super::account(0), change.clone());
    let (answer_right, ended_right) =
        crate::manage_edit::forced_full(|| crate::manage::change(&right, &origin, &super::account(0), change));
    let answer_left = answer_left.map_err(anyhow::Error::msg)?;
    let answer_right = answer_right.map_err(anyhow::Error::msg)?;
    anyhow::ensure!(serde_json::to_value(answer_left)? == serde_json::to_value(answer_right)?, "same answer");
    crate::updates::change_probe::announce(&left);
    crate::updates::change_probe::announce(&right);
    anyhow::ensure!(
        crate::updates::change_probe::announcements(&left) == crate::updates::change_probe::announcements(&right),
        "same update notices"
    );
    left_projection.refresh(&left)?;
    right_projection.refresh(&right)?;
    anyhow::ensure!(
        left_projection.snapshot() == activity && right_projection.snapshot() == activity,
        "activity projection unchanged"
    );
    anyhow::ensure!(ended_left.is_empty() && ended_right.is_empty(), "no session ended");
    anyhow::ensure!(
        left_slots.iter().zip(slots(&left)).all(|(before, after)| Arc::ptr_eq(before, &after))
            && right_slots.iter().zip(slots(&right)).all(|(before, after)| Arc::ptr_eq(before, &after)),
        "no account slot removed or replaced"
    );
    anyhow::ensure!(effects(&left)? == before_left, "narrow effects empty");
    anyhow::ensure!(effects(&right)? == before_right, "full effects empty");
    anyhow::ensure!(
        serde_json::to_value(&*left.store().book)? == serde_json::to_value(&*right.store().book)?,
        "books equal after round {round}"
    );
    anyhow::ensure!(left.store().hub == right.store().hub, "hub settings equal");
    for receiver in &mut receivers {
        anyhow::ensure!(
            matches!(receiver.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)),
            "no live stream cut"
        );
    }
    let expected_book = serde_json::to_value(&*left.store().book)?;
    let expected_hub = left.store().hub.clone();
    drop(left);
    drop(right);
    drop(entered);
    runtime.shutdown_timeout(std::time::Duration::from_secs(5));
    wait_closed(&left_dir)?;
    wait_closed(&right_dir)?;
    let left = Store::open(&left_dir)?;
    let right = Store::open(&right_dir)?;
    let left_book: Value = serde_json::to_value(&*left.book)?;
    let right_book: Value = serde_json::to_value(&*right.book)?;
    anyhow::ensure!(
        left_book == right_book && left_book == expected_book,
        "all reopened Book fields equal to committed snapshot"
    );
    anyhow::ensure!(
        left.hub == right.hub && left.hub == expected_hub,
        "all reopened hub fields equal to committed snapshot"
    );
    let left_reports: Vec<(String, i64, Value)> = left.shelf.rows("reports")?;
    let right_reports: Vec<(String, i64, Value)> = right.shelf.rows("reports")?;
    anyhow::ensure!(
        !left_reports.is_empty() && left_reports == right_reports,
        "persisted update reports retained and equal"
    );
    anyhow::ensure!(
        super::audit::read(&left, round + 1)? == super::audit::read(&right, round + 1)?,
        "persisted audit records and readers equal except wall-clock time"
    );
    drop(left);
    drop(right);
    wait_closed(&left_dir)?;
    wait_closed(&right_dir)?;
    Ok(())
}

fn slots(hub: &Hub) -> Vec<Arc<crate::account_records::storage::Slot>> {
    hub.store()
        .book
        .directory
        .accounts
        .iter()
        .map(|account| hub.account_records.account(&account.name, account.opened))
        .collect()
}

fn negative_control(root: &Path, base: &Path) -> anyhow::Result<()> {
    let dir = root.join("negative-control");
    copy_dir(base, &dir)?;
    let hub = super::local_hub::open(&dir)?;
    let standings = crate::control::change_probe::seed(&hub);
    let mut receivers = crate::data::change_probe::seed(&hub, &standings);
    crate::updates::change_probe::seed(&hub);
    let calls = crate::control::change_probe::snapshot(&hub).len();
    let streams = crate::data::counts(&hub);
    anyhow::ensure!(calls > 0 && streams.1 > 0 && streams.2 > 0, "nonempty runtime fixture");
    {
        let mut store = hub.edit_store();
        store.moves[0].old = fastctx_types::hub_move::OldPlace::Forwarding;
        store.save_moves()?;
        store.book.directory.members.clear();
        store.book.directory.accounts.clear();
        store.save_book()?;
    }
    hub.changed();
    anyhow::ensure!(crate::control::change_probe::snapshot(&hub).is_empty(), "negative control cuts calls");
    anyhow::ensure!(crate::data::counts(&hub).2 == 0, "negative control removes registrations");
    for receiver in &mut receivers {
        anyhow::ensure!(receiver.try_recv().is_ok(), "negative control cuts live streams");
    }
    anyhow::ensure!(hub.account_records.health()["accounts"] == 0, "negative control drops account slots");
    let updates = crate::updates::change_probe::snapshot(&hub);
    anyhow::ensure!(updates["last"].as_object().expect("history map").is_empty(), "negative control retires reports");
    let store = hub.store();
    anyhow::ensure!(store.hub.package_targets.is_empty(), "negative control retires package targets");
    anyhow::ensure!(
        store.book.forgetting.pending.is_empty() && store.book.account_forgetting.pending.is_empty(),
        "negative control prunes deletions"
    );
    anyhow::ensure!(store.moves.iter().all(|incoming| !incoming.forwarding()), "negative control stops forwarding");
    println!(
        "negative_control calls_cut={calls} live_streams_cut={} registrations_removed={} records_pruned=true updates_retired=true deletions_pruned=true move_stopped=true",
        streams.1, streams.2
    );
    Ok(())
}
