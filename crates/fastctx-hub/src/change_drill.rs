//! Measures a directory edit without a listener or a network connection.

mod audit;
mod equivalence;
mod exercise;
mod forwarding;
mod local_hub;
mod verify;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use fastctx_net::directory::{Account, ActsFor, Assist, Kind, Level, Member, Share};
use fastctx_net::wire::Change;
use fastctx_types::{AccountName, MachineName};
use jiff::Timestamp;

use crate::{Book, Store};

/// Runs the opt-in local benchmark. Each size gets a fresh database under `--root`.
pub fn run() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--equivalence") {
        return equivalence::run();
    }
    let mut args = std::env::args().skip(1);
    let (mut root, mut sizes, mut iterations) = (None, vec![1000usize, 10_000], 1000usize);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().map(PathBuf::from),
            "--sizes" => {
                sizes = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing sizes"))?
                    .split(',')
                    .map(str::parse)
                    .collect::<Result<_, _>>()?
            }
            "--iterations" => iterations = args.next().ok_or_else(|| anyhow::anyhow!("missing iterations"))?.parse()?,
            _ => anyhow::bail!("unknown argument {arg}; use --root PATH [--sizes 1000,10000] [--iterations 1000]"),
        }
    }
    let root = root.ok_or_else(|| anyhow::anyhow!("--root is required"))?;
    anyhow::ensure!(
        iterations > 0 && sizes.iter().all(|size| *size >= 4 && size.is_multiple_of(2)),
        "positive iterations and even sizes >= 4 required"
    );
    fastctx_platform::disk::create_dir_all(&root)?;
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    let _entered = runtime.enter();
    println!(
        "change_drill profile={} iterations={iterations} commit=History (writer acquisition excluded; origin, expiry, authorization, audit, durable commit and snapshot included)",
        if cfg!(debug_assertions) { "debug" } else { "release" }
    );
    for size in sizes {
        measure(&root, size, iterations)?;
    }
    Ok(())
}

fn account(i: usize) -> AccountName {
    format!("a{i}").parse().expect("generated account")
}
fn machine(i: usize) -> MachineName {
    format!("m{i}").parse().expect("generated machine")
}

fn fixture(machines: usize) -> Book {
    let accounts = machines / 2;
    let at = Timestamp::from_second(1_700_000_000).expect("fixture time");
    let mut book = Book::default();
    for i in 0..accounts {
        book.directory.accounts.push(Arc::new(Account {
            name: account(i),
            level: if i == 0 { Level::Super } else { Level::Member },
            locked: false,
            opened: at,
            collab: None,
            history: false,
            profile: None,
            promotion: false,
        }));
    }
    for i in 0..machines {
        let mut key = [0; 32];
        key[..8].copy_from_slice(&(i as u64 + 1).to_le_bytes());
        let owner = account(i % accounts);
        book.directory.members.push(Arc::new(Member {
            name: machine(i),
            was: Vec::new(),
            key: fastctx_net::sign::PublicKey::from_bytes(key),
            kind: Kind::Computer,
            logins: [owner.clone()].into_iter().collect(),
            home: Some(owner),
            managed: false,
            code: None,
            group: None,
            direct: Vec::new(),
            relay: Vec::new(),
            added: at,
            online: false,
            since: None,
            through: None,
            reach: None,
            paths: None,
            strain: None,
            workbench: None,
            settled: None,
        }));
    }
    for i in 0..accounts {
        for offset in [1, 2] {
            book.directory.shares.push(Arc::new(Share {
                machine: machine(i),
                account: account((i + offset) % accounts),
                jobs: None,
            }));
            book.directory.assists.push(Arc::new(Assist {
                machine: machine(i + accounts),
                account: account((i + offset) % accounts),
                agent: "codex".parse().expect("agent"),
                home: None,
                until: None,
                weekly: None,
                daily: None,
                five_hour: None,
                turns: None,
            }));
        }
        book.directory.acting.push(Arc::new(ActsFor {
            machine: machine(i),
            account: account((i + 1) % accounts),
            since: at,
        }));
    }
    book
}

