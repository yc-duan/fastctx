//! One approval spans the node registrations and, later, the hosted hubs.
//! The controller withdraws logins between those phases while hubs still run.

use crate::Request;
use anyhow::Context as _;
use fastctx_types::{ProcessIdentity, uninstall::Event};
use notify::Watcher as _;
use serde::{Deserialize, Serialize};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
enum Message {
    Progress { event: Event },
    Ready,
    SystemDone,
}

pub(crate) struct Session {
    temporary: tempfile::TempDir,
    control: PathBuf,
    final_control: PathBuf,
    output: std::fs::File,
    buffer: Vec<u8>,
    pending: std::collections::VecDeque<Message>,
    changes: tokio::sync::mpsc::UnboundedReceiver<notify::Result<notify::Event>>,
    watcher: notify::RecommendedWatcher,
    process: tokio::task::JoinHandle<anyhow::Result<u32>>,
    exit: Option<anyhow::Result<u32>>,
}

impl Session {
    pub(crate) async fn begin(
        request: &Request,
        events: &mut impl FnMut(Event) -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        let temporary = tempfile::Builder::new().prefix("fastctx-uninstall-admin-").tempdir()?;
        fastctx_platform::private::ensure_administrator_handoff_dir(temporary.path())?;
        let program = temporary.path().join(if cfg!(windows) { "fastctx.exe" } else { "fastctx" });
        std::fs::copy(std::env::current_exe()?, &program)?;
        let input = temporary.path().join("request.json");
        std::fs::write(&input, serde_json::to_vec(request)?)?;
        let path = temporary.path().join("events.jsonl");
        std::fs::File::create(&path)?;
        let output = fastctx_platform::file_read::open(&path)?;
        let control = temporary.path().join("continue.json");
        let final_control = temporary.path().join("complete.json");
        let identity = serde_json::to_string(&fastctx_platform::current_identity()?)?;
        let args = vec![
            "uninstall".into(),
            "system".into(),
            "--request".into(),
            input.into_os_string(),
            "--output".into(),
            path.into_os_string(),
            "--control".into(),
            control.clone().into_os_string(),
            "--controller".into(),
            identity.into(),
        ];
        let (send, changes) = tokio::sync::mpsc::unbounded_channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = send.send(event);
        })?;
        watcher.watch(temporary.path(), notify::RecursiveMode::NonRecursive)?;
        #[cfg(windows)]
        let process = tokio::task::spawn_blocking(move || -> anyhow::Result<u32> {
            if fastctx_platform::elevation::is_elevated()? {
                let mut command = std::process::Command::new(&program);
                command.args(args);
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt as _;
                    command.creation_flags(0x08000000);
                }
                let status = command.status()?;
                return Ok(status.code().context("the system helper ended without a status")? as u32);
            }
            Ok(fastctx_platform::elevation::run_elevated(&program, &args)?)
        });
        #[cfg(unix)]
        let process = {
            let graphical = request.installation.graphical;
            let log = temporary.path().join("helper.log");
            let diagnostics = std::fs::File::create(&log)?;
            tokio::spawn(async move {
                let mut command = if fastctx_platform::elevation::is_elevated()? {
                    let mut command = tokio::process::Command::new(&program);
                    command.args(args);
                    command
                } else if graphical && cfg!(target_os = "macos") {
                    // macOS asks in its own dialog; the helper still writes
                    // what it says of itself to its log.
                    let quote = |word: &std::ffi::OsStr| format!("'{}'", word.to_string_lossy().replace('\'', r"'\''"));
                    let mut line = quote(program.as_os_str());
                    for arg in &args {
                        line.push(' ');
                        line.push_str(&quote(arg));
                    }
                    line.push_str(" > /dev/null 2>> ");
                    line.push_str(&quote(log.as_os_str()));
                    let script = format!(
                        "do shell script \"{}\" with administrator privileges",
                        line.replace('\\', r"\\").replace('"', "\\\"")
                    );
                    let mut command = tokio::process::Command::new("/usr/bin/osascript");
                    command.args(["-e", &script]);
                    command
                } else if graphical {
                    let mut command = tokio::process::Command::new("pkexec");
                    command.arg(&program).args(args);
                    command
                } else {
                    let mut command = tokio::process::Command::new("sudo");
                    command.arg("--").arg(&program).args(args);
                    command
                };
                command.stdout(std::process::Stdio::null()).stderr(diagnostics);
                Ok(command.status().await?.code().context("the system helper ended without a status")? as u32)
            })
        };
        let mut session = Self {
            temporary,
            control,
            final_control,
            output,
            buffer: Vec::new(),
            pending: Default::default(),
            changes,
            watcher,
            process,
            exit: None,
        };
        loop {
            match session.next().await? {
                Some(Message::Progress { event }) => events(event)?,
                Some(Message::Ready) => return Ok(session),
                Some(Message::SystemDone) => anyhow::bail!("the system helper finished before its node phase"),
                None => anyhow::bail!("the system helper ended before disabling the node registrations"),
            }
        }
    }

    fn read(&mut self) -> anyhow::Result<()> {
        self.output.read_to_end(&mut self.buffer)?;
        while let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line = self.buffer.drain(..=end).collect::<Vec<_>>();
            self.pending.push_back(serde_json::from_slice(&line)?);
        }
        Ok(())
    }

    async fn next(&mut self) -> anyhow::Result<Option<Message>> {
        loop {
            self.read()?;
            if let Some(message) = self.pending.pop_front() {
                return Ok(Some(message));
            }
            if self.exit.is_some() {
                return Ok(None);
            }
            tokio::select! {
                change = self.changes.recv() => { change.context("watching system uninstall progress stopped")??; },
                code = &mut self.process => { self.exit = Some(code.map_err(Into::into).and_then(|result| result)); self.read()?; return Ok(self.pending.pop_front()); },
            }
        }
    }

    pub(crate) async fn finish_system(
        &mut self,
        events: &mut impl FnMut(Event) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        std::fs::write(&self.control, b"true\n")?;
        while let Some(message) = self.next().await? {
            match message {
                Message::Progress { event } => events(event)?,
                Message::Ready => anyhow::bail!("the system helper repeated its phase handoff"),
                Message::SystemDone => return Ok(()),
            }
        }
        anyhow::bail!("the system helper exited before its completion handoff");
    }

    pub(crate) async fn complete(
        mut self,
        local_done: bool,
        deferred: bool,
        events: &mut impl FnMut(Event) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        std::fs::write(&self.final_control, if local_done { b"true\n".as_slice() } else { b"false\n".as_slice() })?;
        if deferred && local_done {
            drop(self.watcher);
            drop(self.output);
            let _ = self.temporary.keep();
            return Ok(());
        }
        while let Some(message) = self.next().await? {
            match message {
                Message::Progress { event } => events(event)?,
                _ => anyhow::bail!("the system helper repeated a completion handoff"),
            }
        }
        let code = self.exit.context("system helper finished without an exit status")??;
        if code != 0 {
            // What the helper said of itself, where it says it: its own words name the step.
            let said = std::fs::read_to_string(self.temporary.path().join("helper.log")).unwrap_or_default();
            let said = said.trim();
            let tail: String = said.chars().rev().take(1500).collect::<Vec<_>>().into_iter().rev().collect();
            anyhow::bail!(
                "the system helper exited {code}{}",
                if tail.is_empty() { String::new() } else { format!(": {tail}") }
            );
        }
        anyhow::ensure!(self.buffer.is_empty(), "the system helper left an incomplete progress line");
        drop(self.watcher);
        drop(self.output);
        self.temporary.close()?;
        Ok(())
    }
}

