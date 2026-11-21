//! `fastctx data compact`: gives back to the disk the room the asset
//! library's deleted rows leave — copies let go, purges done — which its
//! databases otherwise keep for reuse. They are compacted with this
//! machine's node stopped and its lock held, as a login is moved
//! (`move_login`): each only where its disk has twice its size free besides
//! a gibibyte, its temporary files on that disk; the node's service is put
//! back as it stood. Linux only, in this version.

use clap::Args;

#[derive(Args)]
pub struct CompactArgs {
    /// Stop this machine's node, and with it every conversation on it, while
    /// its library is compacted.
    #[arg(long)]
    yes: bool,
    /// Put the node's service back after a compaction cut short.
    #[arg(long, conflicts_with = "yes")]
    resume: bool,
}

pub fn run(args: CompactArgs) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    return linux::run(args);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = args;
        anyhow::bail!("the library is compacted on Linux in this version of fastctx")
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use anyhow::{Context, bail};
    use fastctx_node::login_moves::ServiceBefore;
    use fastctx_storage::rusqlite;
    use serde::{Deserialize, Serialize};

    use super::CompactArgs;
    use crate::move_login::linux::{Here, hold, outside_fastctx, unfinished};

    /// What a compaction under way keeps, so one cut short puts the service
    /// back as it stood.
    #[derive(Serialize, Deserialize)]
    struct Compacting {
        service: Option<ServiceBefore>,
        at: jiff::Timestamp,
    }

    /// The databases of the asset library.
    const DATABASES: [&str; 2] = ["assets.sqlite3", "asset-bytes.sqlite3"];

    fn record(here: &Here) -> PathBuf {
        here.world.path().join("compacting.json")
    }

    pub fn run(args: CompactArgs) -> anyhow::Result<()> {
        let here = Here::find()?;
        outside_fastctx()?;
        let kept = record(&here);
        if args.resume {
            let Ok(bytes) = fs::read(&kept) else {
                println!("no compaction was cut short");
                return Ok(());
            };
            let compacting: Compacting = serde_json::from_slice(&bytes).context("reading what the compaction kept")?;
            put_back(&here, compacting.service.as_ref())?;
            fs::remove_file(&kept).context("forgetting the compaction")?;
            println!("the node's service is as it was before the compaction");
            return Ok(());
        }
        if kept.exists() {
            bail!("a compaction was cut short; `fastctx data compact --resume` puts the node's service back first");
        }
        if unfinished(&here) {
            bail!("a login's move is unfinished; `fastctx move-login --resume` finishes it first");
        }
        if !args.yes {
            bail!("compacting stops this machine's node, and every conversation on it, for a few minutes: pass --yes");
        }
        let service = match &here.service {
            Some(service) => {
                let status = service.status()?;
                Some(ServiceBefore { name: status.name, enabled: status.enabled, active: status.state.starts_with("active") })
            }
            None => None,
        };
        let compacting = Compacting { service: service.clone(), at: jiff::Timestamp::now() };
        fastctx_platform::publish(&kept, &serde_json::to_vec_pretty(&compacting)?, fastctx_platform::Permissions::Private)?;
        here.own(&kept)?;
        println!("compacting this machine's asset library: its node is stopped until it is done");
        let compacted = (|| -> anyhow::Result<Vec<String>> {
            let _held = hold(&here)?;
            let dir = here.world.assets();
            // SQLite makes its temporary files where it is told: on the
            // library's own disk, not a small /tmp.
            // SAFETY: set before any thread of this command opens a database.
            unsafe { std::env::set_var("SQLITE_TMPDIR", &dir) };
            DATABASES.iter().map(|name| compact(&dir.join(name))).collect()
        })();
        let put = put_back(&here, service.as_ref());
        match &compacted {
            Ok(said) => {
                for line in said {
                    println!("{line}");
                }
            }
            Err(error) => eprintln!("fastctx: compacting: {error:#}"),
        }
        put?;
        fs::remove_file(&kept).context("forgetting the compaction")?;
        compacted.map(|_| ())
    }

    /// Puts the node's service back as it stood.
    fn put_back(here: &Here, before: Option<&ServiceBefore>) -> anyhow::Result<()> {
        if let (Some(service), Some(before)) = (&here.service, before) {
            if before.enabled {
                service.release().context("starting the node's service again")?;
            } else {
                println!("the node's service was held before; it is left held");
            }
        }
        Ok(())
    }

    fn with_log(path: &Path) -> u64 {
        let log = PathBuf::from(format!("{}-wal", path.display()));
        fs::metadata(path).map_or(0, |found| found.len()) + fs::metadata(log).map_or(0, |found| found.len())
    }

    /// Compacts the database at `path`, where its disk has the room: what it
    /// came to, said.
    fn compact(path: &Path) -> anyhow::Result<String> {
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        if !path.exists() {
            return Ok(format!("{name}: not there"));
        }
        let connection = rusqlite::Connection::open(path).with_context(|| format!("opening {name}"))?;
        connection.busy_timeout(Duration::from_secs(10))?;
        let fold = |connection: &rusqlite::Connection| -> anyhow::Result<()> {
            let busy: i64 = connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
            if busy != 0 {
                bail!("{name}'s log could not be folded in: something else has it open");
            }
            Ok(())
        };
        fold(&connection)?;
        let before = with_log(path);
        let free = fastctx_platform::space::available(path)?;
        let needed = before.saturating_mul(2).saturating_add(1 << 30);
        if free < needed {
            return Ok(format!("{name}: left as it is, {} MiB free where {} MiB are needed", free >> 20, needed >> 20));
        }
        connection.execute_batch("VACUUM;").with_context(|| format!("compacting {name}"))?;
        fold(&connection)?;
        drop(connection);
        let after = with_log(path);
        Ok(format!("{name}: {} MiB -> {} MiB", before >> 20, after >> 20))
    }
}
