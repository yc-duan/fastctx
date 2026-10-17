//! Create the node with its own primary token, private inherited pipes and a
//! kill-on-close job. Explicitly detached world jobs may break away and survive
//! an idle node handoff; ordinary node-owned children retain its lifetime.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, PipeReader, PipeWriter};
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::io::{AsRawHandle as _, BorrowedHandle, FromRawHandle as _, OwnedHandle};
use std::ptr;
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
    mpsc::SyncSender,
};

use anyhow::{Context as _, bail};
use fastctx_platform::service_channel::{ServiceCommand, ServiceEvent};
use windows_sys::Win32::Foundation::{HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0};
use windows_sys::Win32::Security::{ImpersonateLoggedOnUser, RevertToSelf};
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST,
    PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
};

use super::super::Kind;
use super::account::{Environment, Identity, Login};
use super::config::Config;
use super::runtime::{Event, Instance};
use super::security::Descriptor;

#[path = "output.rs"]
mod output;

pub(super) struct Child {
    pub instance: Arc<Instance>,
    pub identity: Identity,
    pub pid: u32,
    pub process: OwnedHandle,
    job: OwnedHandle,
    control: PipeWriter,
    expected: Arc<AtomicU8>,
    watchers: Vec<std::thread::JoinHandle<()>>,
    // Kept until the node is gone; a replacement loads the same profile
    // before this reference is released during a handoff.
    login: Option<Rc<Login>>,
}

impl Child {
    pub fn spawn(config: &Config, login: Option<Login>, events: SyncSender<Event>) -> anyhow::Result<Self> {
        let user = config.target.as_ref().map(|target| target.sid.as_str());
        let (control_read, control) = private_pipe(user)?;
        let (event_read, event_write) = private_pipe(user)?;
        let identity = login.as_ref().map(|login| login.identity).unwrap_or(Identity::System);
        let token = login.as_ref().map(|login| login.token.as_raw_handle());
        if (config.kind == Kind::Node && !config.systems()) != token.is_some() {
            bail!("a node of an account cannot be launched under the guardian's SYSTEM identity");
        }
        let environment = Environment::new(token).context("creating the user's environment block")?;
        let executable =
            if config.kind == Kind::Hub { config.root()?.join("payload.exe") } else { config.executable() };
        let mut args: Vec<OsString> =
            if config.kind == Kind::Node { vec!["node".into()] } else { vec!["hub".into(), "serve".into()] };
        args.extend(["--dir".into(), config.directory().into_os_string()]);
        if config.kind == Kind::Node {
            args.extend([
                "--supervised".into(),
                "--supervisor-control".into(),
                (control_read.as_raw_handle() as usize).to_string().into(),
                "--supervisor-events".into(),
                (event_write.as_raw_handle() as usize).to_string().into(),
                "--service-identity".into(),
                (match identity {
                    Identity::S4u => "s4u",
                    Identity::Interactive(_) => "interactive",
                    Identity::System => "system",
                })
                .into(),
            ]);
        }
        let program = wide(executable.as_os_str())?;
        let cwd = wide(login.as_ref().map(|login| login.home.as_os_str()).unwrap_or(config.directory().as_os_str()))?;
        let mut command = command_line(executable.as_os_str(), &args)?;
        let job = new_job()?;
        // User-controlled log paths are opened under that user's token. A
        // reparse point there cannot turn the guardian into a SYSTEM writer.
        let impersonation = Impersonation::new(token)?;
        let log = if config.kind == Kind::Node {
            config.directory().join("service-node.log")
        } else {
            config.root()?.join("service-hub.log")
        };
        let log = fastctx_platform::rolling_log::RollingLog::shared(&log).context("opening the foreground log")?;
        let (output_read, output) = private_pipe(user)?;
        let output_token = token
            .map(|token| {
                // SAFETY: the login owns this live token throughout creation.
                unsafe { BorrowedHandle::borrow_raw(token) }.try_clone_to_owned()
            })
            .transpose()?;
        let input = File::open(r"\\.\NUL").context("opening null stdin")?;
        let mut handles = vec![input.as_raw_handle(), output.as_raw_handle()];
        if config.kind == Kind::Node {
            handles.extend([control_read.as_raw_handle(), event_write.as_raw_handle()]);
        }
        for &handle in &handles {
            // SAFETY: only the explicitly-listed child ends are inheritable.
            if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
                return Err(io::Error::last_os_error().into());
            }
        }
        let mut attributes = Attributes::new()?;
        attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize, &handles)?;
        let jobs = [job.as_raw_handle()];
        attributes.set(PROC_THREAD_ATTRIBUTE_JOB_LIST as usize, &jobs)?;
        let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = input.as_raw_handle();
        startup.StartupInfo.hStdOutput = output.as_raw_handle();
        startup.StartupInfo.hStdError = output.as_raw_handle();
        startup.lpAttributeList = attributes.pointer();
        let mut info: PROCESS_INFORMATION = unsafe { zeroed() };
        let flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW;
        // SAFETY: every pointer and explicit handle lives through creation;
        // the process joins the job before it can execute its first instruction.
        let made = unsafe {
            match token {
                Some(token) => CreateProcessAsUserW(
                    token,
                    program.as_ptr(),
                    command.as_mut_ptr(),
                    ptr::null(),
                    ptr::null(),
                    1,
                    flags,
                    environment.as_ptr(),
                    cwd.as_ptr(),
                    &startup.StartupInfo,
                    &mut info,
                ),
                None => CreateProcessW(
                    program.as_ptr(),
                    command.as_mut_ptr(),
                    ptr::null(),
                    ptr::null(),
                    1,
                    flags,
                    environment.as_ptr(),
                    cwd.as_ptr(),
                    &startup.StartupInfo,
                    &mut info,
                ),
            }
        };
        let error = io::Error::last_os_error();
        drop(impersonation);
        if made == 0 {
            return Err(error).context("creating the foreground process");
        }
        // SAFETY: CreateProcess returned two owned handles.
        let process = unsafe {
            drop(OwnedHandle::from_raw_handle(info.hThread));
            OwnedHandle::from_raw_handle(info.hProcess)
        };
        drop((control_read, event_write, output, input));
        let instance = Arc::new(Instance);
        let expected = Arc::new(AtomicU8::new(1));
        let mut child = Self {
            instance,
            identity,
            pid: info.dwProcessId,
            process,
            job,
            control,
            expected,
            watchers: Vec::new(),
            login: login.map(Rc::new),
        };
        let watching = (|| {
            child.watchers.push(output::start(output_read, log, output_token)?);
            child.watchers.push(watch_exit(&child.process, child.instance.clone(), events.clone())?);
            if config.kind == Kind::Node {
                child.watchers.push(watch_messages(
                    event_read,
                    child.instance.clone(),
                    child.expected.clone(),
                    events,
                )?);
            }
            Ok::<_, anyhow::Error>(())
        })();
        if let Err(error) = watching {
            child.terminate()?;
            return Err(error);
        }
        Ok(child)
    }

    pub fn profile_lease(&self) -> Option<Rc<Login>> {
        self.login.clone()
    }

    pub fn send(&mut self, command: ServiceCommand) -> anyhow::Result<()> {
        // Once Yielded has been read, even an already-in-flight cancellation
        // cannot authorize another response from that terminal instance.
        if self.expected.load(Ordering::SeqCst) == 0x80 {
            return Ok(());
        }
        self.expected.store(
            match command {
                ServiceCommand::Activate => 2,
                ServiceCommand::YieldWhenIdle => 4,
                ServiceCommand::CancelYield => 2 | 4,
            },
            Ordering::SeqCst,
        );
        command.write(&mut self.control).context("sending a node lifecycle command")
    }

    pub fn terminate(&self) -> anyhow::Result<()> {
        // SAFETY: this job belongs only to this foreground node and its
        // ordinary descendants. Explicitly detached jobs are outside it.
        if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        wait(&self.process)
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        // Close any ordinary descendants' inherited pipe ends before joining
        // readers. Detached world jobs are not members of this job.
        let _ = self.terminate();
        for watcher in self.watchers.drain(..) {
            let _ = watcher.join();
        }
    }
}