pub async fn system_worker(
    request: &Request,
    output: &Path,
    control: &Path,
    controller: &ProcessIdentity,
) -> anyhow::Result<()> {
    request.installation.validate()?;
    #[cfg(target_os = "linux")]
    if request.installation.application_control.is_some()
        && request.steps.iter().any(|step| matches!(step.action, crate::Action::Package))
    {
        crate::package::remember(request.installation.clone())?;
    }
    let mut failure = None;
    let mut events = |event| {
        if let Err(error) = write(output, &Message::Progress { event }) {
            failure = Some(error);
        }
    };
    crate::execute::run_system_phase(request, true, &mut events);
    if let Some(error) = failure {
        return Err(error);
    }
    write(output, &Message::Ready)?;
    let (send, mut changes) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = send.send(event);
    })?;
    watcher.watch(control.parent().context("system handoff has no parent")?, notify::RecursiveMode::NonRecursive)?;
    loop {
        if control.try_exists()? {
            anyhow::ensure!(std::fs::read(control)? == b"true\n", "invalid system handoff signal");
            break;
        }
        tokio::select! {
            change = changes.recv() => { change.context("watching the system handoff stopped")??; },
            result = fastctx_platform::host_process::exited(controller) => { result?; anyhow::bail!("the original uninstall controller exited before the hub phase"); },
        }
    }
    let mut failure = None;
    crate::execute::run_system_phase(request, false, |event| {
        if let Err(error) = write(output, &Message::Progress { event }) {
            failure = Some(error);
        }
    });
    if let Some(error) = failure {
        return Err(error);
    }
    write(output, &Message::SystemDone)?;
    let final_control = control.parent().context("the system handoff has no parent")?.join("complete.json");
    let completed = loop {
        if final_control.try_exists()? {
            let value = std::fs::read(&final_control)?;
            anyhow::ensure!(value == b"true\n" || value == b"false\n", "invalid kernel completion signal");
            break value == b"true\n";
        }
        tokio::select! {
            change = changes.recv() => { change.context("watching kernel completion stopped")??; },
            result = fastctx_platform::host_process::exited(controller) => { result?; anyhow::bail!("the uninstall controller exited before recording its local result"); },
        }
    };
    #[cfg(target_os = "linux")]
    if completed
        && request.installation.application_control.is_some()
        && request.steps.iter().any(|step| matches!(step.action, crate::Action::Package))
    {
        let result = crate::package::finish(request).await;
        if result.is_ok() {
            crate::package::schedule_cleanup(
                control.parent().context("the system helper has no temporary directory")?,
            )?;
        }
        return result;
    }
    let _ = completed;
    Ok(())
}

/// Each message is appended through a file of its own, closed again at once: macOS reports a change
/// to a file only when its writer closes it, so appends to one kept open never wake the controller.
fn write(output: &Path, message: &Message) -> anyhow::Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    let mut file = std::fs::OpenOptions::new().append(true).open(output)?;
    file.write_all(&line)?;
    file.sync_data()?;
    Ok(())
}
