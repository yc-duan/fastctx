//! A command MCP's lifetime and bounded standard streams.

use fastctx_types::{mcp_message, profile::CommandMcp};
use std::ffi::{OsStr, OsString};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub(super) struct Process {
    pub input: mpsc::Sender<Vec<u8>>,
    pub output: mpsc::Receiver<Result<Vec<u8>, String>>,
    pub log: Arc<Mutex<Vec<u8>>>,
    child: Arc<fastctx_platform::Child>,
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

pub(super) fn validate(server: &CommandMcp) -> Result<(), String> {
    if server.command.trim().is_empty() || server.command.contains('\0') {
        return Err("an MCP command must name a program".into());
    }
    if server.args.iter().any(|arg| arg.contains('\0')) {
        return Err("an MCP argument contains NUL".into());
    }
    if server.env.iter().any(|(k, v)| k.is_empty() || k.contains(['=', '\0']) || v.contains('\0')) {
        return Err("an MCP environment entry has an invalid name or value".into());
    }
    if server.cwd.as_ref().is_some_and(|cwd| !Path::new(cwd).is_absolute()) {
        return Err("the MCP working directory must be absolute".into());
    }
    Ok(())
}

fn variable(env: &[(OsString, OsString)], key: &str) -> Option<OsString> {
    env.iter().find(|(k, _)| if cfg!(windows) { k.eq_ignore_ascii_case(key) } else { k == key }).map(|(_, v)| v.clone())
}

fn program(command: &str, env: &[(OsString, OsString)], cwd: &Path) -> Result<PathBuf, String> {
    let extensions = variable(env, "PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
    let extensions: Vec<_> = extensions.to_string_lossy().split(';').map(str::to_owned).collect();
    let candidates = if Path::new(command).components().count() > 1 || Path::new(command).is_absolute() {
        vec![cwd.join(command)]
    } else {
        std::env::split_paths(&variable(env, "PATH").unwrap_or_default()).map(|p| p.join(command)).collect()
    };
    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
        if candidate.extension().is_none() {
            for ext in &extensions {
                let mut name = candidate.as_os_str().to_os_string();
                name.push(ext);
                let path = PathBuf::from(name);
                if path.is_file() {
                    return Ok(path);
                }
            }
        }
    }
    Err(format!("MCP program {command} was not found on the user's PATH"))
}

pub(super) fn start(server: &CommandMcp, fastctx: &Path, root: &Path) -> Result<Process, String> {
    validate(server)?;
    let mut env = fastctx_platform::user_environment().map_err(|e| e.to_string())?;
    let cwd = server
        .cwd
        .as_ref()
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(fastctx_platform::user_home)
        .map_err(|e| e.to_string())?;
    let mut settings = server.env.clone();
    settings.entry("PYTHONUTF8".into()).or_insert_with(|| "1".into());
    let (executable, args): (PathBuf, Vec<OsString>) = if cfg!(windows) {
        for (name, value) in &settings {
            env.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
            env.push((name.into(), value.into()));
        }
        let executable = program(&server.command, &env, &cwd)?;
        if executable.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat")) {
            // Rust's batch launcher quotes for cmd.exe; the helper and its
            // descendants are born inside the node's kill-on-close job.
            env.retain(|(key, _)| !key.eq_ignore_ascii_case("FASTCTX_HOME"));
            env.push(("FASTCTX_HOME".into(), root.as_os_str().into()));
            let mut args = vec!["mcp-child".into(), "--".into(), executable.into_os_string()];
            args.extend(server.args.iter().map(OsString::from));
            (fastctx.to_path_buf(), args)
        } else {
            (executable, server.args.iter().map(OsString::from).collect())
        }
    } else {
        let shell = PathBuf::from(variable(&env, "SHELL").unwrap_or_else(|| "/bin/sh".into()));
        let mut args: Vec<OsString> = vec!["-lc".into(), "exec /usr/bin/env \"$@\"".into(), "fastctx-mcp".into()];
        args.extend(settings.iter().map(|(k, v)| OsString::from(format!("{k}={v}"))));
        args.push(server.command.clone().into());
        args.extend(server.args.iter().map(OsString::from));
        (shell, args)
    };
    let args: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
    let mut child = fastctx_platform::Child::spawn(&executable, &args, &cwd, &env)
        .map_err(|e| format!("starting {}: {e}", server.command))?;
    let mut stdin = child.stdin.take().expect("piped input");
    let stdout = child.stdout.take().expect("piped output");
    let mut stderr = child.stderr.take().expect("piped error");
    let child = Arc::new(child);
    let log = Arc::new(Mutex::new(Vec::new()));
    let (input, mut writes) = mpsc::channel::<Vec<u8>>(1);
    let (read, output) = mpsc::channel(1);
    let errors = read.clone();
    std::thread::spawn(move || {
        while let Some(line) = writes.blocking_recv() {
            if let Err(e) = stdin.write_all(&line).and_then(|()| stdin.write_all(b"\n")).and_then(|()| stdin.flush()) {
                let _ = errors.blocking_send(Err(format!("writing to MCP: {e}")));
                break;
            }
        }
    });
    let tail = log.clone();
    std::thread::spawn(move || {
        let mut stdout = std::io::BufReader::new(stdout);
        loop {
            let mut line = Vec::new();
            match (&mut stdout).take((mcp_message::MAX_LINE + 2) as u64).read_until(b'\n', &mut line) {
                Ok(0) => {
                    let _ = read.blocking_send(Err("the MCP program closed its output".into()));
                    break;
                }
                Err(e) => {
                    let _ = read.blocking_send(Err(format!("reading MCP output: {e}")));
                    break;
                }
                Ok(_) => {}
            }
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if line.len() > mcp_message::MAX_LINE {
                let _ = read.blocking_send(Err("an MCP output line exceeds 32 MiB".into()));
                break;
            }
            if mcp_message::parse(&line).is_ok() {
                if read.blocking_send(Ok(line)).is_err() {
                    break;
                }
            } else {
                append(&tail, &line);
                append(&tail, b"\n");
            }
        }
    });
    let tail = log.clone();
    std::thread::spawn(move || {
        let mut bytes = [0; 4096];
        while let Ok(n) = stderr.read(&mut bytes) {
            if n == 0 {
                break;
            }
            append(&tail, &bytes[..n]);
        }
    });
    let waiting = child.clone();
    std::thread::spawn(move || {
        let _ = waiting.wait();
        let _ = waiting.kill();
    });
    Ok(Process { input, output, log, child })
}

fn append(log: &Mutex<Vec<u8>>, bytes: &[u8]) {
    const MOST: usize = 64 * 1024;
    let mut log = log.lock().expect("never poisoned");
    let bytes = &bytes[bytes.len().saturating_sub(MOST)..];
    let discard = (log.len() + bytes.len()).saturating_sub(MOST);
    log.drain(..discard);
    log.extend_from_slice(bytes);
}
