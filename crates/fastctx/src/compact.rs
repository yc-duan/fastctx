//! `fastctx data compact`: gives back to the disk the room the asset
//! library's deleted rows leave — copies let go, purges done — which its
//! databases otherwise keep for reuse. They are compacted with this
//! machine's node stopped and its lock held, as a login is moved
//! (`move_login`), one maintenance at a time: each only where its disk has
//! twice its size free besides a gibibyte, its temporary files on that disk;
//! the node's service is put back as it stood. Linux only, in this version.

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
pub(crate) mod linux {
    use std::fs;
    use std::path::PathBuf;

    use anyhow::{Context, bail};
    use fastctx_node::login_moves::ServiceBefore;
    use serde::{Deserialize, Serialize};

    use super::CompactArgs;
    use crate::move_login::linux::{Here, alone, hold, outside_fastctx, put_service_back, there, unfinished};

    /// What a compaction under way keeps, so one cut short puts the service
    /// back as it stood.
    #[derive(Serialize, Deserialize)]
    struct Compacting {
        service: Option<ServiceBefore>,
        at: jiff::Timestamp,
    }

    /// Where a compaction keeps what it does while it goes: there, one was
    /// cut short.
    pub(crate) fn record(here: &Here) -> PathBuf {
        here.world.path().join("compacting.json")
    }

    pub fn run(args: CompactArgs) -> anyhow::Result<()> {
        let here = Here::find()?;
        outside_fastctx()?;
        // A move, a compaction or the putting back of one cut short: one at
        // a time on this World.
        let _alone = alone(&here)?;
        let kept = record(&here);
        if args.resume {
            let bytes = match fs::read(&kept) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    println!("no compaction was cut short");
                    return Ok(());
                }
                Err(error) => return Err(error).context("reading what the compaction kept"),
            };
            let compacting: Compacting = serde_json::from_slice(&bytes).context("reading what the compaction kept")?;
            put_service_back(&here, compacting.service.as_ref())?;
            fs::remove_file(&kept).context("forgetting the compaction")?;
            println!("the compaction cut short is settled: its node's service is put back as said");
            return Ok(());
        }
        if there(&kept)? {
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
            fastctx_node::compact_library(&dir).context("compacting the library")
        })();
        let put = put_service_back(&here, service.as_ref());
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
}
