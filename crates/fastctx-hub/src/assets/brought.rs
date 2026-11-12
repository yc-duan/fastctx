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
            let rows = statement.query_map(params![], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })
        .map_err(io::Error::other)?;
    for (key, change, prefix, parked, accumulated, machine) in receipts {
        let (Ok(prefix), Ok(accumulated)) = (<[u8; 32]>::try_from(prefix.as_slice()), <[u8; 32]>::try_from(accumulated.as_slice())) else {
            continue;
        };
        let head = Head { scope: key.clone(), change: change as u64, prefix, parked: parked as u64, accumulated };
        let Some(scope) = scopes::scope(assets, &key).map_err(io::Error::other)? else { continue };
        let library = scopes::library(assets, &scope).map_err(io::Error::other)?;
        if !library.holds(&head).map_err(io::Error::other)? && !assets.closed_receipt(&head)? {
            return Ok(Some(format!("{machine} holds the hub's receipt of {key} that the hub's history no longer holds")));
        }
    }
    Ok(None)
}
