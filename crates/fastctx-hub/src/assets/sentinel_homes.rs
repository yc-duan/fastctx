//! The machines a conversation's sentinels have lived on (design §29 2.7,
//! freeze spec §六 registry 9): each home a sentinel record the hub orders
//! into the conversation's scope named, kept for good, so a machine whose
//! sentinel moved on before its first wake reached the hub still begins
//! its stream of wakes (`serve::sentinel_runs_gate`; codex-847f57fd #13).

use super::Assets;

/// The table of the homes, in the hub's control database.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sentinel_homes (scope TEXT NOT NULL, home TEXT NOT NULL, PRIMARY KEY (scope, home)) WITHOUT ROWID;
";

/// Keeps that a sentinel of the conversation whose scope is `scope` lives
/// on the machine named `home`.
pub(super) fn record(assets: &Assets, scope: &str, home: &str) -> Result<(), String> {
    let (scope, home) = (scope.to_string(), home.to_string());
    assets
        .control
        .write_now(128, move |c| {
            c.prepare_cached("INSERT OR IGNORE INTO sentinel_homes (scope, home) VALUES (?1, ?2)")?
                .execute(fastctx_storage::rusqlite::params![scope, home])?;
            Ok(())
        })
        .map_err(|error| error.to_string())
}

/// The machines the sentinels of the conversation whose scope is `scope`
/// have lived on, by the names they had then.
pub(super) fn ever(assets: &Assets, scope: &str) -> Result<Vec<String>, String> {
    let scope = scope.to_string();
    assets
        .control
        .read_now(move |c| {
            Ok(c.prepare_cached("SELECT home FROM sentinel_homes WHERE scope = ?1")?
                .query_map([scope], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .map_err(|error| error.to_string())
}
