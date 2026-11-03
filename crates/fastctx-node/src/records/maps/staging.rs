//! Small map revisions publish in one durable commit; larger diffs retain
//! bounded staging transactions and become visible only when complete.
use super::*;
use fastctx_storage::rusqlite::params;

type Change = (String, Option<Vec<u8>>);
const INLINE_ROWS: usize = 16;
const INLINE_BYTES: usize = 32 * 1024;

fn change_bytes((key, value): &Change) -> usize {
    key.len() + value.as_ref().map_or(0, Vec::len) + 128
}

#[allow(clippy::too_many_arguments)]
pub(super) fn stage_now<T: Send + 'static, R: Send + 'static>(
    database: &Database,
    conversation: String,
    name: String,
    values: Values,
    priority: Priority,
    order: Arc<Order>,
    describe: impl FnOnce(Option<&Values>) -> Result<R>,
    install: impl FnOnce(&Rows<'_>, &Staged) -> Result<T> + Send + 'static,
) -> Result<(T, R)> {
    for (key, value) in &values {
        if key.len() > KEY_BYTES || value.len() > VALUE_BYTES {
            return Err(invalid("map scalars exceed their row admission bound"));
        }
    }
    let hash = digest(&values);
    let (base, previous) = database.read_background_now(|db| {
        let tx = db.unchecked_transaction()?;
        let base = tx
            .query_row(
                "SELECT published FROM record_map_heads WHERE conversation=?1 AND name=?2",
                (&conversation, &name),
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);
        let previous = if base == 0 { Values::new() } else { read_revision(&tx, &conversation, &name, base)? };
        tx.commit()?;
        Ok((base, previous))
    })?;
    let description = describe((base != 0).then_some(&previous))?;
    let mut changes: Vec<_> =
        previous.keys().filter(|key| !values.contains_key(*key)).map(|key| (key.clone(), None)).collect();
    changes.extend(
        values
            .into_iter()
            .filter(|(key, value)| previous.get(key) != Some(value))
            .map(|(key, value)| (key, Some(value))),
    );
    drop(previous);
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|error| Error::new(error.to_string()))?;
    let token = format!("{:x}", u128::from_be_bytes(random));
    let pin = Arc::new(Pin(token.clone()));
    LIVE.lock().expect("map pins").insert(token.clone(), Arc::downgrade(&pin));
    let bytes = changes.iter().map(change_bytes).sum::<usize>();
    if changes.len() <= INLINE_ROWS && bytes <= INLINE_BYTES {
        return commit(database, priority, bytes + 1024, order, pin.clone(), move |db| {
            let revision = allocate(db, &conversation, &name, &token, changes.len() as i64, &hash)?;
            let staged = Staged { conversation, name, revision, token, _pin: pin };
            write_changes(db, &staged, changes)?;
            publish(db, &staged, base)?;
            let rows = Rows { connection: db, conversation: &staged.conversation };
            Ok((install(&rows, &staged)?, description))
        });
    }
    let (scope, map, lease, expected) = (conversation.clone(), name.clone(), token.clone(), changes.len() as i64);
    let revision = commit(database, priority, 512, order.clone(), pin.clone(), move |db| {
        allocate(db, &scope, &map, &lease, expected, &hash)
    })?;
    let staged = Staged { conversation, name, revision, token, _pin: pin.clone() };
    let mut changes = changes.into_iter().peekable();
    while changes.peek().is_some() {
        let mut batch = Vec::new();
        let mut bytes = 0;
        while let Some(change) = changes.peek() {
            let size = change_bytes(change);
            if !batch.is_empty() && (batch.len() == 64 || bytes + size > BATCH_BYTES) {
                break;
            }
            bytes += size;
            batch.push(changes.next().expect("peeked map row"));
        }
        let kept = staged.clone();
        commit(database, priority, bytes, order.clone(), pin.clone(), move |db| write_changes(db, &kept, batch))?;
    }
    commit(database, priority, 512, order, pin, move |db| {
        publish(db, &staged, base)?;
        let rows = Rows { connection: db, conversation: &staged.conversation };
        Ok((install(&rows, &staged)?, description))
    })
}

fn allocate(db: &Connection, scope: &str, map: &str, token: &str, expected: i64, hash: &str) -> Result<i64> {
    db.prepare_cached(
        "INSERT OR IGNORE INTO record_map_heads(conversation,name,allocated,published) VALUES(?1,?2,0,0)",
    )?
    .execute((scope, map))?;
    let revision=db.prepare_cached("UPDATE record_map_heads SET allocated=allocated+1 WHERE conversation=?1 AND name=?2 AND allocated<9223372036854775807 RETURNING allocated")?.query_row((scope,map),|row|row.get::<_,i64>(0)).optional()?.ok_or_else(||invalid("map revision counter exhausted"))?;
    db.prepare_cached("INSERT INTO record_map_revisions(conversation,name,revision,state,expected,written,hash) VALUES(?1,?2,?3,0,?4,0,?5)")?.execute((scope,map,revision,expected,hash))?;
    db.prepare_cached("INSERT INTO record_map_pins(conversation,name,identity,revision) VALUES(?1,?2,?3,?4)")?
        .execute((scope, map, token, revision))?;
    Ok(revision)
}

fn write_changes(db: &Connection, staged: &Staged, changes: Vec<Change>) -> Result<()> {
    let count = changes.len() as i64;
    if count == 0 {
        return Ok(());
    }
    for (key, body) in changes {
        db.prepare_cached("INSERT OR IGNORE INTO record_map_keys(conversation,name,identity) VALUES(?1,?2,?3)")?
            .execute((&staged.conversation, &staged.name, &key))?;
        db.prepare_cached(
            "INSERT INTO record_map_entries(conversation,name,identity,revision,body) VALUES(?1,?2,?3,?4,?5)",
        )?
        .execute(params![staged.conversation, staged.name, key, staged.revision, body])?;
    }
    if db.prepare_cached("UPDATE record_map_revisions SET written=written+?4 WHERE conversation=?1 AND name=?2 AND revision=?3 AND state=0")?.execute((&staged.conversation,&staged.name,staged.revision,count))?!=1 {
        return Err(invalid("map staging revision disappeared"));
    }
    Ok(())
}

fn publish(db: &Connection, staged: &Staged, base: i64) -> Result<()> {
    if db.prepare_cached("UPDATE record_map_revisions SET state=2 WHERE conversation=?1 AND name=?2 AND revision=?3 AND state=0 AND written=expected AND EXISTS(SELECT 1 FROM record_map_pins WHERE conversation=?1 AND name=?2 AND identity=?4 AND revision=?3)")?.execute((&staged.conversation,&staged.name,staged.revision,&staged.token))?!=1 {
        return Err(invalid("map staging is incomplete or was collected"));
    }
    if db
        .prepare_cached("UPDATE record_map_heads SET published=?3 WHERE conversation=?1 AND name=?2 AND published=?4")?
        .execute((&staged.conversation, &staged.name, staged.revision, base))?
        != 1
    {
        return Err(invalid("map diff base changed during staging"));
    }
    Ok(())
}
