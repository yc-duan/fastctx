//! Lifecycle health: concurrent FULL writes cannot restart or contaminate a pinned backup.
use fastctx_storage::{
    Database,
    format_upgrade::{self, BackupBasis, Begin, Event, Options, Phase, Service},
    rusqlite::Connection,
};
use fastctx_sync::store::{
    boundary,
    format_upgrade::{SqliteUpgradeJournal, prepare_format_upgrade},
    schema,
};
use std::{
    path::Path,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

pub fn run(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let root = root.join("upgrade");
    fastctx_platform::ensure_private_dir(&root)?;
    let path = root.join("world.sqlite3");
    {
        let mut old = Connection::open(&path)?;
        old.execute_batch("CREATE TABLE old_rows(id INTEGER PRIMARY KEY, value BLOB NOT NULL)")?;
        let transaction = old.transaction()?;
        for _ in 0..128 {
            transaction.execute("INSERT INTO old_rows(value) VALUES(zeroblob(16384))", [])?;
        }
        transaction.commit()?;
    }
    let basis = BackupBasis::observe(&path)?;
    let database = Database::open_guarded(&path, "", boundary::guard, |db| {
        schema::initialize(db)?;
        prepare_format_upgrade(
            db,
            Begin {
                service: Service::World,
                previous_format: Some(3),
                written_by: "lifecycle-drill".into(),
                at_millis: 1,
                backup_basis: basis,
            },
        )?;
        Ok(())
    })?;
    let journal = Arc::new(SqliteUpgradeJournal::new(database.clone(), Service::World)?);
    let (started, copying) = mpsc::channel();
    let (resume, waiting) = mpsc::channel();
    let waiting = Mutex::new(Some(waiting));
    let progress = Arc::new(Mutex::new(Vec::new()));
    let observed = progress.clone();
    let gate = format_upgrade::start(
        &database,
        journal,
        Options {
            observe: Arc::new(move |event| {
                if let Event::Progress { remaining_pages, total_pages } = event {
                    observed.lock().expect("progress").push((remaining_pages, total_pages));
                    if let Some(waiting) = waiting.lock().expect("observer").take() {
                        started.send(()).expect("writer remains alive");
                        waiting
                            .recv_timeout(Duration::from_secs(15))
                            .expect("writer completes while backup holds snapshot");
                    }
                }
            }),
            ..Options::default()
        },
    )?;
    copying.recv_timeout(Duration::from_secs(15))?;
    assert!(!gate.finished(), "migration opened before the backup finished");
    for _ in 0..16 {
        database.write_now(128, |db| {
            db.execute("INSERT INTO old_rows(value) VALUES(x'01')", [])?;
            Ok(())
        })?;
    }
    resume.send(())?;
    let record = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async { tokio::time::timeout(Duration::from_secs(30), gate.wait()).await })??;
    assert_eq!(record.phase, Phase::Complete);
    assert!(gate.finished());
    assert_eq!(gate.wait_blocking()?, record);
    let backup = Connection::open(root.join("upgrade-backups").join(record.file_name.expect("complete backup")))?;
    assert_eq!(backup.query_row("SELECT count(*) FROM old_rows", [], |row| row.get::<_, i64>(0))?, 128);
    assert_eq!(
        database.read_now(|db| Ok(db.query_row("SELECT count(*) FROM old_rows", [], |row| row.get::<_, i64>(0))?))?,
        144
    );
    let progress = progress.lock().expect("progress");
    assert!(progress.len() > 2);
    assert!(
        progress.windows(2).all(|pair| pair[1].0 <= pair[0].0 && pair[1].1 == pair[0].1),
        "a concurrent writer restarted the backup"
    );
    println!(
        "upgrade lifecycle: {} monotonic steps, exact T0, concurrent FULL writes, migration gate opened after standalone verification",
        progress.len()
    );
    Ok(())
}
