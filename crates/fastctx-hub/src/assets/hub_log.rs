//! The hub's own log in the asset library (design §14, §28.21): read on as
//! it grows, a look a minute, into the stream `hub` the hub orders in its
//! own scope `h/<hub>/log`, which its super administrators read; each piece
//! of whole lines named by its file and where it begins, its bytes the one
//! object, ordered by when its first line was written. Where the reading
//! stands is kept in the hub's library beside the pieces; a piece written
//! before a stop that kept no standing is not written again. The hub keeps
//! every piece; devices keep about the last week of them.

use super::Assets;
use super::mirror::{apply, writing};
use super::scopes;
use fastctx_content::v2::{HubLog, Kind as _, KindRecord, LEASE};
use fastctx_content::{Record as Item, State};
use fastctx_platform::log_tail::{self, Tail};
use fastctx_types::assets::Readers;
use std::path::Path;
use std::sync::Weak;
use std::time::Duration;

/// How often the log is looked at: as a machine's is (§28.28 M11).
const EVERY: Duration = Duration::from_secs(10);
/// The most a piece holds, and a look writes.
const PIECE: usize = 1 << 20;
const A_LOOK: usize = 16 << 20;
/// Where the reading stands, in the hub's library.
const STANDING: &str = "hub-log/standing";

/// The key of the hub's own scope its log goes in.
pub(crate) fn scope_key(assets: &Assets) -> String {
    format!("{}log", Readers::Hub.key_prefix(assets.id.as_str()))
}

/// Looks at the hub's log every ten seconds for as long as the hub runs.
pub(crate) async fn follow(hub: Weak<crate::Hub>) {
    loop {
        tokio::time::sleep(EVERY).await;
        let Some(running) = hub.upgrade() else { return };
        let (assets, path) = (running.assets.clone(), running.log.path().to_path_buf());
        let looked = fastctx_platform::task::spawn_blocking(move || look(&assets, &path)).await;
        if let Ok(Err(error)) = looked {
            running.log.line(format_args!("writing the hub's log into the asset library: {error}"));
        }
    }
}

/// Writes what is new of the log at `path`, once its scope is described.
fn look(assets: &Assets, path: &Path) -> Result<(), String> {
    let key = scope_key(assets);
    let Some(scope) = scopes::scope(assets, &key)? else { return Ok(()) };
    let library = scopes::library(assets, &scope)?;
    let error = |error: fastctx_content::Error| error.to_string();
    let _writing = writing(&key);
    let mut at: Tail = library
        .checkpoint(STANDING)
        .map_err(error)?
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .map(|value| Tail {
            file: value["file"].as_str().unwrap_or_default().to_string(),
            offset: value["offset"].as_u64().unwrap_or(0),
        })
        .unwrap_or_default();
    // The time the last piece written was ordered by: what the rest of a
    // line longer than a piece is ordered by (AT10).
    let mut time = library
        .checkpoint(STANDING)
        .map_err(error)?
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value["time"].as_i64())
        .unwrap_or(0);
    // The leases keep the new objects until their records are taken.
    let (mut records, mut _leases, mut written) = (Vec::new(), Vec::new(), 0);
    // A piece is named under how many times the hub was put back: a log put
    // back with it and written on again names its pieces anew (§29, 2.2).
    let restored = assets.restores();
    while written < A_LOOK {
        let Some(piece) = log_tail::next(path, &at, PIECE).map_err(|error| error.to_string())? else { break };
        let item = format!("{restored:016x}/{}/{:016x}", piece.file, piece.offset).into_bytes();
        // Written before a stop that kept no standing: on from its end, as
        // long as it was then, not as long as the file is now (AS11).
        if let Some(entry) = library.item(&key, HubLog::STREAM, &item).map_err(error)? {
            let length = entry.objects.first().map_or(0, |object| object.size);
            if length == 0 {
                return Err(format!("the piece of the hub's log at {} is empty", piece.offset));
            }
            written += length as usize;
            time = log_tail::order_time(&entry.order).unwrap_or(time);
            at = Tail { offset: piece.offset + length, file: piece.file };
            continue;
        }
        time = log_tail::piece_time(&piece.bytes, time);
        let order = format!("{time:016x}/{restored:016x}/{}/{:016x}", piece.file, piece.offset).into_bytes();
        let lease = library.put(&mut piece.bytes.as_slice(), "hub-log", LEASE).map_err(error)?;
        // How late its last line is, which says when it is old (N13).
        let last = log_tail::last_time(&piece.bytes).unwrap_or(time).max(time);
        let body = serde_json::to_vec(
            &serde_json::json!({ "file": piece.file, "offset": piece.offset, "length": piece.bytes.len(), "last": last }),
        )
        .map_err(|error| error.to_string())?;
        records.push(KindRecord {
            kind: HubLog::KIND,
            schema: 1,
            record: Item { item, order, state: State::Is { body, objects: vec![lease.object] } },
        });
        _leases.push(lease);
        written += piece.bytes.len();
        at = Tail { offset: piece.offset + piece.bytes.len() as u64, file: piece.file };
    }
    if written == 0 {
        return Ok(());
    }
    apply(assets, &scope, &library, HubLog::STREAM, records)?;
    let standing = serde_json::to_vec(&serde_json::json!({ "file": at.file, "offset": at.offset, "time": time }))
        .map_err(|error| error.to_string())?;
    library.keep_checkpoint(STANDING, standing).map_err(error)
}
