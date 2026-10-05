use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ignore::WalkBuilder;
use nucleo_matcher::Utf32String;

use crate::{paths, rules};

pub(crate) struct Entry {
    pub root: usize,
    pub relative: String,
    pub text: Utf32String,
    pub name: usize,
}

pub(crate) struct Name {
    pub text: Utf32String,
    pub lower: String,
}

pub(crate) struct Index {
    pub roots: Vec<PathBuf>,
    pub entries: Vec<Entry>,
    pub names: Vec<Name>,
    pub directories: HashSet<PathBuf>,
    pub rules: BTreeMap<PathBuf, rules::Stamp>,
    pub outer_rules: BTreeMap<PathBuf, rules::Stamp>,
}

impl Index {
    pub fn build(
        roots: Vec<PathBuf>,
        closed: &AtomicBool,
        watch: impl Fn(&Path) -> io::Result<()> + Send + Sync + Clone + 'static,
    ) -> io::Result<Self> {
        let mut index = Self {
            roots,
            entries: Vec::new(),
            names: Vec::new(),
            directories: HashSet::new(),
            rules: BTreeMap::new(),
            outer_rules: rules::global()?,
        };
        let mut seen = HashSet::new();
        let mut names = HashMap::new();
        for (number, root) in index.roots.iter().enumerate() {
            paths::validate_root(root)?;
            for ancestor in root.parent().into_iter().flat_map(Path::ancestors) {
                rules::directory(ancestor, &mut index.outer_rules)?;
            }
            if root.is_dir() {
                rules::directory(root, &mut index.outer_rules)?;
            }
            let filter_root = root.clone();
            let filter_error = Arc::new(Mutex::new(None));
            let error_out = filter_error.clone();
            let captured = Arc::new(Mutex::new(BTreeMap::new()));
            let rules_out = captured.clone();
            let watch = watch.clone();
            let mut builder = WalkBuilder::new(root);
            builder.hidden(false).follow_links(false).filter_entry(move |entry| {
                if paths::excluded(entry.path(), &filter_root, entry.file_type().is_some_and(|kind| kind.is_dir())) {
                    return false;
                }
                if entry.file_type().is_some_and(|kind| kind.is_symlink()) {
                    return false;
                }
                if !entry.file_type().is_some_and(|kind| kind.is_dir()) {
                    return true;
                }
                let validate = || -> io::Result<bool> {
                    let metadata = entry.metadata().map_err(io::Error::other)?;
                    if paths::linked(&metadata) {
                        return Ok(false);
                    }
                    if metadata.is_dir() {
                        watch(entry.path())?;
                        rules::directory(entry.path(), &mut rules_out.lock().expect("rules poisoned"))?;
                    }
                    Ok(true)
                };
                match validate() {
                    Ok(include) => include,
                    Err(error) => {
                        *error_out.lock().expect("filter error poisoned") = Some(error);
                        false
                    }
                }
            });
            for entry in builder.build() {
                if closed.load(Ordering::Acquire) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "file search closed"));
                }
                let entry = entry.map_err(io::Error::other)?;
                if let Some(error) = entry.error() {
                    return Err(io::Error::other(error.to_string()));
                }
                let path = entry.path();
                let Some(kind) = entry.file_type() else { continue };
                if kind.is_dir() {
                    index.directories.insert(path.to_owned());
                } else if kind.is_file() && seen.insert(path.to_owned()) {
                    if paths::linked(&entry.metadata().map_err(io::Error::other)?) {
                        continue;
                    }
                    let relative = if path == root {
                        paths::text(path.file_name().map(Path::new).expect("absolute file has name"))?
                    } else {
                        paths::text(path.strip_prefix(root).expect("walker stays below root"))?
                    };
                    let name = relative.rsplit('/').next().expect("has a name");
                    let name = *names.entry(name.to_owned()).or_insert_with(|| {
                        let number = index.names.len();
                        index.names.push(Name { text: Utf32String::from(name), lower: name.to_lowercase() });
                        number
                    });
                    index.entries.push(Entry {
                        root: number,
                        text: Utf32String::from(relative.as_str()),
                        name,
                        relative,
                    });
                }
            }
            if let Some(error) = filter_error.lock().expect("filter error poisoned").take() {
                return Err(error);
            }
            index.rules.extend(std::mem::take(&mut *captured.lock().expect("rules poisoned")));
        }
        Ok(index)
    }

    pub fn path(&self, entry: &Entry) -> PathBuf {
        let root = &self.roots[entry.root];
        if self.directories.contains(root) { root.join(&entry.relative) } else { root.clone() }
    }

    pub fn policy_changed(&self, path: &Path, checked: &mut HashSet<PathBuf>) -> io::Result<bool> {
        for parent in path.parent().into_iter().flat_map(Path::ancestors) {
            if !checked.insert(parent.to_owned()) {
                continue;
            }
            let mut current = BTreeMap::new();
            rules::directory(parent, &mut current)?;
            for (file, stamp) in current {
                if let Some(old) = self.rules.get(&file).or_else(|| self.outer_rules.get(&file))
                    && stamp != *old
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}
