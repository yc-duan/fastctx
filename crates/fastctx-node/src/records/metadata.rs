//! Explicit node-owned singleton metadata. A logical pathname names only that
//! record, never a directory scan. Growing collections use their own row family.

use super::*;
use std::path::Path;

pub(crate) struct Entry {
    pub directory: ConversationDir,
    pub family: String,
}
pub(crate) fn prepare(path: &Path) -> io::Result<Entry> {
    let (directory, parent, key, _) = crate::record_database::item(path)?;
    let family = format!("metadata/{}/{key}", parent.strip_prefix("items/").expect("logical item family"));
    ensure_file(&directory, &family, path.to_path_buf(), |path, push| match fastctx_platform::disk::read(path) {
        Ok(bytes) => {
            super::metadata_legacy::validate(path, &bytes)?;
            push(Source::new("value".into(), path.to_path_buf(), bytes)?)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    })?;
    Ok(Entry { directory, family })
}
pub(crate) fn raw(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let entry = prepare(path)?;
    super::read(&entry.directory, |rows| rows.get_raw(&entry.family, "value"))
}
pub(crate) fn get<T: DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    raw(path)?
        .map(|bytes| {
            serde_json::from_slice(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {error}", path.display())))
        })
        .transpose()
}
pub(crate) fn exists(path: &Path) -> io::Result<bool> {
    raw(path).map(|row| row.is_some())
}
pub(crate) fn change<T: Send + 'static>(
    path: &Path,
    bytes: usize,
    action: impl FnOnce(&Rows<'_>, &str) -> Result<T> + Send + 'static,
) -> io::Result<T> {
    let entry = prepare(path)?;
    super::write_at(&entry.directory, Priority::Interactive, bytes, move |rows| action(rows, &entry.family))
}
pub(crate) fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let bytes = bytes.to_vec();
    change(path, bytes.len(), move |rows, family| rows.put_raw(family, "value", &bytes))
}
pub(crate) fn keep(path: &Path, value: &impl Serialize) -> io::Result<()> {
    write(path, &serde_json::to_vec(value)?)
}
