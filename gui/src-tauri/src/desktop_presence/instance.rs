//! Binding the first named-pipe instance elects the GUI process atomically.
//! The listening endpoint exists before any WebView or World connection.

use std::os::windows::io::AsRawHandle;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
use windows::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::FlushFileBuffers;
use windows::Win32::System::Pipes::GetNamedPipeServerProcessId;
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
use windows::core::HSTRING;

use super::identity::APP_ID;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Launch {
    Open(Option<String>),
    Notification(String),
    /// Exit, as the tray's quit does: World's uninstaller asks it, from any
    /// session of this user, including a remote one.
    Quit,
}

pub struct Instance {
    pipe: NamedPipeServer,
    name: String,
    pub launch: Launch,
}

fn pipe(name: &str, first: bool) -> std::io::Result<NamedPipeServer> {
    let sid = fastctx_platform::current_user_sid()?;
    let sddl = HSTRING::from(format!("O:{sid}D:P(A;;GA;;;{sid})"));
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(&sddl, SDDL_REVISION_1, &mut descriptor, None)?;
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let result = ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(name, (&mut attributes as *mut SECURITY_ATTRIBUTES).cast());
        LocalFree(Some(HLOCAL(descriptor.0)));
        result
    }
}

pub fn enter(launch: Launch) -> Result<Option<Instance>, String> {
    let sid = fastctx_platform::current_user_sid().map_err(|e| e.to_string())?;
    let name = format!(r"\\.\pipe\{APP_ID}.{sid}");
    let bound = tauri::async_runtime::block_on(async { pipe(&name, true) });
    match bound {
        Ok(pipe) => Ok(Some(Instance { pipe, name, launch })),
        Err(error) if error.raw_os_error() == Some(5) => {
            let (owner, result, timed_out) = forward(&name, &launch);
            match result {
                Ok(()) => Ok(None),
                Err(error) if timed_out => {
                    let owner = owner
                        .ok_or_else(|| format!("{error}; the busy endpoint did not expose its process identity"))?;
                    super::takeover::retire(&owner).map_err(|why| format!("{error}; {why}"))?;
                    let pipe = tauri::async_runtime::block_on(async { pipe(&name, true) })
                        .map_err(|e| format!("binding Fastctx endpoint after {} exited: {e}", owner.pid))?;
                    Ok(Some(Instance { pipe, name, launch }))
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(format!("binding the Fastctx instance endpoint: {error}")),
    }
}

/// Asks the World running for this user, in whichever session, to exit,
/// and waits until it and the processes it started (its WebView2, which
/// hold its data) have exited. No World running is nothing to wait for.
pub fn quit_running() -> Result<(), String> {
    let sid = fastctx_platform::current_user_sid().map_err(|e| e.to_string())?;
    let name = format!(r"\\.\pipe\{APP_ID}.{sid}");
    let (owner, result, _) = forward(&name, &Launch::Quit);
    let Some(world) = owner else {
        return match result {
            Err(error) if error.contains("os error 2") => Ok(()),
            other => other,
        };
    };
    let mut waited = vec![world.clone()];
    waited.extend(started_by(&world)?);
    let began = std::time::Instant::now();
    for identity in &waited {
        while fastctx_platform::is_alive(identity).map_err(|e| e.to_string())? {
            if began.elapsed() >= std::time::Duration::from_secs(30) {
                return Err(format!(
                    "waiting for Fastctx process {} to exit: {}",
                    identity.pid,
                    result.err().unwrap_or_else(|| "exit was acknowledged but the process remains".into())
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    Ok(())
}

/// The processes `world` started, and theirs: each a child by its parent's
/// id and created no earlier than that parent, so a reused id is not taken.
fn started_by(world: &fastctx_types::ProcessIdentity) -> Result<Vec<fastctx_types::ProcessIdentity>, String> {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    let mut parents = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(|e| e.to_string())?;
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            parents.push((entry.th32ProcessID, entry.th32ParentProcessID));
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = windows::Win32::Foundation::CloseHandle(snapshot);
    }
    let created = |identity: &fastctx_types::ProcessIdentity| identity.token.parse::<u64>().map_err(|e| e.to_string());
    let mut tree = vec![world.clone()];
    let mut index = 0;
    while index < tree.len() {
        let parent = tree[index].clone();
        for &(pid, _) in parents.iter().filter(|(_, ppid)| *ppid == parent.pid) {
            if let Some(child) = fastctx_platform::identity_of(pid).map_err(|e| e.to_string())?
                && created(&child)? >= created(&parent)?
                && !tree.contains(&child)
            {
                tree.push(child);
            }
        }
        index += 1;
    }
    tree.remove(0);
    Ok(tree)
}

fn forward(name: &str, launch: &Launch) -> (Option<fastctx_types::ProcessIdentity>, Result<(), String>, bool) {
    let mut owner = None;
    let result = tauri::async_runtime::block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut pipe = loop {
                match ClientOptions::new().open(name) {
                    Ok(pipe) => break pipe,
                    Err(error) if error.raw_os_error() == Some(231) => {
                        tokio::time::sleep(std::time::Duration::from_millis(25)).await
                    }
                    Err(error) => return Err(format!("opening Fastctx instance endpoint {name}: {error}")),
                }
            };
            let mut pid = 0;
            unsafe { GetNamedPipeServerProcessId(HANDLE(pipe.as_raw_handle()), &mut pid) }
                .map_err(|e| e.to_string())?;
            owner =
                fastctx_platform::identity_of(pid).map_err(|e| format!("identifying Fastctx pipe owner {pid}: {e}"))?;
            let permission = unsafe { AllowSetForegroundWindow(pid) };
            let mut request = serde_json::to_vec(launch).map_err(|e| e.to_string())?;
            request.push(b'\n');
            pipe.write_all(&request).await.map_err(|e| format!("sending launch to Fastctx {pid}: {e}"))?;
            let mut response = String::new();
            tokio::io::BufReader::new(pipe)
                .read_line(&mut response)
                .await
                .map_err(|e| format!("reading Fastctx {pid} acknowledgement: {e}"))?;
            serde_json::from_str::<Result<(), String>>(&response)
                .map_err(|e| format!("Fastctx {pid} did not acknowledge launch: {e}"))?
                .map_err(|error| match permission {
                    Ok(()) => error,
                    Err(why) => format!("{error}; transferring foreground permission: {why}"),
                })
        })
        .await
    });
    match result {
        Ok(result) => (owner, result, false),
        Err(_) => {
            (owner, Err(format!("Fastctx instance endpoint {name} did not acknowledge within five seconds")), true)
        }
    }
}

impl Instance {
    pub async fn serve(self, app: tauri::AppHandle) -> Result<(), String> {
        let mut waiting = self.pipe;
        loop {
            waiting.connect().await.map_err(|e| e.to_string())?;
            // Keep a bound instance throughout each handoff. No second process
            // can ever bind with FILE_FLAG_FIRST_PIPE_INSTANCE while we live.
            let next = pipe(&self.name, false).map_err(|e| e.to_string())?;
            let connected = waiting;
            waiting = next;
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let mut stream = tokio::io::BufReader::new(connected);
                let mut line = String::new();
                let result = async {
                    stream.read_line(&mut line).await.map_err(|e| e.to_string())?;
                    let launch = serde_json::from_str(&line).map_err(|e| format!("invalid Fastctx launch: {e}"))?;
                    super::launch(&app, launch).await
                }
                .await;
                if let Err(error) = &result {
                    super::report(&app, error.clone());
                }
                let mut reply = serde_json::to_vec(&result).expect("serializing launch result");
                reply.push(b'\n');
                if let Err(error) = stream.get_mut().write_all(&reply).await {
                    super::report(&app, format!("acknowledging Fastctx launch: {error}"));
                    return;
                }
                // Tokio's flush does not drain a Windows pipe. Keep the server
                // handle alive until the client has consumed the acknowledgement.
                let pipe = stream.into_inner();
                let drained = tauri::async_runtime::spawn_blocking(move || unsafe {
                    FlushFileBuffers(HANDLE(pipe.as_raw_handle()))
                })
                .await;
                match drained {
                    Ok(Ok(())) => (),
                    Ok(Err(error)) => super::report(&app, format!("draining Fastctx launch acknowledgement: {error}")),
                    Err(error) => super::report(&app, format!("joining Fastctx launch acknowledgement: {error}")),
                }
            });
        }
    }
}
