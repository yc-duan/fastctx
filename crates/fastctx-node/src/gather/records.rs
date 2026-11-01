//! Gathering preserves logical record identities and invalidates only completed
//! imports. The generation is published before moving any source into a room.
use super::*;
use crate::record_migration::scheduler;
use fastctx_storage::{Database, rusqlite::OptionalExtension};
use std::{cell::RefCell, collections::HashMap};

const GENERATION: &str = ".record-generation";
pub(super) const ORIGINS: &str = ".queue-origins.json";
pub(super) const REJECTIONS: &str = ".queue-rejections.json";
pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS conversation_record_gathering (
    conversation TEXT PRIMARY KEY, generation TEXT NOT NULL, boundary INTEGER NOT NULL,
    complete INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;";
thread_local! {
    static TOUCHED: RefCell<BTreeSet<PathBuf>> = const { RefCell::new(BTreeSet::new()) };
    static WORLD: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

pub(super) fn begin(world: &WorldDir) {
    WORLD.with_borrow_mut(|current| *current = Some(world.path().to_path_buf()));
    TOUCHED.with_borrow_mut(BTreeSet::clear);
}
pub(super) fn moving(destination: &Path) -> io::Result<()> {
    let Some(room) = WORLD.with_borrow(|world| {
        let conversations = world.as_ref()?.join("conversations");
        let first = destination.strip_prefix(&conversations).ok()?.components().next()?;
        Some(conversations.join(first.as_os_str()))
    }) else {
        return Ok(());
    };
    if TOUCHED.with_borrow(|rooms| rooms.contains(&room)) {
        return Ok(());
    }
    ensure_private_dir(&room)?;
    publish(&room.join(GENERATION), crate::speech::new_id("gather").as_str().as_bytes(), Permissions::Private)?;
    TOUCHED.with_borrow_mut(|rooms| rooms.insert(room.to_path_buf()));
    Ok(())
}

#[derive(Default)]
pub(crate) struct Layout {
    aliases: BTreeMap<PathBuf, PathBuf>,
    routes: BTreeMap<PathBuf, PathBuf>,
    origins: BTreeMap<PathBuf, PathBuf>,
    generations: HashMap<String, String>,
    rejected: std::sync::Mutex<BTreeSet<PathBuf>>,
}
impl Layout {
    /// Startup metadata only; legacy bodies are decoded after the listener opens.
    pub(crate) fn load(world: &WorldDir) -> io::Result<Self> {
        let mut layout = Self::default();
        for kept in world.kept()? {
            let directory = &kept.directory;
            let mut aliases = read_optional::<BTreeMap<PathBuf, PathBuf>>(&directory.aliases())?.unwrap_or_default();
            if let Some(plan) = read_optional::<Plan>(&directory.path().join(PLAN))? {
                for (index, source) in plan.sources.iter().enumerate() {
                    aliases
                        .entry(source.directory.clone())
                        .or_insert_with(|| directory.path().join("legacy").join(index.to_string()));
                }
            }
            layout.aliases.extend(aliases);
            for (relative, original) in
                read_optional::<BTreeMap<PathBuf, PathBuf>>(&directory.path().join(ORIGINS))?.unwrap_or_default()
            {
                if relative.is_absolute()
                    || relative.components().any(|part| matches!(part, std::path::Component::ParentDir))
                {
                    return Err(io::Error::other("invalid gathered queue origin"));
                }
                let canonical = directory.path().join(relative);
                layout.routes.insert(original.clone(), canonical.clone());
                layout.origins.insert(canonical, original);
            }
            for relative in read_optional::<BTreeSet<PathBuf>>(&directory.path().join(REJECTIONS))?.unwrap_or_default()
            {
                layout.rejected.get_mut().expect("new layout").insert(directory.path().join(relative));
            }
            match fastctx_platform::disk::read_to_string(directory.path().join(GENERATION)) {
                Ok(generation) => {
                    layout.generations.insert(kept.name, generation);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(layout)
    }
    /// No filesystem resolution: live jobs can still write through the old path.
    pub(crate) fn logical(&self, path: &Path) -> PathBuf {
        if let Some(canonical) = self.routes.get(&path.with_extension(""))
            && !self.rejected.lock().expect("queue origins").contains(canonical)
        {
            return extension(canonical, path);
        }
        for from in path.ancestors() {
            if let Some(to) = self.aliases.get(from) {
                return to.join(path.strip_prefix(from).expect("ancestor path"));
            }
        }
        path.to_path_buf()
    }
    pub(crate) fn original(&self, path: &Path) -> PathBuf {
        if self.rejected_origin(path) {
            return path.to_path_buf();
        }
        self.origins
            .get(&path.with_extension(""))
            .map_or_else(|| path.to_path_buf(), |original| extension(original, path))
    }
    pub(crate) fn rejected_origin(&self, path: &Path) -> bool {
        self.rejected.lock().expect("queue origins").contains(&path.with_extension(""))
    }
    pub(crate) fn reject_origin(&self, world: &Path, owner: &str, path: &Path) -> io::Result<()> {
        let stem = path.with_extension("");
        if path.extension().is_none_or(|extension| extension != "json") || !self.origins.contains_key(&stem) {
            return Ok(());
        }
        let room = ConversationDir::at(&world.join("conversations").join(owner));
        crate::conversation_store::with_live_files(&room, || {
            let rejected = room.path().join(REJECTIONS);
            let mut rejections: BTreeSet<PathBuf> = read_optional(&rejected)?.unwrap_or_default();
            let relative = stem.strip_prefix(room.path()).map_err(io::Error::other)?;
            rejections.insert(relative.to_path_buf());
            publish(&rejected, &json(&rejections)?, Permissions::Private)?;
            let file = room.path().join(ORIGINS);
            let mut origins: BTreeMap<PathBuf, PathBuf> = read_optional(&file)?.unwrap_or_default();
            if let Ok(relative) = stem.strip_prefix(room.path()) {
                origins.remove(relative);
            }
            publish(&file, &json(&origins)?, Permissions::Private)?;
            self.rejected.lock().expect("queue origins").insert(stem);
            Ok(())
        })
    }
    pub(crate) fn ensure(&self, world: &Path, database: &Database, conversation: &str) -> io::Result<()> {
        let Some(generation) = self.generations.get(conversation) else { return Ok(()) };
        let (database, scope, generation) = (database.clone(), conversation.to_string(), generation.clone());
        scheduler::request(world, conversation, &format!("gather/{generation}"), move || {
            reset(&database, &scope, &generation)
        })
    }
}
fn extension(stem: &Path, path: &Path) -> PathBuf {
    path.extension().map_or_else(
        || stem.to_path_buf(),
        |extension| {
            let mut name = stem.file_name().expect("queue stem").to_os_string();
            name.push(".");
            name.push(extension);
            stem.with_file_name(name)
        },
    )
}
fn reset(database: &Database, conversation: &str, generation: &str) -> io::Result<()> {
    let (scope, token) = (conversation.to_string(), generation.to_string());
    let complete = database.write_now_at(scheduler::priority(), 256, move |db| {
        if crate::conversation_store::deleted(db, &scope)? { return Ok(true); }
        let prior: Option<(String, bool)> = db.query_row("SELECT generation,complete FROM conversation_record_gathering WHERE conversation=?1", [&scope], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
        if let Some((prior, complete)) = prior && prior == token { return Ok(complete); }
        db.execute("INSERT INTO conversation_record_gathering(conversation,generation,boundary) VALUES(?1,?2,COALESCE((SELECT position FROM conversation_record_positions WHERE conversation=?1 AND stream='records'),0)) ON CONFLICT(conversation) DO UPDATE SET generation=excluded.generation,boundary=excluded.boundary,complete=0", (&scope,token))?;
        Ok(false)
    }).map_err(io::Error::other)?;
    if complete {
        return Ok(());
    }
    for statement in [
        "DELETE FROM conversation_record_imports WHERE conversation=?1 AND family IN (SELECT family FROM conversation_record_imports WHERE conversation=?1 LIMIT 128)",
        "DELETE FROM session_sources WHERE conversation=?1 AND identity IN (SELECT identity FROM session_sources WHERE conversation=?1 AND finished=1 LIMIT 128)",
        "DELETE FROM migration_files WHERE conversation=?1 AND (family,path) IN (SELECT family,path FROM migration_files WHERE conversation=?1 AND verified=1 LIMIT 128)",
    ] {
        loop {
            scheduler::checkpoint()?;
            let scope = conversation.to_string();
            let count = database
                .write_now_at(scheduler::priority(), 16384, move |db| Ok(db.execute(statement, [scope])?))
                .map_err(io::Error::other)?;
            if count == 0 {
                break;
            }
        }
    }
    let scope = conversation.to_string();
    database.write_now_at(scheduler::priority(), 256, move |db| {
        db.execute("DELETE FROM session_migration_state WHERE conversation=?1 AND EXISTS(SELECT 1 FROM session_heads WHERE conversation=?1 AND imported=1)",[&scope])?;
        db.execute("UPDATE session_heads SET imported=0 WHERE conversation=?1",[&scope])?;
        db.execute("UPDATE run_imported SET checked=0 WHERE conversation=?1",[&scope])?;
        db.execute("UPDATE conversation_record_gathering SET complete=1 WHERE conversation=?1",[scope])?;
        Ok(())
    }).map_err(io::Error::other)
}
