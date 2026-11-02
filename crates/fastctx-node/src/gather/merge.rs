//! Later files in an already gathered source are merged with the same authority
//! rules. Existing logical job slots stay stable across every restart.
use super::*;

pub(super) fn existing(world: &WorldDir, name: &str, directories: &[PathBuf]) -> io::Result<()> {
    let into = ConversationDir::at(&world.conversations().join(name));
    let mut aliases: BTreeMap<PathBuf, PathBuf> = read_optional(&into.aliases())?.unwrap_or_default();
    let mut next = aliases
        .values()
        .filter_map(|path| path.file_name()?.to_str()?.parse::<usize>().ok())
        .max()
        .map_or(0, |index| index + 1);
    let mut sources = vec![Source { directory: into.path().to_path_buf(), records: into.path().to_path_buf() }];
    for directory in directories {
        let legacy = aliases
            .entry(directory.clone())
            .or_insert_with(|| {
                let path = into.path().join("legacy").join(next.to_string());
                next += 1;
                path
            })
            .clone();
        publish(&into.aliases(), &json(&aliases)?, Permissions::Private)?;
        let index = legacy
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse().ok())
            .ok_or_else(|| io::Error::other("invalid legacy job slot"))?;
        let source = Source { directory: directory.clone(), records: directory.clone() };
        preserve_paths(&directory.join("attachments"), &legacy.join("attachments"))?;
        move_records(&source, &into, index, true)?;
        sources.push(source);
    }
    merge_lines(&sources, &into)?;
    for directory in directories {
        if live_jobs(directory)? {
            continue;
        }
        let legacy = aliases.get(directory).expect("persisted alias");
        for file in JOB_FILES.iter().chain(LAUNCH_FILES) {
            let path = directory.join(file);
            if path.exists() {
                move_one(&path, &legacy.join(file))?;
            }
        }
        for entry in entries(directory)? {
            fastctx_platform::owned_directory::delete(&entry)?;
        }
        remove_dir(directory)?;
        if legacy.exists() {
            fastctx_platform::directory_alias::create(legacy, directory)?;
        }
    }
    Ok(())
}
