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
use fastctx_types::assets::{Held, HoldingsAt, Readers, Reply};

/// Scopes one answer looks at, and gives, at most.
const LOOKED_AT_ONCE: i64 = 1024;
const GIVEN_AT_ONCE: usize = 256;

/// The scopes `machine` may hold described after `after`, judged by the
/// directory as it stands, whose `basis` (`shares::basis`) the answer
/// carries: a machine that heard another lists them all again, a scope
/// described long ago being perhaps its to hold now. Call on a thread that
/// may block.
pub(crate) fn list(
    assets: &Assets,
    directory: &Indexed<'_>,
    machine: &MachineName,
    after: Option<HoldingsAt>,
    basis: String,
) -> Result<Reply, String> {
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
    let (mut scopes, mut through) = (Vec::new(), None);
    for (key, seq) in rows {
        let at = HoldingsAt { seq: seq.max(0) as u64, scope: key.clone() };
        if let Some(scope) = scopes::scope(assets, &key)? {
            // Whether the machine may hold it cannot be told now: the whole
            // listing is asked again, not passed by it (AD06).
            if let (Readers::Body { body, .. }, Some(bodies)) = (&scope.descriptor.readers, &scope.bodies)
                && let Err(error) = bodies.members(directory.all, body)
            {
                return Err(format!("who may hold {key} cannot be told now: {error}"));
            }
            if scopes::may_hold_known(directory, machine, &scope).map_err(|error| format!("who may hold {key} cannot be told now: {error}"))? {
                assets.holds(&key, machine);
                let holds = scope
                    .descriptor
                    .classes
                    .iter()
                    .filter(|class| scopes::holds(directory, machine, &scope, &class.prefix))
                    .map(|class| class.prefix.clone())
                    .collect();
                scopes.push(Held { scope: key, descriptor: scope.fact.clone(), holds });
            }
        }
        through = Some(at);
        if scopes.len() >= GIVEN_AT_ONCE {
            break;
        }
    }
    let next = if looked == LOOKED_AT_ONCE || scopes.len() >= GIVEN_AT_ONCE { through.clone() } else { None };
    Ok(Reply::Holdings { scopes, next, through, basis })
}

/// An answer of what `machine` may hold, made by the directory as it stood
/// when the request began, judged again by the directory as it stands now
/// (AD05): a scope it may no longer hold is left out. The basis stays the
/// one the listing went by (AE03); judged again by another directory than
/// that, it is none (AF05): what the listing passed by may be this
/// machine's now, though what decides holding come back to how it was, and
/// a machine told no basis lists all again. Sealed, nothing is given.
pub(crate) fn recheck(assets: &Assets, directory: &Indexed<'_>, now: &str, machine: &MachineName, reply: Reply) -> Result<Reply, String> {
    if let Some(why) = assets.sealed() {
        return Ok(Reply::Sealed { why });
    }
    let Reply::Holdings { scopes, next, through, basis } = reply else { return Ok(reply) };
    let basis = if basis == now { basis } else { String::new() };
    let mut kept = Vec::with_capacity(scopes.len());
    for held in scopes {
        let holds = match scopes::scope(assets, &held.scope)? {
            Some(scope) => scopes::may_hold_known(directory, machine, &scope)
                .map_err(|error| format!("who may hold {} cannot be told now: {error}", held.scope))?,
            None => false,
        };
        if holds {
            kept.push(held);
        } else {
            assets.gone(&held.scope, machine);
        }
    }
    Ok(Reply::Holdings { scopes: kept, next, through, basis })
}
