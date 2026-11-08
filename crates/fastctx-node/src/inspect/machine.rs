//! A machine's processes, GPUs, memory and disks, as a look shows them:
//! one fact a line, each line saying what it is about before its number.

use fastctx_types::inspect::Inspected;
use fastctx_types::ipc::Failure;

/// The most processes a look shows.
const PROCESSES: usize = 200;
/// How much of a process's command line a look shows.
const COMMAND: usize = 300;

/// The machine's memory, disks and NVIDIA GPUs.
pub(super) async fn resources() -> Inspected {
    let gpus = crate::stats::gpus().await;
    let (memory, disks) = fastctx_platform::task::blocking(|| {
        (fastctx_platform::system::memory().ok(), fastctx_platform::system::disks().unwrap_or_default())
    })
    .await;
    let mut text = String::new();
    if let Some(memory) = memory {
        text.push_str(&format!("memory used: {} of {}\n", gib(memory.used), gib(memory.total)));
        if memory.swap_total > 0 {
            text.push_str(&format!("swap used: {} of {}\n", gib(memory.swap_used), gib(memory.swap_total)));
        }
    }
    for disk in disks {
        let at = disk.at.to_string_lossy().replace('\\', "/");
        let fs = if disk.fs.is_empty() { String::new() } else { format!(" ({})", disk.fs) };
        text.push_str(&format!("disk {at} free: {} of {}{fs}\n", gib(disk.free), gib(disk.total)));
    }
    for (index, gpu) in gpus.iter().enumerate() {
        text.push_str(&format!(
            "gpu {index} ({}) memory used: {} of {}\n",
            gpu.name,
            gib(gpu.memory_used),
            gib(gpu.memory_total)
        ));
        if let Some(busy) = gpu.busy {
            text.push_str(&format!("gpu {index} ({}) busy: {}%\n", gpu.name, (busy * 100.0).round()));
        }
    }
    Inspected { text, note: gpus.is_empty().then(|| "no NVIDIA GPU".to_string()) }
}

/// The processes running, or those whose name or command line holds
/// `matching`: those holding the most memory first. Command lines are
/// matched and shown with their secrets hidden.
pub(super) fn processes(matching: Option<&str>) -> Result<Inspected, Failure> {
    let running = fastctx_platform::processes::running()
        .map_err(|error| Failure::not_run(format!("listing the processes: {error}")))?;
    let mut running: Vec<_> = running
        .into_iter()
        .map(|process| {
            let command = process.command.as_deref().map(super::redact::command);
            (process, command)
        })
        .collect();
    if let Some(words) = matching {
        let wanted = words.to_lowercase();
        running.retain(|(process, command)| {
            process.name.to_lowercase().contains(&wanted)
                || command.as_ref().is_some_and(|command| command.to_lowercase().contains(&wanted))
        });
        if running.is_empty() {
            return Ok(super::noted(format!("no process holds “{words}”")));
        }
    }
    running.sort_by(|(a, _), (b, _)| b.memory.cmp(&a.memory).then(a.pid.cmp(&b.pid)));
    let mut text = String::new();
    for (process, command) in running.iter().take(PROCESSES) {
        let started = process
            .started
            .and_then(|at| jiff::Timestamp::try_from(at).ok())
            .map(|at| format!("  started {}", moment(at)))
            .unwrap_or_default();
        let memory = process.memory.map(|bytes| format!("  memory {}", gib(bytes))).unwrap_or_default();
        let command = command.as_deref().map(|command| format!("  {}", within(command, COMMAND))).unwrap_or_default();
        text.push_str(&format!("pid {}  {}{started}{memory}{command}\n", process.pid, process.name));
    }
    let note = (running.len() > PROCESSES)
        .then(|| format!("{} processes; the {PROCESSES} holding the most memory are shown", running.len()));
    Ok(Inspected { text, note })
}

/// `line` within `most` characters.
pub(super) fn within(line: &str, most: usize) -> String {
    match line.char_indices().nth(most) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_string(),
    }
}

/// Bytes in gibibytes, to a tenth.
fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

/// A moment to the second, in UTC.
pub(super) fn moment(at: jiff::Timestamp) -> String {
    at.round(jiff::Unit::Second).map_or_else(|_| at.to_string(), |at| at.to_string())
}