pub(super) fn private_pipe(user: Option<&str>) -> anyhow::Result<(PipeReader, PipeWriter)> {
    let user = user.map(|sid| format!("(A;;GRGW;;;{sid})")).unwrap_or_default();
    let descriptor = Descriptor::new(&format!("D:P(A;;GA;;;SY)(A;;GA;;;BA){user}S:(ML;;NW;;;ME)"))?;
    let (mut read, mut write) = (ptr::null_mut(), ptr::null_mut());
    // Anonymous pipes are implemented with kernel pipe objects. Give them
    // explicit ACLs rather than relying on an undiscoverable name or defaults.
    if unsafe { CreatePipe(&mut read, &mut write, &descriptor.attributes(), 0) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(unsafe {
        (PipeReader::from(OwnedHandle::from_raw_handle(read)), PipeWriter::from(OwnedHandle::from_raw_handle(write)))
    })
}

fn watch_exit(
    process: &OwnedHandle,
    instance: Arc<Instance>,
    events: SyncSender<Event>,
) -> anyhow::Result<std::thread::JoinHandle<()>> {
    let process = process.try_clone()?;
    Ok(std::thread::Builder::new().name("guardian child exit".into()).spawn(move || {
        let exit = wait(&process).and_then(|()| {
            let mut code = 0;
            if unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } == 0 {
                return Err(io::Error::last_os_error().into());
            }
            Ok(code)
        });
        let _ = events.send(Event::Exited(instance, exit));
    })?)
}

