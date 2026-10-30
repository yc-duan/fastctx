//! Imports name-based counts once, before the hub can accept a new member.

use super::*;

pub(super) fn migrate(
    dir: &Path,
    directory: &Directory,
    letters: &mut BTreeMap<MachineName, Letters>,
    shelf: &shelf::Shelf,
) -> io::Result<bool> {
    let marker = dir.join(COUNTS).join(".legacy-imported");
    match std::fs::read(&marker) {
        Ok(bytes) if bytes == b"1" => return Ok(false),
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid count migration marker: {}", marker.display()),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut changed = false;
    for member in &directory.members {
        let recorded = letters.keys().any(|name| directory.member(name).is_some_and(|known| known.key == member.key));
        if !recorded && read_taken(dir, &member.name)?.is_some() {
            letters.insert(member.name.clone(), Letters::fresh(None)?);
            changed = true;
        }
    }
    if changed {
        // The import's destination must survive a restart before its first count.
        keep_legacy_letters(shelf, letters)?;
    }
    ensure_private_dir(&dir.join(COUNTS))?;
    for (name, entry) in letters.iter() {
        if read_count(dir, entry.out_epoch)?.is_none()
            && let Some(taken) = read_taken(dir, name)?
        {
            publish(&count_path(dir, entry.out_epoch), &json(&taken)?, Permissions::Private)?;
        }
    }
    publish(&marker, b"1", Permissions::Private)?;
    Ok(changed)
}
