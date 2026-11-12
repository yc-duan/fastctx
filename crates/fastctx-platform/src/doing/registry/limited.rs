//! Small current wait descriptions, independent of a registry's historical peak.

use super::*;

/// Descriptions may be incomplete, while the caller census can still proceed.
pub struct TaskWaits {
    pub tasks: HashMap<tokio::task::Id, String>,
    pub partial: bool,
}

/// Acquires the registry by the shared deadline and copies only bounded rows.
pub fn task_waits_bounded(deadline: Instant) -> TaskWaits {
    let Some(state) = super::super::snapshot_lock(&STATE, deadline) else {
        return TaskWaits { tasks: HashMap::new(), partial: true };
    };
    let mut result = TaskWaits { tasks: HashMap::new(), partial: false };
    let mut bytes = 0;
    let mut count = 0;
    for entry in state.waiting.values() {
        count += 1;
        if count > 8192 || bytes + entry.name.len() + entry.subject.len() + 32 > 1024 * 1024 {
            result.partial = true;
            break;
        }
        if let Some(Who::Task(task)) = entry.who {
            let (subject, cut) = preview(&entry.subject);
            result.partial |= cut;
            let description = format!("waiting for {}({})", entry.name, subject);
            bytes += description.len();
            result.tasks.insert(task, description);
        }
    }
    for entry in state.outside.values() {
        count += 1;
        if count > 8192 || bytes + entry.what.len() + 16 > 1024 * 1024 {
            result.partial = true;
            break;
        }
        if let Some(task) = entry.task {
            let (what, cut) = preview(&entry.what);
            result.partial |= cut;
            let description = format!("waiting for {}", what);
            bytes += description.len();
            result.tasks.insert(task, description);
        }
    }
    result
}

fn preview(text: &str) -> (&str, bool) {
    let mut end = text.len().min(4096);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], end < text.len())
}
