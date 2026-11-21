//! Who uses a login's directory now, as Linux tells it in `/proc`: a
//! process holding a file in it open or mapped, working or rooted in it, run
//! from it, or started with it as its agent's directory; for a login an
//! agent finds by default, any of that agent's processes of the login's user
//! started without naming another. A process of the login's user whose state
//! cannot be read counts as one. Nothing here proves no process will use the
//! directory a moment later: the move looks again before the original is set
//! aside, and checks the original against what was copied before it is
//! deleted.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A process using the directory.
pub struct Occupant {
    pub pid: u32,
    /// When it started, in clock ticks since the machine booted: with its
    /// id, which process it is; an id taken again since is another's.
    started: u64,
    command: Vec<String>,
    why: String,
    /// The agent itself, or a process it started — what `--stop-agents`
    /// stops — and not what this command runs in.
    pub agent: bool,
}

impl Occupant {
    pub fn said(&self) -> String {
        let command = if self.command.is_empty() { "?".to_string() } else { self.command.join(" ") };
        format!("{} ({command}): {}", self.pid, self.why)
    }
}

struct Process {
    parent: u32,
    started: u64,
    owner: u32,
    command: Vec<String>,
    exe: Option<PathBuf>,
}

/// The parent and the start time of the process `pid`.
pub fn stat(pid: u32) -> Option<(u32, u64)> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The name is in parentheses and may hold anything; the fields follow it:
    // the state, the parent, ... the start time 19 fields on.
    let (_, rest) = stat.rsplit_once(')')?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    Some((fields.get(1)?.parse().ok()?, fields.get(19)?.parse().ok()?))
}

/// The command line of the process `pid`, word by word.
pub fn command(pid: u32) -> Vec<String> {
    fs::read(format!("/proc/{pid}/cmdline"))
        .map(|bytes| {
            bytes.split(|byte| *byte == 0).filter(|part| !part.is_empty()).map(|part| String::from_utf8_lossy(part).into_owned()).collect()
        })
        .unwrap_or_default()
}

/// The program the process `pid` runs, as it was named when it started.
pub fn program(pid: u32) -> Option<PathBuf> {
    let exe = fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let text = exe.to_string_lossy();
    Some(PathBuf::from(text.strip_suffix(" (deleted)").unwrap_or(&text)))
}

fn processes() -> HashMap<u32, Process> {
    let mut found = HashMap::new();
    let Ok(entries) = fs::read_dir("/proc") else { return found };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else { continue };
        let Some((parent, started)) = stat(pid) else { continue };
        let owner = fs::metadata(entry.path()).map(|metadata| metadata.uid()).unwrap_or(u32::MAX);
        found.insert(pid, Process { parent, started, owner, command: command(pid), exe: program(pid) });
    }
    found
}

/// Whether the process runs `agent` itself: its program, or the script its
/// interpreter runs, is the agent's.
fn runs(process: &Process, agent: &str) -> bool {
    let named = |part: &str| {
        let name = Path::new(part).file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
        name == agent || name.starts_with(&format!("{agent}-")) || name.starts_with(&format!("{agent}."))
    };
    let package = match agent {
        "claude" => "@anthropic-ai/claude-code",
        _ => "@openai/codex",
    };
    process.command.iter().take(2).any(|part| named(part))
        || process.command.iter().take(3).any(|part| part.contains(package))
        || process.exe.as_ref().is_some_and(|exe| {
            named(&exe.to_string_lossy()) || exe.to_string_lossy().contains(&format!("/{agent}/versions/"))
        })
}

/// The process `pid` and those it was started by, up to the first.
fn lineage(all: &HashMap<u32, Process>, pid: u32) -> Vec<u32> {
    let mut found = Vec::new();
    let mut at = pid;
    while at > 1 && found.len() < 128 && !found.contains(&at) {
        found.push(at);
        match all.get(&at) {
            Some(process) => at = process.parent,
            None => break,
        }
    }
    found
}

/// Each process using `directory`, said with why; `user` is the login's.
pub fn of(directory: &Path, agent: &str, default: bool, user: u32) -> Vec<Occupant> {
    let me = std::process::id();
    let all = processes();
    let mine: BTreeSet<u32> = lineage(&all, me).into_iter().collect();
    let mut found = Vec::new();
    for (pid, process) in &all {
        if *pid == me {
            continue;
        }
        let at = PathBuf::from(format!("/proc/{pid}"));
        let why = match uses(&at, directory, agent, default && process.owner == user, process) {
            Ok(Some(why)) => why,
            Ok(None) => continue,
            Err(why) if process.owner == user => format!("cannot be looked at: {why}"),
            Err(_) => continue,
        };
        let inside = mine.contains(pid);
        let agent = !inside && lineage(&all, *pid).iter().any(|one| all.get(one).is_some_and(|process| runs(process, agent)));
        let why = if inside { format!("{why}; this command runs inside it") } else { why };
        found.push(Occupant { pid: *pid, started: process.started, command: process.command.clone(), why, agent });
    }
    found.sort_by_key(|occupant| occupant.pid);
    found
}

