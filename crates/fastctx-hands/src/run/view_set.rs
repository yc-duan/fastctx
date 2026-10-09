//! Closed view identifiers form a sparse, file-backed bit set.
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub(super) struct ViewSet(Option<Arc<Mutex<File>>>);
impl ViewSet {
    pub fn insert(&mut self, id: u64) -> io::Result<()> {
        if self.0.is_none() {
            self.0 = Some(Arc::new(Mutex::new(tempfile::tempfile()?)));
        }
        let mut file = self.0.as_ref().unwrap().lock().map_err(|e| io::Error::other(e.to_string()))?;
        let at = id / 8;
        let mut byte = [0];
        file.seek(SeekFrom::Start(at))?;
        file.read(&mut byte)?;
        byte[0] |= 1 << (id % 8);
        file.seek(SeekFrom::Start(at))?;
        file.write_all(&byte)
    }
    pub fn contains(&self, id: &u64) -> io::Result<bool> {
        let Some(file) = &self.0 else {
            return Ok(false);
        };
        let mut file = file.lock().map_err(|e| io::Error::other(e.to_string()))?;
        file.seek(SeekFrom::Start(id / 8))?;
        let mut byte = [0];
        file.read(&mut byte)?;
        Ok(byte[0] & (1 << (id % 8)) != 0)
    }
}
