//! Small layout metadata is loaded independently for each conversation. A bad
//! mapping never partly installs aliases; filesystem failures stay retryable.
use super::{
    PLAN, Plan,
    records::{GENERATION, ORIGINS, REJECTIONS},
};
use crate::record_migration::scheduler;
use fastctx_platform::{Permissions, disk, publish};
use fastctx_storage::Database;
use fastctx_types::layout::{ConversationDir, WorldDir};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

#[derive(Default)]
struct Mappings {
    aliases: BTreeMap<PathBuf, PathBuf>,
    routes: BTreeMap<PathBuf, PathBuf>,
    origins: BTreeMap<PathBuf, PathBuf>,
    generations: HashMap<String, String>,
    rejected: BTreeSet<PathBuf>,
}
impl Mappings {
    fn extend(&mut self, other: Self) {
        self.aliases.extend(other.aliases);
        self.routes.extend(other.routes);
        self.origins.extend(other.origins);
        self.generations.extend(other.generations);
        self.rejected.extend(other.rejected);
    }
}

#[derive(Default)]
pub(crate) struct Layout {
    mappings: RwLock<Mappings>,
    pending: Mutex<HashMap<String, bool>>,
}
struct Fault {
    path: PathBuf,
    error: io::Error,
    damaged: bool,
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, Fault> {
    match disk::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Fault { path: path.into(), error, damaged: false }),
    }
}
fn json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, Fault> {
    read(path)?
        .map(|bytes| {
            serde_json::from_slice(&bytes).map_err(|error| Fault {
                path: path.into(),
                error: io::Error::new(io::ErrorKind::InvalidData, error),
                damaged: true,
            })
        })
        .transpose()
}
fn relative(path: &Path, source: &Path) -> Result<(), Fault> {
    if path.is_absolute() || path.components().any(|part| matches!(part, std::path::Component::ParentDir)) {
        return Err(Fault {
            path: source.into(),
            error: io::Error::new(io::ErrorKind::InvalidData, "invalid gathered queue path"),
            damaged: true,
        });
    }
    Ok(())
}
fn load_room(directory: &ConversationDir, name: &str) -> Result<Mappings, Fault> {
    let mut maps = Mappings {
        aliases: json::<BTreeMap<PathBuf, PathBuf>>(&directory.aliases())?.unwrap_or_default(),
        ..Default::default()
    };
    if let Some(plan) = json::<Plan>(&directory.path().join(PLAN))? {
        for (index, source) in plan.sources.iter().enumerate() {
            maps.aliases
                .entry(source.directory.clone())
                .or_insert_with(|| directory.path().join("legacy").join(index.to_string()));
        }
    }
    let file = directory.path().join(ORIGINS);
    for (tail, original) in json::<BTreeMap<PathBuf, PathBuf>>(&file)?.unwrap_or_default() {
        relative(&tail, &file)?;
        let canonical = directory.path().join(tail);
        maps.routes.insert(original.clone(), canonical.clone());
        maps.origins.insert(canonical, original);
    }
    let file = directory.path().join(REJECTIONS);
    for tail in json::<BTreeSet<PathBuf>>(&file)?.unwrap_or_default() {
        relative(&tail, &file)?;
        maps.rejected.insert(directory.path().join(tail));
    }
    let file = directory.path().join(GENERATION);
    if let Some(bytes) = read(&file)? {
        let generation = String::from_utf8(bytes).map_err(|error| Fault {
            path: file,
            error: io::Error::new(io::ErrorKind::InvalidData, error),
            damaged: true,
        })?;
        maps.generations.insert(name.to_string(), generation);
    }
    Ok(maps)
}

