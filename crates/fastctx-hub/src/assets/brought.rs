//! What machines brought while every asset was sealed, looked at again as
//! the hub starts (design §17, AL01): what showed the hub was put back is
//! on the disk before the seal for its recovery is, so a stop between the
//! two would otherwise open the hub as if nothing had been shown.

use super::{Assets, chain, scopes};
use fastctx_content::v2::Head;
use fastctx_storage::rusqlite::params;
use std::io;

/// Why the hub was put back, if what was brought and kept shows it as an
/// ordinary open would: a chain head its word carried that its chain does
/// not hold, or its receipt of what its libraries no longer hold, neither of
/// a history it set aside.
pub(super) fn put_back(assets: &Assets) -> io::Result<Option<String>> {
    let heads: Vec<(i64, Vec<u8>, String)> = assets
        .control
        .read_now(|c| {
            let mut statement = c.prepare_cached("SELECT seq, head, machine FROM recovery_heads ORDER BY seq DESC")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })
        .map_err(io::Error::other)?;
    for (seq, head, machine) in heads {
        let Ok(head) = <[u8; 32]>::try_from(head.as_slice()) else { continue };
        let seq = seq as u64;
        let kept = assets.control.read_now(move |c| chain::head_at(c, seq)).map_err(io::Error::other)?;
        if kept != Some(head) && !assets.closed_head(seq, &head)? {
            return Ok(Some(format!("{machine} saw the hub's history at {seq} that the hub does not hold")));
        }
    }
    type Brought = (String, i64, Vec<u8>, i64, Vec<u8>, String);
    let receipts: Vec<Brought> = assets
        .control
        .read_now(|c| {
            let mut statement =
                c.prepare_cached("SELECT scope, change, prefix, parked, accumulated, machine FROM recovery_receipts")?;
            let rows = statement.query_map(params![], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
            })?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })
        .map_err(io::Error::other)?;
    for (key, change, prefix, parked, accumulated, machine) in receipts {
        let (Ok(prefix), Ok(accumulated)) =
            (<[u8; 32]>::try_from(prefix.as_slice()), <[u8; 32]>::try_from(accumulated.as_slice()))
        else {
            continue;
        };
        let head = Head { scope: key.clone(), change: change as u64, prefix, parked: parked as u64, accumulated };
        let Some(scope) = scopes::scope(assets, &key).map_err(io::Error::other)? else { continue };
        // A library being deleted holds what the hub let go of as it meant
        // to: its receipts show nothing put back, and it opens for no one
        // (AM01).
        if assets.deleting(&scope.owner)? {
            continue;
        }
        // A library still coming from where the hub was cannot be asked
        // yet: its receipts are kept, and judged at a start after it came.
        if assets.owners.is_coming(&scope.owner) {
            continue;
        }
        let library = scopes::library(assets, &scope).map_err(io::Error::other)?;
        if !library.holds(&head).map_err(io::Error::other)? && !assets.closed_receipt(&head)? {
            return Ok(Some(format!(
                "{machine} holds the hub's receipt of {key} that the hub's history no longer holds"
            )));
        }
    }
    Ok(None)
}

/// Forgets what was brought while the hub was sealed as it started, once
/// the super administrator opened it (AM01): what was judged to show
/// nothing put back is not judged again at every start after. What a machine
/// brought that is not judged yet — its judgment waiting for the seal the
/// opening holds — stays, to be judged at the next start if the hub stops
/// first (AO01).
pub(crate) fn forget(assets: &Assets) -> io::Result<()> {
    assets
        .control
        .write_now(256, |c| {
            c.prepare_cached("DELETE FROM recovery_heads WHERE judged = 1")?.execute([])?;
            c.prepare_cached("DELETE FROM recovery_receipts WHERE judged = 1")?.execute([])?;
            Ok(())
        })
        .map_err(io::Error::other)
}

/// What `machine` brought this time — the head it saw and the receipts it
/// reported — was judged to show nothing put back: those alone, not what
/// another of its sessions brought that waits to be judged (AP09).
pub(super) fn judged(
    assets: &Assets,
    machine: &fastctx_types::MachineName,
    seen: Option<(u64, [u8; 32])>,
    receipts: &[Head],
) {
    let name = machine.to_string();
    let receipts = receipts.to_vec();
    let _ = assets.control.write_now(256 + receipts.len() * 128, move |c| {
        if let Some((seq, head)) = seen {
            c.prepare_cached("UPDATE recovery_heads SET judged = 1 WHERE machine = ?1 AND seq = ?2 AND head = ?3")?
                .execute(params![name, seq as i64, &head[..]])?;
        }
        for head in &receipts {
            c.prepare_cached(
                "UPDATE recovery_receipts SET judged = 1
                 WHERE machine = ?1 AND scope = ?2 AND change = ?3 AND prefix = ?4 AND parked = ?5 AND accumulated = ?6",
            )?
            .execute(params![name, head.scope, head.change as i64, &head.prefix[..], head.parked as i64, &head.accumulated[..]])?;
        }
        Ok(())
    });
}

/// Brings the tables of what was brought up to this release: whether each
/// was judged.
pub(super) fn migrate(c: &fastctx_storage::rusqlite::Connection) -> fastctx_storage::Result<()> {
    for table in ["recovery_heads", "recovery_receipts"] {
        let marked =
            c.prepare(&format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = 'judged'"))?.exists([])?;
        if !marked {
            c.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN judged INTEGER NOT NULL DEFAULT 0;"))?;
        }
    }
    Ok(())
}
