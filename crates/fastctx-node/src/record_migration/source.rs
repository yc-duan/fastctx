//! Proofs for families with their own relational writer. Verification reads the
//! original again before a committed completion permits archival.
use super::{
    archive,
    files::{self, Context, FileComplete, FileRow},
    scheduler,
};
use fastctx_storage::rusqlite::OptionalExtension;
use std::{io, path::Path};

pub(crate) fn process(
    context: &Context,
    path: &Path,
    import: impl FnOnce() -> io::Result<()>,
    verify: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    scheduler::checkpoint()?;
    let key = path.to_string_lossy().into_owned();
    let verified = context
        .database
        .read_background_now(|db| {
            Ok(db
                .query_row(
                    "SELECT verified FROM migration_files WHERE family=?1 AND path=?2",
                    (&context.family, &key),
                    |row| row.get::<_, bool>(0),
                )
                .optional()?
                .unwrap_or(false))
        })
        .map_err(io::Error::other)?;
    // Its family archives what it verified once all its sources are read.
    if verified {
        return Ok(());
    }
    let signature = match archive::fingerprint(path) {
        Ok(signature) => signature,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(super::errors::retry(error)),
    };
    let result = import().and_then(|()| verify());
    if result.as_ref().is_err_and(|error| !super::errors::damaged(error)) {
        return result.map_err(super::errors::retry);
    }
    let damage = result.err().map(|error| error.to_string());
    if damage.is_none() && archive::fingerprint(path)? != signature {
        return Err(io::Error::new(io::ErrorKind::WouldBlock, "legacy source changed before relational verification"));
    }
    files::complete(
        context,
        FileComplete {
            row: FileRow { path: key, signature, ordinal: 1, incarnation: String::new() },
            owner: context.conversation.clone(),
            damage,
        },
    )
}
