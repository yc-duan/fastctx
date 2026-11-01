//! Per-file checkpoints isolate malformed inputs and prove original bytes before
//! archival. A parser failure cannot disguise a database or checkpoint failure.
use super::{archive, scheduler};
use crate::records::Source;
use fastctx_platform::disk;
use fastctx_storage::{Database, rusqlite::OptionalExtension};
use std::{
    cell::RefCell,
    io,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub(crate) struct Context {
    pub database: Database,
    pub world: PathBuf,
    pub conversation: String,
    pub family: String,
}
#[derive(Clone)]
pub(crate) struct FileRow {
    pub path: String,
    pub signature: archive::Signature,
    pub ordinal: i64,
    pub incarnation: String,
}
#[derive(Clone)]
pub(crate) struct FileComplete {
    pub row: FileRow,
    pub owner: String,
    pub damage: Option<String>,
}
thread_local! {
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
    static MODIFIED: RefCell<Option<(PathBuf, jiff::Timestamp)>> = const { RefCell::new(None) };
    static VERIFYING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn verifying() -> bool {
    VERIFYING.get()
}

impl Context {
    pub(crate) fn within<T>(&self, work: impl FnOnce() -> T) -> T {
        struct Restore(Option<Context>);
        impl Drop for Restore {
            fn drop(&mut self) {
                CONTEXT.set(self.0.take());
            }
        }
        let _restore = Restore(CONTEXT.replace(Some(self.clone())));
        work()
    }
}
pub(crate) fn modified(path: &Path) -> io::Result<jiff::Timestamp> {
    if let Some(at) =
        MODIFIED.with_borrow(|current| current.as_ref().filter(|(file, _)| file == path).map(|(_, at)| *at))
    {
        return Ok(at);
    }
    jiff::Timestamp::try_from(disk::metadata(path)?.modified()?).map_err(io::Error::other)
}

pub(crate) fn file(
    path: &Path,
    push: &mut dyn FnMut(Source) -> io::Result<()>,
    mut read: impl FnMut(&mut dyn FnMut(Source) -> io::Result<()>) -> io::Result<()>,
) -> io::Result<()> {
    scheduler::checkpoint()?;
    if VERIFYING.get() {
        return read(push);
    }
    let context = CONTEXT
        .with_borrow(|context| context.clone())
        .ok_or_else(|| io::Error::other("legacy file has no migration context"))?;
    let key = path.to_string_lossy().into_owned();
    let prior = context
        .database
        .read_background_now(|db| {
            Ok(db
                .query_row(
                    "SELECT signature,through,verified,conversation,incarnation FROM migration_files WHERE family=?1 AND path=?2",
                    (&context.family, &key),
                    |row| {
                        Ok((
                            row.get::<_, Vec<u8>>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, bool>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()?)
        })
        .map_err(io::Error::other)?;
    if prior.as_ref().is_some_and(|(_, through, verified, _, _)| *verified && *through > 0) {
        return Ok(());
    }
    let signature = match archive::fingerprint(path) {
        Ok(signature) => signature,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(super::errors::retry(error)),
    };
    let incarnation = match prior.as_ref() {
        Some((_, _, _, _, incarnation)) => incarnation.clone(),
        None => {
            let mut bytes = [0u8; 16];
            getrandom::fill(&mut bytes).map_err(io::Error::other)?;
            format!("{:032x}", u128::from_be_bytes(bytes))
        }
    };
    let through = if let Some((bytes, through, _, owner, _)) = prior {
        let old: archive::Signature = serde_json::from_slice(&bytes)?;
        if old != signature {
            return complete(
                &context,
                FileComplete {
                    row: FileRow { path: key, signature, ordinal: through, incarnation: incarnation.clone() },
                    owner,
                    damage: Some("legacy source changed after a committed batch".into()),
                },
            );
        }
        // A development build before row proofs left checkpoints without
        // them: such a file is read again from its start, keeping each row
        // once more, with its proof.
        let proven: i64 = context
            .database
            .read_background_now(|db| {
                Ok(db.query_row(
                    "SELECT COUNT(*) FROM migration_row_proofs WHERE family=?1 AND path=?2 AND ordinal<=?3",
                    (&context.family, &key, through),
                    |row| row.get(0),
                )?)
            })
            .map_err(io::Error::other)?;
        if proven < through { 0 } else { through }
    } else {
        0
    };
    let previous = MODIFIED.replace(Some((path.to_path_buf(), modified(path)?)));
    struct Restore(Option<(PathBuf, jiff::Timestamp)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            MODIFIED.set(self.0.take());
        }
    }
    let _restore = Restore(previous);
    let mut ordinal = 0;
    let mut owner = context.conversation.clone();
    let mut failed = None;
    let result = read(&mut |mut source| {
        if !matches!(source, Source::Row(_)) {
            return push(source);
        }
        ordinal += 1;
        if let Some(scope) = source.conversation() {
            owner = scope.to_string();
        }
        if ordinal <= through {
            return Ok(());
        }
        source.set_file(FileRow {
            path: key.clone(),
            signature: signature.clone(),
            ordinal,
            incarnation: incarnation.clone(),
        });
        match push(source) {
            Ok(()) => Ok(()),
            Err(error) => {
                let returned = io::Error::new(error.kind(), error.to_string());
                failed = Some(error);
                Err(returned)
            }
        }
    });
    let result = if let Some(error) = failed {
        if error.kind() != io::ErrorKind::InvalidData {
            return Err(error);
        }
        Err(error)
    } else {
        result
    };
    if result.as_ref().is_err_and(|error| !super::errors::damaged(error)) {
        return result.map_err(super::errors::retry);
    }
    let mut damage = result.err().map(|error| error.to_string());
    if damage.is_none()
        && let Err(error) = push(Source::Flush)
    {
        if super::errors::damaged(&error) {
            damage = Some(error.to_string());
        } else {
            return Err(super::errors::retry(error));
        }
    }
    if damage.is_none() {
        let previous = VERIFYING.replace(true);
        struct RestoreVerification(bool);
        impl Drop for RestoreVerification {
            fn drop(&mut self) {
                VERIFYING.set(self.0);
            }
        }
        let _verifying = RestoreVerification(previous);
        let mut checked = 0;
        let mut failed = None;
        let result = read(&mut |mut source| {
            let Source::Row(_) = &source else {
                return Ok(());
            };
            checked += 1;
            source.set_file(FileRow {
                path: key.clone(),
                signature: signature.clone(),
                ordinal: checked,
                incarnation: incarnation.clone(),
            });
            let Source::Row(row) = source else { unreachable!() };
            match push(Source::Verify(row)) {
                Ok(()) => Ok(()),
                Err(error) => {
                    let returned = io::Error::new(error.kind(), error.to_string());
                    failed = Some(error);
                    Err(returned)
                }
            }
        });
        let result = if let Some(error) = failed { Err(error) } else { result };
        if result.as_ref().is_err_and(|error| !super::errors::damaged(error)) {
            return result.map_err(super::errors::retry);
        }
        damage = result.err().map(|error| error.to_string());
        if damage.is_none() && checked != ordinal {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "the legacy source changed its row count when reread",
            ));
        }
    }
    if damage.is_none() && archive::fingerprint(path)? != signature {
        return Err(io::Error::new(io::ErrorKind::WouldBlock, "a legacy source changed while it was decoded"));
    }
    let mut complete = FileComplete { row: FileRow { path: key, signature, ordinal, incarnation }, owner, damage };
    match push(Source::File(complete.clone())) {
        Err(error) if error.kind() == io::ErrorKind::InvalidData && !storage_error(&error) => {
            complete.damage = Some(error.to_string());
            push(Source::File(complete))
        }
        result => result,
    }
}

pub(crate) fn complete(context: &Context, file: FileComplete) -> io::Result<()> {
    if file.damage.is_some() {
        crate::record_database::reject_origin(&context.world, &file.owner, Path::new(&file.row.path))?;
    }
    let family = context.family.clone();
    let signature = serde_json::to_vec(&file.row.signature)?;
    context.database.write_now_at(scheduler::priority(), signature.len() + file.row.path.len(), move |db| {
        db.execute("INSERT INTO migration_files(conversation,family,path,signature,through,verified,damage,incarnation) VALUES(?1,?2,?3,?4,?5,1,?6,?7)
            ON CONFLICT(family,path) DO UPDATE SET conversation=excluded.conversation,signature=excluded.signature,through=excluded.through,verified=1,damage=excluded.damage,incarnation=excluded.incarnation",
            (&file.owner, family, file.row.path, signature, file.row.ordinal, file.damage, file.row.incarnation))?;
        Ok(())
    }).map_err(io::Error::other)
}

/// Files shared by old fields stay readable until every decoder has finished.
pub(crate) fn archive_ready(context: &Context) -> io::Result<()> {
    let mut after = String::new();
    loop {
        let files = context.database.read_background_now(|db| {
            Ok(db.prepare("SELECT path,signature,conversation,damage FROM migration_files WHERE family=?1 AND path>?2 AND verified=1 AND archived=0 ORDER BY path LIMIT 32")?
                .query_map((&context.family, &after), |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?, row.get::<_, String>(2)?, row.get::<_, Option<String>>(3)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?)
        }).map_err(io::Error::other)?;
        let Some(last) = files.last() else { break };
        after = last.0.clone();
        for (path, signature, owner, damage) in files {
            let signature: archive::Signature = serde_json::from_slice(&signature)?;
            let dependencies = dependencies(&path, &context.family);
            let ready = context
                .database
                .read_background_now(|db| {
                    for family in &dependencies {
                        let verified: bool = db.query_row(
                            "SELECT EXISTS(SELECT 1 FROM migration_files WHERE family=?1 AND path=?2 AND verified=1)",
                            (family, &path),
                            |row| row.get(0),
                        )?;
                        if !verified {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                })
                .map_err(io::Error::other)?;
            if !ready && damage.is_none() {
                continue;
            }
            let directory =
                fastctx_types::layout::ConversationDir::at(&context.world.join("conversations").join(&owner));
            if let Some(reason) = &damage {
                archive::damaged(&context.world, &owner, &context.family, Path::new(&path), reason)?;
            } else {
                let result = crate::conversation_store::with_live_files(&directory, || {
                    archive::verified(&context.world, &owner, Path::new(&path), &signature)
                });
                result.map_err(super::errors::retry)?;
            }
            context
                .database
                .write_now_at(scheduler::priority(), path.len(), move |db| {
                    db.execute(
                        "UPDATE migration_files SET archived=1 WHERE path=?1 AND signature=?2 AND verified=1",
                        (
                            path,
                            serde_json::to_vec(&signature)
                                .map_err(|error| fastctx_storage::Error::new(error.to_string()))?,
                        ),
                    )?;
                    Ok(())
                })
                .map_err(io::Error::other)?;
        }
    }
    Ok(())
}
fn dependencies(path: &str, family: &str) -> Vec<String> {
    let name = Path::new(path).file_name().and_then(|name| name.to_str()).unwrap_or("");
    match name {
        "native-hosts.json" => ["native-processes", "native-lifetime"].map(str::to_string).to_vec(),
        "scratch-locations.json" => ["scratch-known", "scratch-seen", "scratch-state"].map(str::to_string).to_vec(),
        _ if name.ends_with(".exit-receipt") => {
            let prefix = family.rsplit_once('/').map_or("", |(prefix, _)| prefix);
            ["exit-children", "exit-consumed", "exit-frozen"].map(|suffix| format!("{prefix}/{suffix}")).to_vec()
        }
        _ => vec![family.to_string()],
    }
}

pub(super) fn storage_error(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|inner| {
        inner.is::<fastctx_storage::Error>() || inner.downcast_ref::<io::Error>().is_some_and(storage_error)
    })
}
pub(crate) fn was_seen(path: &Path) -> io::Result<bool> {
    let context = CONTEXT
        .with_borrow(|context| context.clone())
        .ok_or_else(|| io::Error::other("legacy source has no context"))?;
    context
        .database
        .read_background_now(|db| {
            Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM migration_files WHERE family=?1 AND path=?2)",
                (&context.family, path.to_string_lossy().as_ref()),
                |row| row.get(0),
            )?)
        })
        .map_err(io::Error::other)
}
