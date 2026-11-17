//! Worker-only scheduling and parent lifetime hooks, before request code runs.

use std::io;

#[cfg(windows)]
pub(super) fn prepare() -> io::Result<()> {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, IDLE_PRIORITY_CLASS, SetPriorityClass};
    // SAFETY: lowers the current isolated process, not a shared pool thread.
    if unsafe { SetPriorityClass(GetCurrentProcess(), IDLE_PRIORITY_CLASS) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn prepare() -> io::Result<()> {
    // SAFETY: changes only this isolated process's scheduling priority.
    if unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 19) } == -1 {
        return Err(io::Error::last_os_error());
    }
    follow_parent()
}

#[cfg(target_os = "linux")]
fn follow_parent() -> io::Result<()> {
    // Child starts us with SIGKILL on parent death. Before any role can start
    // descendants, replace it with a handler that kills our whole group.
    unsafe {
        let parent = libc::getppid();
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = end_group as *const () as usize;
        libc::sigemptyset(&mut action.sa_mask);
        if libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut()) == -1
            || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0) == -1
        {
            return Err(io::Error::last_os_error());
        }
        if libc::getppid() != parent {
            end_group(0);
        }
    }
    Ok(())
}

#[cfg(unix)]
extern "C" fn end_group(_: libc::c_int) {
    // SAFETY: these calls are async-signal-safe. A directly invoked `work`
    // must never kill the interactive shell's process group.
    unsafe {
        if libc::getpgrp() == libc::getpid() {
            libc::killpg(libc::getpid(), libc::SIGKILL);
        }
        libc::_exit(1);
    }
}

#[cfg(target_os = "macos")]
fn follow_parent() -> io::Result<()> {
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
    // SAFETY: kqueue returns an owned descriptor; getppid reads process state.
    let (raw, parent) = unsafe { (libc::kqueue(), libc::getppid()) };
    if raw == -1 {
        return Err(io::Error::last_os_error());
    }
    let queue = unsafe { OwnedFd::from_raw_fd(raw) };
    let change = libc::kevent {
        ident: parent as libc::uintptr_t,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    // SAFETY: registers one valid change; no event buffer is supplied.
    if unsafe { libc::kevent(queue.as_raw_fd(), &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::getppid() } != parent {
        end_group(0);
    }
    std::thread::Builder::new().name("worker parent".into()).stack_size(64 * 1024).spawn(move || {
        let _role = crate::roles::scoped(crate::roles::Role::Blocking);
        loop {
            let mut event: libc::kevent = unsafe { std::mem::zeroed() };
            // SAFETY: one initialized queue and one event-sized output buffer.
            let got = unsafe { libc::kevent(queue.as_raw_fd(), std::ptr::null(), 0, &mut event, 1, std::ptr::null()) };
            if got == -1 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            end_group(0);
        }
    })?;
    Ok(())
}
