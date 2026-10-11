//! Two diagnostic files, each at most ten decimal megabytes.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

pub const FILE_BYTES: u64 = 10_000_000;

pub struct RollingLog {
    path: PathBuf,
    writer: Mutex<Option<File>>,
}

impl RollingLog {
    /// Concurrent service children share one writer and rotation state.
    pub fn shared(path: &Path) -> io::Result<Arc<Self>> {
        static LOGS: OnceLock<Mutex<std::collections::BTreeMap<PathBuf, Weak<RollingLog>>>> = OnceLock::new();
        let mut logs = LOGS.get_or_init(Default::default).lock().expect("never poisoned");
        if let Some(log) = logs.get(path).and_then(Weak::upgrade) {
            return Ok(log);
        }
        let log = Arc::new(Self::open(path)?);
        logs.insert(path.to_path_buf(), Arc::downgrade(&log));
        Ok(log)
    }
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self { path: path.to_path_buf(), writer: Mutex::new(Some(open(path)?)) })
    }

    pub fn write(&self, mut bytes: &[u8]) -> io::Result<()> {
        let mut writer = self.writer.lock().expect("never poisoned");
        while !bytes.is_empty() {
            if writer.is_none() {
                *writer = Some(open(&self.path)?);
            }
            let size = writer.as_ref().expect("opened").metadata()?.len();
            if size >= FILE_BYTES || size + bytes.len() as u64 > FILE_BYTES && size != 0 {
                writer.take();
                let previous = self.path.with_extension("log.1");
                match fs::remove_file(&previous) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                    _ => {}
                }
                if size > FILE_BYTES {
                    let mut prior = File::open(&self.path)?;
                    prior.seek(io::SeekFrom::End(-(FILE_BYTES as i64)))?;
                    let mut kept = OpenOptions::new().create_new(true).write(true).open(previous)?;
                    io::copy(&mut (&mut prior).take(FILE_BYTES), &mut kept)?;
                    drop(prior);
                    fs::remove_file(&self.path)?;
                } else {
                    fs::rename(&self.path, previous)?;
                }
                *writer = Some(open(&self.path)?);
            }
            let written = bytes.len().min(FILE_BYTES as usize);
            writer.as_mut().expect("opened").write_all(&bytes[..written])?;
            bytes = &bytes[written..];
        }
        Ok(())
    }
}

fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}
