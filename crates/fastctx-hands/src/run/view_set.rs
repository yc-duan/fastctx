//! Closed view identifiers use one native word, then a sparse file bit set.
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Storage {
    first: AtomicU64,
    file: Mutex<Option<File>>,
}

#[derive(Clone, Default)]
pub(super) struct ViewSet(Option<Arc<Storage>>);
impl ViewSet {
    pub fn insert(&mut self, id: u64) -> io::Result<()> {
        let storage = self.0.get_or_insert_with(|| Arc::new(Storage::default()));
        if id < u64::from(u64::BITS) {
            storage.first.fetch_or(1 << id, Ordering::Relaxed);
            return Ok(());
        }
        let mut slot = storage.file.lock().map_err(|e| io::Error::other(e.to_string()))?;
        if slot.is_none() {
            *slot = Some(tempfile::tempfile()?);
        }
        let file = slot.as_mut().expect("created view index");
        let at = id / 8;
        let mut byte = [0];
        file.seek(SeekFrom::Start(at))?;
        // Past the end of the file no bit of that byte was ever set.
        if file.read(&mut byte)? == 0 {
            byte[0] = 0;
        }
        byte[0] |= 1 << (id % 8);
        file.seek(SeekFrom::Start(at))?;
        file.write_all(&byte)
    }
    pub fn contains(&self, id: &u64) -> io::Result<bool> {
        let Some(storage) = &self.0 else {
            return Ok(false);
        };
        if *id < u64::from(u64::BITS) {
            return Ok(storage.first.load(Ordering::Relaxed) & (1 << id) != 0);
        }
        let mut slot = storage.file.lock().map_err(|e| io::Error::other(e.to_string()))?;
        let Some(file) = slot.as_mut() else {
            return Ok(false);
        };
        file.seek(SeekFrom::Start(id / 8))?;
        let mut byte = [0];
        if file.read(&mut byte)? == 0 {
            return Ok(false);
        }
        Ok(byte[0] & (1 << (id % 8)) != 0)
    }
}
