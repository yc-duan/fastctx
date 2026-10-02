//! Atomic job records and an exclusive supervisor/reconciliation lock.

use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Starting,
    Running,
    Exited,
    Failed,
    Interrupted,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    /// Native OS PID of bash; on Unix this is also the command's process-group ID.
    pub pid: Option<u32>,
    pub supervisor_pid: u32,
    pub supervisor_identity: String,
    pub command: String,
    pub cwd: PathBuf,
    pub machine: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub output_path: PathBuf,
    pub state: JobState,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    /// None means no failure has been recorded, not that a hook is guaranteed delivered.
    pub hook_error: Option<String>,
}

pub(super) fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC timestamps fit RFC3339")
}

pub(super) fn write(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("record has no parent"))?;
    #[cfg(windows)]
    let mut temporary = super::windows_record::temporary(parent)?;
    #[cfg(not(windows))]
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temporary, value)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    #[cfg(windows)]
    super::windows_record::publish(temporary, path)?;
    #[cfg(not(windows))]
    temporary.persist(path).map_err(|error| error.error)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub(super) fn read(path: &Path) -> io::Result<JobRecord> {
    Ok(serde_json::from_reader(std::io::BufReader::new(
        File::open(path)?,
    ))?)
}

pub(super) fn lock(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))
}

pub(super) fn reserve(directory: &Path) -> io::Result<(String, PathBuf, PathBuf)> {
    fs::create_dir_all(directory.join("out"))?;
    fs::create_dir_all(directory.join("jobs"))?;
    loop {
        let mut entropy = [0_u8; 6];
        getrandom::fill(&mut entropy).map_err(io::Error::other)?;
        let alphabet = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let id: String = std::iter::once('j')
            .chain(std::iter::once('-'))
            .chain(
                entropy
                    .iter()
                    .map(|byte| alphabet[usize::from(*byte) % alphabet.len()] as char),
            )
            .collect();
        let output = directory.join("out").join(format!("{id}.log"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
        {
            Ok(_) => {
                return Ok((
                    id.clone(),
                    directory.join("jobs").join(format!("{id}.json")),
                    output,
                ));
            }
            // An ID collision must never overwrite a previous command's output.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}