fn watch_messages(
    mut pipe: PipeReader,
    instance: Arc<Instance>,
    expected: Arc<AtomicU8>,
    events: SyncSender<Event>,
) -> anyhow::Result<std::thread::JoinHandle<()>> {
    Ok(std::thread::Builder::new().name("guardian child events".into()).spawn(move || {
        loop {
            match ServiceEvent::read(&mut pipe) {
                Ok(Some(event)) => {
                    let bit = match event {
                        ServiceEvent::Prepared => 1,
                        ServiceEvent::Serving => 2,
                        ServiceEvent::Yielded => 4,
                    };
                    let accepted = expected
                        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |mask| {
                            if mask & bit != 0 {
                                Some(match event {
                                    ServiceEvent::Yielded => 0x80,
                                    ServiceEvent::Serving => 4,
                                    ServiceEvent::Prepared => 0,
                                })
                            } else {
                                None
                            }
                        })
                        .is_ok();
                    if !accepted {
                        let _ = events.send(Event::ProtocolFailed(
                            instance,
                            io::Error::new(io::ErrorKind::InvalidData, "unsolicited node lifecycle event"),
                        ));
                        break;
                    }
                    if events.send(Event::Node(instance.clone(), event)).is_err() || event == ServiceEvent::Yielded {
                        break;
                    }
                }
                Ok(None) => {
                    let _ = events.send(Event::ProtocolFailed(
                        instance,
                        io::Error::new(io::ErrorKind::UnexpectedEof, "node closed its lifecycle channel"),
                    ));
                    break;
                }
                Err(error) => {
                    let _ = events.send(Event::ProtocolFailed(instance, error));
                    break;
                }
            }
        }
    })?)
}

fn wait(process: &OwnedHandle) -> anyhow::Result<()> {
    if unsafe { WaitForSingleObject(process.as_raw_handle(), INFINITE) } != WAIT_OBJECT_0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

fn new_job() -> anyhow::Result<OwnedHandle> {
    // SAFETY: an unnamed job, exclusively owned by the guardian.
    let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    let job = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
    if unsafe {
        SetInformationJobObject(
            raw,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of_val(&limits) as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    Ok(job)
}

struct Impersonation(bool);
impl Impersonation {
    fn new(token: Option<HANDLE>) -> anyhow::Result<Self> {
        if let Some(token) = token
            && unsafe { ImpersonateLoggedOnUser(token) } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self(token.is_some()))
    }
}
impl Drop for Impersonation {
    fn drop(&mut self) {
        // Continuing under an unintended identity after a failed revert is
        // forbidden by the RevertToSelf contract; SCM will restart the host.
        if self.0 && unsafe { RevertToSelf() } == 0 {
            std::process::abort();
        }
    }
}

pub(super) struct Attributes(Vec<usize>);
impl Attributes {
    pub(super) fn new() -> anyhow::Result<Self> {
        let mut size = 0;
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 2, 0, &mut size);
        }
        let mut buffer = vec![0; size.div_ceil(size_of::<usize>())];
        if unsafe { InitializeProcThreadAttributeList(buffer.as_mut_ptr().cast(), 2, 0, &mut size) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self(buffer))
    }
    pub(super) fn pointer(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.0.as_mut_ptr().cast()
    }
    pub(super) fn set<T>(&mut self, kind: usize, data: &[T]) -> anyhow::Result<()> {
        if unsafe {
            UpdateProcThreadAttribute(
                self.pointer(),
                0,
                kind,
                data.as_ptr().cast(),
                std::mem::size_of_val(data),
                ptr::null_mut(),
                ptr::null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.pointer());
        }
    }
}

pub(super) fn wide(value: &OsStr) -> anyhow::Result<Vec<u16>> {
    let units: Vec<u16> = value.encode_wide().collect();
    if units.contains(&0) {
        bail!("a Windows argument cannot contain NUL");
    }
    Ok(units.into_iter().chain([0]).collect())
}

pub(super) fn command_line(program: &OsStr, args: &[OsString]) -> anyhow::Result<Vec<u16>> {
    let mut line = vec![u16::from(b'"')];
    let program = wide(program)?;
    if program.contains(&u16::from(b'"')) {
        bail!("an executable path cannot contain quotes");
    }
    line.extend(&program[..program.len() - 1]);
    line.push(u16::from(b'"'));
    for arg in args {
        line.extend([u16::from(b' '), u16::from(b'"')]);
        let units = wide(arg)?;
        let mut slashes = 0;
        for &unit in &units[..units.len() - 1] {
            if unit == u16::from(b'\\') {
                slashes += 1;
                continue;
            }
            line.extend(std::iter::repeat_n(
                u16::from(b'\\'),
                if unit == u16::from(b'"') { slashes * 2 + 1 } else { slashes },
            ));
            line.push(unit);
            slashes = 0;
        }
        line.extend(std::iter::repeat_n(u16::from(b'\\'), slashes * 2));
        line.push(u16::from(b'"'));
    }
    line.push(0);
    Ok(line)
}
