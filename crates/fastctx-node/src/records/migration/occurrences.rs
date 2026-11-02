//! Equal legacy actions can be distinct observations. Their occurrence number
//! commits with the row and source cursor, and survives partial-source retries.
use super::source::LegacyRow;
use fastctx_storage::{
    Error, Result,
    rusqlite::{Connection, OptionalExtension},
};

pub(super) fn assign(db: &Connection, conversation: &str, family: &str, row: &mut LegacyRow) -> Result<()> {
    if !row.counted {
        return Ok(());
    }
    let source = row.file.as_ref().ok_or_else(|| Error::new("a counted legacy row needs its source"))?;
    if source.incarnation.is_empty() {
        return Err(Error::new("a counted legacy row needs a persistent source instance"));
    }
    let counter = format!("occurrence/{}/{}", source.incarnation, row.key);
    let count:i64=db.prepare_cached("INSERT INTO migration_content_keys(conversation,family,hash,occurrences) VALUES(?1,?2,?3,1) ON CONFLICT(conversation,family,hash) DO UPDATE SET occurrences=occurrences+1 WHERE occurrences<9223372036854775807 RETURNING occurrences")?
        .query_row((conversation,family,counter),|row|row.get(0)).optional()?.ok_or_else(||Error::new("legacy occurrence count exhausted"))?;
    row.key = format!("{}-{count:016x}", row.key);
    Ok(())
}