fn within(path: &Path, directory: &Path) -> bool {
    let text = path.to_string_lossy();
    Path::new(text.strip_suffix(" (deleted)").unwrap_or(&text)).starts_with(directory)
}

fn uses(at: &Path, directory: &Path, agent: &str, default: bool, process: &Process) -> Result<Option<String>, String> {
    for link in ["cwd", "root", "exe"] {
        match fs::read_link(at.join(link)) {
            Ok(path) if within(&path, directory) => return Ok(Some(format!("its {link} is {}", path.display()))),
            Ok(_) => {}
            // Gone, or a kernel thread: nothing of it uses anything.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        }
    }
    let files = fs::read_dir(at.join("fd")).map_err(|error| error.to_string())?;
    for file in files.flatten() {
        if let Ok(path) = fs::read_link(file.path())
            && within(&path, directory)
        {
            return Ok(Some(format!("it holds {} open", path.display())));
        }
    }
    let maps = fs::read_to_string(at.join("maps")).map_err(|error| error.to_string())?;
    for line in maps.lines() {
        if let Some(path) = line.split_whitespace().nth(5).map(PathBuf::from)
            && within(&path, directory)
        {
            return Ok(Some(format!("it maps {}", path.display())));
        }
    }
    let environment = fs::read(at.join("environ")).map_err(|error| error.to_string())?;
    let mut named = false;
    for variable in environment.split(|byte| *byte == 0) {
        let text = String::from_utf8_lossy(variable);
        let Some((name, value)) = text.split_once('=') else { continue };
        if (name != "CLAUDE_CONFIG_DIR" && name != "CODEX_HOME") || value.is_empty() {
            continue;
        }
        if (name == "CLAUDE_CONFIG_DIR") == (agent == "claude") {
            named = true;
        }
        let resolved = fs::canonicalize(value).unwrap_or_else(|_| PathBuf::from(value));
        if within(&resolved, directory) || within(Path::new(value), directory) {
            return Ok(Some(format!("it was started with {name}={value}")));
        }
    }
    // The login its agent opens by default: any of the agent's processes
    // started without naming another.
    if default && !named && runs(process, agent) {
        return Ok(Some(format!("it is {agent}, started without naming another directory than its default one")));
    }
    Ok(None)
}

/// Stops `occupant`: asked to end, then made to after ten seconds, through
/// a handle on the process itself, so that an id another process took
/// meanwhile is never signalled. Where the system gives no such handle,
/// nothing is signalled.
pub fn stop(occupant: &Occupant) -> io::Result<()> {
    // SAFETY: pidfd_open takes an id and flags, and returns a descriptor.
    let opened = unsafe { libc::syscall(libc::SYS_pidfd_open, occupant.pid as libc::pid_t, 0) };
    if opened < 0 {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ESRCH) { Ok(()) } else { Err(error) };
    }
    // SAFETY: the descriptor was just opened, and is owned here alone.
    let handle = unsafe { OwnedFd::from_raw_fd(opened as i32) };
    // The handle is on the process found, not one that took its id since.
    if stat(occupant.pid).map(|(_, started)| started) != Some(occupant.started) {
        return Ok(());
    }
    for (signal, within) in [(libc::SIGTERM, Duration::from_secs(10)), (libc::SIGKILL, Duration::from_secs(5))] {
        // SAFETY: the descriptor is open; no signal information is given.
        let sent = unsafe {
            libc::syscall(libc::SYS_pidfd_send_signal, handle.as_raw_fd(), signal, std::ptr::null::<libc::siginfo_t>(), 0)
        };
        if sent < 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::ESRCH) { Ok(()) } else { Err(error) };
        }
        if ended(&handle, within) {
            return Ok(());
        }
    }
    Err(io::Error::other("it did not end"))
}

/// Whether the process `handle` is on ends within `within`.
fn ended(handle: &OwnedFd, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        let mut poll = libc::pollfd { fd: handle.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let left = deadline.saturating_duration_since(Instant::now()).as_millis().min(i32::MAX as u128) as i32;
        // SAFETY: one pollfd, on an open descriptor.
        let ready = unsafe { libc::poll(&mut poll, 1, left) };
        if ready > 0 {
            return true;
        }
        if ready == 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return false;
        }
    }
}
