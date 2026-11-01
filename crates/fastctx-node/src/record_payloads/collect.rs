//! Collection checks current ownership per deletion batch; unrelated writes
//! cannot invalidate a pass or protect abandoned payloads indefinitely.
use super::*;

/// Only the exclusive node owner runs this collector. A live staging handle is
/// protected even when its caller cancelled while the writer still owns it.
pub(crate) fn collect(database: &Database, scope: &str) -> Result<()> {
    let fenced = database.read_background_now(|db| Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM node_conversation_transfers WHERE name=?1) OR EXISTS(SELECT 1 FROM node_conversation_deletions WHERE name=?1)",
        [scope], |row| row.get::<_, bool>(0))?))?;
    if fenced {
        return Ok(());
    }
    let key = (database.path().to_path_buf(), scope.to_string());
    if !COLLECTING.lock().expect("payload collection").insert(key.clone()) {
        return Ok(());
    }
    let _collecting = Collecting { key, started: std::time::Instant::now() };
    let mut after = String::new();
    loop {
        let leases=database.read_background_now(|db| {
            let mut query=db.prepare_cached("SELECT identity FROM record_payload_leases WHERE conversation=?1 AND identity>?2 ORDER BY identity LIMIT 64")?;
            Ok(query.query_map((scope,&after),|row|row.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?)
        })?;
        let Some(last) = leases.last() else { break };
        after = last.clone();
        let abandoned: Vec<_> = {
            let live = LIVE.lock().expect("payload pins");
            leases.into_iter().filter(|id| live.get(id).is_none_or(|pin| pin.strong_count() == 0)).collect()
        };
        let scope = scope.to_string();
        database.write_now_at(Priority::Background, 4096, move |db| {
            for id in abandoned {
                if db
                    .execute("DELETE FROM record_payload_leases WHERE conversation=?1 AND identity=?2", (&scope, id))?
                    != 0
                {
                    changed(db, &scope)?;
                }
            }
            Ok(())
        })?;
    }
    check_roots(database, scope)?;
    sweep(
        database,
        scope,
        "record_payload_manifests",
        1,
        "NOT EXISTS(SELECT 1 FROM record_payload_roots WHERE conversation=?1 AND hash=?2) AND NOT EXISTS(SELECT 1 FROM record_payload_leases WHERE conversation=?1 AND hash=?2)",
    )?;
    sweep_members(database, scope)?;
    sweep(
        database,
        scope,
        "record_payload_chunks",
        4,
        "NOT EXISTS(SELECT 1 FROM record_payload_members WHERE conversation=?1 AND chunk=?2)",
    )
}
fn check_roots(database: &Database, scope: &str) -> Result<()> {
    let (mut family, mut identity) = (String::new(), String::new());
    loop {
        let roots=database.read_background_now(|db| {
            Ok(db.prepare_cached("SELECT r.family,r.identity,m.complete FROM record_payload_roots r LEFT JOIN record_payload_manifests m ON m.conversation=r.conversation AND m.hash=r.hash WHERE r.conversation=?1 AND (r.family,r.identity)>(?2,?3) ORDER BY r.family,r.identity LIMIT 64")?
                .query_map((scope,&family,&identity),|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,Option<bool>>(2)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)
        })?;
        let Some(last) = roots.last() else { return Ok(()) };
        if roots.iter().any(|root| root.2 != Some(true)) {
            return Err(corrupt("record projection root has no complete manifest"));
        }
        (family, identity) = (last.0.clone(), last.1.clone());
    }
}
fn sweep_members(database: &Database, scope: &str) -> Result<()> {
    let (mut manifest, mut chunk) = (String::new(), String::new());
    loop {
        let members=database.read_background_now(|db| Ok(db.prepare_cached(
            "SELECT manifest,chunk FROM record_payload_members WHERE conversation=?1 AND (manifest,chunk)>(?2,?3) ORDER BY manifest,chunk LIMIT 64")?
            .query_map((scope,&manifest,&chunk),|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?))?;
        let Some(last) = members.last() else { return Ok(()) };
        (manifest, chunk) = (last.0.clone(), last.1.clone());
        let scope = scope.to_string();
        let removed=database.write_now_at(Priority::Background,8192,move |db| {
            let mut delete=db.prepare_cached("DELETE FROM record_payload_members WHERE conversation=?1 AND manifest=?2 AND chunk=?3 AND NOT EXISTS(SELECT 1 FROM record_payload_manifests WHERE conversation=?1 AND hash=?2)")?;
            let mut removed=0;
            for (manifest,chunk) in members {removed+=delete.execute((&scope,manifest,chunk))?;}
            Ok(removed)
        })?;
        fastctx_platform::readings::counter("payload.collected_members").add(removed as u64);
    }
}
fn sweep(database: &Database, scope: &str, table: &'static str, batch: usize, unowned: &'static str) -> Result<()> {
    let mut after = String::new();
    loop {
        let candidates = database.read_background_now(|db| {
            let mut query = db.prepare_cached(&format!(
                "SELECT hash FROM {table} WHERE conversation=?1 AND hash>?2 ORDER BY hash LIMIT 64"
            ))?;
            Ok(query
                .query_map((scope, &after), |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?)
        })?;
        let Some(last) = candidates.last() else { return Ok(()) };
        after = last.clone();
        for candidates in candidates.chunks(batch) {
            let (scope, candidates) = (scope.to_string(), candidates.to_vec());
            let removed = database.write_now_at(Priority::Background, MANIFEST + 256, move |db| {
                let mut delete =
                    db.prepare_cached(&format!("DELETE FROM {table} WHERE conversation=?1 AND hash=?2 AND {unowned}"))?;
                let mut removed = 0;
                for hash in candidates {
                    removed += delete.execute((&scope, hash))?;
                }
                Ok(removed)
            })?;
            let metric = if table == "record_payload_chunks" {
                "payload.collected_chunks"
            } else {
                "payload.collected_manifests"
            };
            fastctx_platform::readings::counter(metric).add(removed as u64);
        }
    }
}

/// Visit scopes in bounded pages; a damaged conversation does not suppress the
/// collection or diagnostics of the following conversations.
pub(crate) fn maintain(database: &Database, mut report: impl FnMut(&str, &Error)) -> Result<()> {
    let mut after = String::new();
    loop {
        let scopes = database.read_background_now(|db| {
            let mut query = db.prepare_cached(
                "SELECT conversation FROM record_payload_versions WHERE conversation>?1 ORDER BY conversation LIMIT 32",
            )?;
            Ok(query.query_map([&after], |row| row.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?)
        })?;
        let Some(last) = scopes.last() else { break };
        after = last.clone();
        for scope in scopes {
            if let Err(error) = collect(database, &scope) {
                report(&scope, &error);
            }
        }
    }
    Ok(())
}
