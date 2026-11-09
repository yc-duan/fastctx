//! Stops a child at real SQLite mutation boundaries, then replays the same input.
//! The parent kills the paused process, so no destructor can complete its write.
use super::*;
use fastctx_storage::guard::{Change, CompositeGuard, Guard};
use std::io::{BufRead, Write};

struct Stop {
    remaining: usize,
}
impl Guard for Stop {
    fn check(&mut self, _: &Change<'_>) -> fastctx_storage::Result<()> {
        if self.remaining > 0 {
            self.remaining -= 1;
        }
        if self.remaining == 0 {
            println!("replica-write-stop");
            std::io::stdout().flush().unwrap();
            loop {
                std::thread::park();
            }
        }
        Ok(())
    }
    fn ended(&mut self, _: bool) {}
    fn committed(&mut self, _: bool) {}
}
pub(crate) fn child(dir: &Path, at: usize) -> Result<()> {
    fastctx_platform::ensure_private_dir(dir)?;
    let db = Database::open_guarded(
        &dir.join("replica.sqlite3"),
        "",
        move |db| Ok(Box::new(CompositeGuard::new(vec![boundary::guard(db)?, Box::new(Stop { remaining: at })]))),
        |db| {
            schema::initialize(db)?;
            db.execute("INSERT INTO sync_scopes(hub,scope,origin,acl_revision,enabled,publication_epoch) VALUES(?1,?2,X'7b7d',?3,1,?3) ON CONFLICT DO NOTHING",params![scope().hub.to_string(),scope().scope.as_str(),1_u64.to_be_bytes().as_slice()])?;
            Ok(())
        },
    )?;
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?;
    runtime.block_on(replay(dir, db))
}
async fn replay(dir: &Path, db: Database) -> Result<()> {
    let blocks = SqliteBlocks::new(db.clone(), dir.join("files"), holder(), policy(false), operation(92), 1 << 24)?;
    let f = Fixture::new()?;
    let receiver = SqliteReplica::new(blocks.clone(), Arc::new(Verifier), f.hub.clone());
    receiver.begin(f.context.clone(), f.begin.clone(), f.view.clone()).await?;
    receiver.apply_changes(f.context.clone(), f.changes.clone()).await?;
    receiver.install_epoch(f.context.clone(), f.epoch.clone()).await?;
    receiver.apply_stream(f.context.clone(), f.stream.clone()).await?;
    fetch(&receiver, &f).await?;
    let timelines = SqliteTimelines::new(blocks.clone());
    settle(&db, &blocks, &timelines).await?;
    let context = AccessContext { principal: Principal::LocalService, scope: scope(), observed_acl: Revision(1) };
    let TimelineResult::Page { page } = timelines
        .timeline(
            context.clone(),
            TimelineRead {
                conversation: f.object.clone(),
                fence: None,
                anchor: TimelineAnchor::Latest,
                max_items: 100,
                max_bytes: 262144,
            },
        )
        .await?
    else {
        return Err("crash replay has no timeline".into());
    };
    assert_eq!(page.items, f.items);
    assert_eq!(source_ingest::count(&db, "SELECT COUNT(*) FROM sync_stream_records")?, 3);
    assert_eq!(source_ingest::count(&db, "SELECT COUNT(*) FROM sync_sources")?, 0);
    assert_eq!(source_ingest::count(&db, "SELECT COUNT(*) FROM sync_asset_pins WHERE phase!='released'")?, 0);
    timelines.release(context, page.fence).await?;
    Ok(())
}
pub(super) async fn run(root: &Path) -> Result<()> {
    let exe = std::env::current_exe()?;
    // Different boundaries cover cursor installation, record adoption and file
    // publication. A resumed attempt uses exactly the same operation identities.
    for at in [1, 7, 19, 41, 79, 127] {
        let dir = root.join(format!("replica-kill-{at}"));
        let mut child = std::process::Command::new(&exe)
            .arg(&dir)
            .arg("replica-child")
            .arg(at.to_string())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()?;
        let output = child.stdout.take().ok_or("child stdout was not piped")?;
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut line = String::new();
            let result = std::io::BufReader::new(output).read_line(&mut line).map(|_| line);
            let _ = send.send(result);
        });
        let line = match receive.recv_timeout(Duration::from_secs(60)) {
            Ok(Ok(line)) => line,
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("child write boundary unavailable: {result:?}").into());
            }
        };
        if line.trim() != "replica-write-stop" {
            let _ = child.kill();
            let status = child.wait()?;
            return Err(format!("child did not reach write {at}: {line:?}, {status}").into());
        }
        child.kill()?;
        assert!(!child.wait()?.success());
        replay(&dir, open(&dir.join("replica.sqlite3"))?).await?;
    }
    println!("replica crash lifecycle: six real process kills inside writer mutations resumed to the same timeline");
    Ok(())
}