impl Layout {
    /// Reads only layout metadata. Quarantine and retries wait for the listener.
    pub(crate) fn load(world: &WorldDir) -> io::Result<Self> {
        let mut layout = Self::default();
        for kept in world.kept()? {
            match load_room(&kept.directory, &kept.name) {
                Ok(maps) => layout.mappings.get_mut().expect("new layout").extend(maps),
                Err(fault) => {
                    layout.pending.get_mut().expect("new layout").insert(kept.name, fault.damaged);
                }
            }
        }
        Ok(layout)
    }
    /// No filesystem resolution: live jobs can still write through an old path.
    pub(crate) fn logical(&self, path: &Path) -> PathBuf {
        let maps = self.mappings.read().expect("record layout");
        if let Some(canonical) = maps.routes.get(&path.with_extension(""))
            && !maps.rejected.contains(canonical)
        {
            return extension(canonical, path);
        }
        for from in path.ancestors() {
            if let Some(to) = maps.aliases.get(from) {
                return to.join(path.strip_prefix(from).expect("ancestor path"));
            }
        }
        path.to_path_buf()
    }
    pub(crate) fn original(&self, path: &Path) -> PathBuf {
        let maps = self.mappings.read().expect("record layout");
        if maps.rejected.contains(&path.with_extension("")) {
            return path.to_path_buf();
        }
        maps.origins
            .get(&path.with_extension(""))
            .map_or_else(|| path.to_path_buf(), |original| extension(original, path))
    }
    pub(crate) fn rejected_origin(&self, path: &Path) -> bool {
        self.mappings.read().expect("record layout").rejected.contains(&path.with_extension(""))
    }
    pub(crate) fn reject_origin(&self, world: &Path, owner: &str, path: &Path) -> io::Result<()> {
        let stem = path.with_extension("");
        if path.extension().is_none_or(|extension| extension != "json")
            || !self.mappings.read().expect("record layout").origins.contains_key(&stem)
        {
            return Ok(());
        }
        let room = ConversationDir::at(&world.join("conversations").join(owner));
        crate::conversation_store::with_live_files(&room, || {
            let rejected = room.path().join(REJECTIONS);
            let mut rejections: BTreeSet<PathBuf> = super::read_optional(&rejected)?.unwrap_or_default();
            rejections.insert(stem.strip_prefix(room.path()).map_err(io::Error::other)?.to_path_buf());
            publish(&rejected, &super::json(&rejections)?, Permissions::Private)?;
            let file = room.path().join(ORIGINS);
            let mut origins: BTreeMap<PathBuf, PathBuf> = super::read_optional(&file)?.unwrap_or_default();
            if let Ok(relative) = stem.strip_prefix(room.path()) {
                origins.remove(relative);
            }
            publish(&file, &super::json(&origins)?, Permissions::Private)?;
            self.mappings.write().expect("record layout").rejected.insert(stem);
            Ok(())
        })
    }
    pub(crate) fn ensure(self: &Arc<Self>, world: &Path, database: &Database, conversation: &str) -> io::Result<()> {
        let pending = self.pending.lock().expect("pending layout").contains_key(conversation);
        if pending {
            let (layout, root, database, scope) =
                (self.clone(), world.to_path_buf(), database.clone(), conversation.to_string());
            scheduler::request(world, conversation, "layout-metadata", move || layout.retry(&root, &database, &scope))?;
        }
        let generation = self.mappings.read().expect("record layout").generations.get(conversation).cloned();
        let Some(generation) = generation else {
            return Ok(());
        };
        let (database, scope) = (database.clone(), conversation.to_string());
        scheduler::request(world, conversation, &format!("gather/{generation}"), move || {
            super::records::reset(&database, &scope, &generation)
        })
    }
    fn retry(&self, world: &Path, database: &Database, conversation: &str) -> io::Result<()> {
        let directory = ConversationDir::at(&world.join("conversations").join(conversation));
        loop {
            if database
                .read_background_now(|db| crate::conversation_store::deleted(db, conversation))
                .map_err(io::Error::other)?
            {
                self.pending.lock().expect("pending layout").remove(conversation);
                return Ok(());
            }
            match load_room(&directory, conversation) {
                Ok(maps) => {
                    let damaged = self.pending.lock().expect("pending layout").remove(conversation).unwrap_or(false);
                    if !damaged {
                        self.mappings.write().expect("record layout").extend(maps);
                    }
                    return Ok(());
                }
                Err(fault) if fault.damaged => {
                    self.pending.lock().expect("pending layout").insert(conversation.to_string(), true);
                    crate::record_migration::archive::layout_metadata(
                        world,
                        database,
                        conversation,
                        &fault.path,
                        &fault.error.to_string(),
                    )?;
                }
                Err(fault) => {
                    fastctx_platform::readings::counter("migration.source_read_retries").add(1);
                    return Err(fault.error);
                }
            }
        }
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
