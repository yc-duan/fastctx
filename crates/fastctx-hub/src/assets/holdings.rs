//! What a machine may hold (design §6.6): the scopes it may hold, listed a
//! page at a time in the order they were described, each with the hub's word
//! on its descriptor; the machine installs those it lacks. A page looks at a
//! bounded number of scopes, so a machine that holds few of many never makes
//! one answer go through them all.

use super::Assets;
use super::scopes;
use fastctx_net::directory_index::Indexed;
use fastctx_storage::rusqlite::params;
use fastctx_types::MachineName;
use fastctx_types::assets::{Held, HoldingsAt, Reply};

/// Scopes one answer looks at, and gives, at most.
const LOOKED_AT_ONCE: i64 = 1024;
const GIVEN_AT_ONCE: usize = 256;

/// The scopes `machine` may hold described after `after`, judged by the
/// directory as it stands. Call on a thread that may block.
pub(crate) fn list(assets: &Assets, directory: &Indexed<'_>, machine: &MachineName, after: Option<HoldingsAt>) -> Result<Reply, String> {
    let (seq, key) = after.map_or((-1, String::new()), |at| (at.seq as i64, at.scope));
    let rows: Vec<(String, i64)> = assets
        .control
        .read_now(move |c| {
            let mut statement = c.prepare_cached("SELECT key, seq FROM asset_scopes WHERE (seq, key) > (?1, ?2) ORDER BY seq, key LIMIT ?3")?;
            let rows = statement.query_map(params![seq, key, LOOKED_AT_ONCE], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        })
        .map_err(|error| error.to_string())?;
    let looked = rows.len() as i64;
    let (mut scopes, mut last) = (Vec::new(), None);
    for (key, seq) in rows {
        last = Some(HoldingsAt { seq: seq.max(0) as u64, scope: key.clone() });
        let Some(scope) = scopes::scope(assets, &key)? else { continue };
        if scopes::may_hold(directory, machine, &scope) {
            assets.holds(&key, machine);
            scopes.push(Held { scope: key, descriptor: scope.fact.clone() });
            if scopes.len() >= GIVEN_AT_ONCE {
                break;
            }
        }
    }
    let next = if looked == LOOKED_AT_ONCE || scopes.len() >= GIVEN_AT_ONCE { last } else { None };
    Ok(Reply::Holdings { scopes, next })
}
