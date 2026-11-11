//! The answer of a run call and the letter that says its job ended come by
//! different ways. After a short job the letter can be here first: it finds
//! no remote-running record to remove, and the answer, arriving after it,
//! would keep one that nothing removes any more. So while a conversation has
//! run calls on their way, the jobs a letter ended are remembered here, and
//! an answer that comes late for one of them keeps no record.

use crate::leases;
use fastctx_types::{JobId, NoticeId, layout::ConversationDir};
use std::io;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

/// The jobs letters ended while calls were on their way: the computer each
/// ran on, and the job.
type Ended = Mutex<Vec<(NoticeId, JobId)>>;

/// Remembered per conversation only while one of its calls is on its way.
static FLIGHTS: LazyLock<leases::Registry<PathBuf, Ended>> = LazyLock::new(Default::default);

/// More than any overlap of calls ends; the oldest go first.
const REMEMBERED: usize = 1024;

/// A run call of a conversation on its way, from before it is sent until
/// its answer is kept.
pub(super) struct Flight(leases::Lease<PathBuf, Ended>);

pub(super) fn begin(directory: &ConversationDir) -> Flight {
    Flight(FLIGHTS.get_or_insert_with(&directory.path().to_path_buf(), Default::default))
}

impl Flight {
    /// Keeps the record of a job the answer left running, with `keep`,
    /// unless a letter already said the job ended. A letter waits for the
    /// record to be kept, or is seen here: neither order leaves one behind.
    pub(super) fn left_running(
        &self,
        computer: &NoticeId,
        job: &JobId,
        keep: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        let ended = self.0.lock().expect("never poisoned");
        if ended.iter().any(|(ran_on, ended)| ran_on == computer && ended == job) {
            return Ok(());
        }
        keep()
    }
}

/// A letter said `job` ended on `computer`: `remove` takes its record away,
/// and the conversation's calls on their way learn of it.
pub(super) fn ended(
    directory: &ConversationDir,
    computer: &NoticeId,
    job: &JobId,
    remove: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let Some(flight) = FLIGHTS.get(&directory.path().to_path_buf()) else { return remove() };
    let mut ended = flight.lock().expect("never poisoned");
    remove()?;
    if ended.len() == REMEMBERED {
        ended.remove(0);
    }
    ended.push((computer.clone(), job.clone()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn directory(name: &str) -> ConversationDir {
        fastctx_types::layout::WorldDir::at(&std::env::temp_dir().join("flights-test"))
            .conversation(&name.parse().expect("a resident id"))
    }

    #[test]
    fn a_letter_before_the_answer_keeps_no_record() {
        let directory = directory("claude-00000001");
        let (computer, job): (NoticeId, JobId) =
            ("letter-0123456789abcdef0123".parse().unwrap(), "j-abc123".parse().unwrap());
        let flight = begin(&directory);
        let removed = Cell::new(false);
        ended(&directory, &computer, &job, || {
            removed.set(true);
            Ok(())
        })
        .unwrap();
        assert!(removed.get());
        let kept = Cell::new(false);
        flight
            .left_running(&computer, &job, || {
                kept.set(true);
                Ok(())
            })
            .unwrap();
        assert!(!kept.get(), "the job had ended before its answer came");
        // Another job of the same call keeps its record.
        let other: JobId = "j-def456".parse().unwrap();
        flight
            .left_running(&computer, &other, || {
                kept.set(true);
                Ok(())
            })
            .unwrap();
        assert!(kept.get());
    }

    #[test]
    fn nothing_is_remembered_once_no_call_is_on_its_way() {
        let directory = directory("claude-00000002");
        let (computer, job): (NoticeId, JobId) =
            ("letter-0123456789abcdef0123".parse().unwrap(), "j-abc123".parse().unwrap());
        drop(begin(&directory));
        ended(&directory, &computer, &job, || Ok(())).unwrap();
        let kept = Cell::new(false);
        begin(&directory)
            .left_running(&computer, &job, || {
                kept.set(true);
                Ok(())
            })
            .unwrap();
        assert!(kept.get(), "a later call's job of the same name is another job");
    }
}