fn measure(root: &Path, machines: usize, iterations: usize) -> anyhow::Result<()> {
    let dir = root.join(format!("machines-{machines}"));
    anyhow::ensure!(!fastctx_platform::disk::exists(&dir)?, "{} already exists; choose a fresh --root", dir.display());
    let book = fixture(machines);
    let target = account(machines / 2 - 1);
    let name = machine(machines - 1);
    let origin = crate::origin::Origin { name: name.clone(), key: book.directory.member(&name).expect("member").key };
    let certificate = fastctx_net::tls::HubCertificate::generate().map_err(anyhow::Error::msg)?;
    let hub = crate::HubFile {
        id: fastctx_net::ids::HubId::from_bytes([42; 8]),
        name: "change-drill".parse().expect("name"),
        public: Vec::new(),
        listen: "127.0.0.1:0".parse().expect("address"),
        self_registration: false,
        dissolved: None,
        updates: Default::default(),
        due: Vec::new(),
        repairs: Vec::new(),
        package_targets: Default::default(),
    };
    Store::create(&dir, &hub, &certificate, &[], &book)?;
    let mut store = Store::open(&dir)?;
    store.enable_runtime()?;
    let catalog = crate::catalog::Catalog::new(store)?;
    let frozen = catalog.read();
    let mut times = Vec::with_capacity(iterations);
    for i in 0..iterations {
        let mut store = catalog.edit();
        let began = Instant::now();
        origin.check(&store.book.directory).map_err(anyhow::Error::msg)?;
        crate::assists::end_due(&mut store, Timestamp::now())?;
        crate::manage::commit(&mut store, &origin, &target, Change::History { on: i % 2 == 0 }, |store, _, change| {
            let Change::History { on } = change else { unreachable!() };
            crate::manage::history(store, &target, on)
        })
        .map_err(anyhow::Error::msg)?;
        times.push(began.elapsed().as_secs_f64() * 1000.0);
        anyhow::ensure!(
            !frozen.book.directory.account(&target).expect("frozen account").history,
            "old snapshots remain immutable"
        );
        drop(store);
        if (i + 1) % 100 == 0 {
            eprintln!("machines={machines} committed={}/{}", i + 1, iterations);
        }
    }
    let snapshot = catalog.read();
    anyhow::ensure!(
        snapshot.book.directory.account(&target).expect("account").history == (iterations % 2 == 1),
        "published value"
    );
    anyhow::ensure!(snapshot.book.machine_log.next == iterations as u64, "audit count");
    let rows: Vec<(String, i64, Arc<Account>)> = snapshot.shelf.rows("accounts")?;
    anyhow::ensure!(
        rows.iter().find(|(_, _, row)| row.name == target).expect("durable account").2.history == (iterations % 2 == 1),
        "durable value"
    );
    let logs: u64 = snapshot.shelf.meta("machine_log.next")?.expect("durable audit sequence");
    anyhow::ensure!(logs == iterations as u64, "durable audit count");
    let records: i64 = snapshot.shelf.0.read_now(|connection| {
        connection.query_row("SELECT count(*) FROM machine_log", [], |row| row.get(0)).map_err(Into::into)
    })?;
    anyhow::ensure!(records == i64::try_from(iterations)?, "every audit record is durable");
    verify::check(&frozen, &snapshot, machines)?;
    let mut copies = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let began = Instant::now();
        let copied = std::hint::black_box((*snapshot.book).clone());
        drop(copied);
        copies.push(began.elapsed().as_secs_f64() * 1000.0);
    }
    copies.sort_by(f64::total_cmp);
    println!(
        "machines={machines} book_copy_p50_ms={:.6} book_copy_p99_ms={:.6}",
        copies[(iterations - 1) / 2],
        copies[(iterations - 1) * 99 / 100]
    );
    times.sort_by(f64::total_cmp);
    println!(
        "machines={machines} accounts={} shares={machines} assists={machines} acting={} samples={iterations} p50_ms={:.6} p99_ms={:.6} max_ms={:.6}",
        machines / 2,
        machines / 2,
        times[(iterations - 1) / 2],
        times[(iterations - 1) * 99 / 100],
        times[iterations - 1]
    );
    exercise::run(&catalog, &origin, machines)?;
    Ok(())
}
