//! Unix transports bytes and resize notifications; VT interpretation is shared
//! with Windows. Queue state, never elapsed time, resolves an escape boundary.

use std::io::{self, IsTerminal};
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;

use mio::{Events, Interest, Poll, Token, Waker, unix::SourceFd};
use rustix::{fs::OFlags, io::Errno};
use signal_hook_mio::v1_0::Signals;
use tokio::sync::mpsc::UnboundedSender;

use super::{Input, decode::Decoder, utf8::Utf8};

const INPUT: Token = Token(0);
const RESIZE: Token = Token(1);
const STOP: Token = Token(2);
static READER: Mutex<Option<Reader>> = Mutex::new(None);

struct Reader {
    input: Arc<OwnedFd>,
    before: OFlags,
    stop: Arc<AtomicBool>,
    wake: Waker,
    thread: JoinHandle<()>,
}

pub(super) fn start(sender: UnboundedSender<Input>) -> io::Result<()> {
    let mut slot = READER.lock().map_err(|_| io::Error::other("terminal reader lock poisoned"))?;
    if slot.is_some() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "terminal input is already being read"));
    }
    let input = Arc::new(if std::io::stdin().is_terminal() {
        rustix::io::dup(rustix::stdio::stdin())?
    } else {
        std::fs::File::options().read(true).open("/dev/tty")?.into()
    });
    let poll = Poll::new()?;
    poll.registry().register(&mut SourceFd(&input.as_raw_fd()), INPUT, Interest::READABLE)?;
    let mut signals = Signals::new([signal_hook::consts::SIGWINCH])?;
    poll.registry().register(&mut signals, RESIZE, Interest::READABLE)?;
    let wake = Waker::new(poll.registry(), STOP)?;
    let stop = Arc::new(AtomicBool::new(false));
    let before = rustix::fs::fcntl_getfl(&input)?;
    // Readiness can be stale. Nonblocking reads also let restore wake an idle
    // reader without a timed poll. Only this flag is ours to restore.
    rustix::fs::fcntl_setfl(&input, before | OFlags::NONBLOCK)?;
    let thread_input = Arc::clone(&input);
    let thread_stop = Arc::clone(&stop);
    super::focus::set(super::Focus::Unknown);
    let thread = std::thread::Builder::new().name("terminal input".into()).spawn(move || {
        if let Err(error) = read(&thread_input, &thread_stop, poll, signals, &sender) {
            let _ = super::send(&sender, Input::Failed(error.to_string()));
        }
    });
    match thread {
        Ok(thread) => {
            *slot = Some(Reader { input, before, stop, wake, thread });
            Ok(())
        }
        Err(error) => {
            restore_flags(&input, before)?;
            Err(error)
        }
    }
}

pub(super) fn restore() -> io::Result<()> {
    let mut slot = READER.lock().map_err(|_| io::Error::other("terminal reader lock poisoned"))?;
    let Some(reader) = slot.as_ref() else { return Ok(()) };
    reader.stop.store(true, Ordering::Release);
    if !reader.thread.is_finished() {
        reader.wake.wake()?;
    }
    let reader = slot.take().unwrap();
    let joined = reader.thread.join();
    restore_flags(&reader.input, reader.before)?;
    joined.map_err(|_| io::Error::other("terminal input thread panicked"))
}

fn restore_flags(input: &OwnedFd, before: OFlags) -> io::Result<()> {
    let mut flags = rustix::fs::fcntl_getfl(input)?;
    flags.set(OFlags::NONBLOCK, before.contains(OFlags::NONBLOCK));
    Ok(rustix::fs::fcntl_setfl(input, flags)?)
}

fn read(
    input: &OwnedFd,
    stop: &AtomicBool,
    mut poll: Poll,
    mut signals: Signals,
    sender: &UnboundedSender<Input>,
) -> io::Result<()> {
    let mut decoder = Decoder::default();
    let mut utf8 = Utf8::default();
    // There are three registered sources: terminal, SIGWINCH, and cancellation.
    let mut events = Events::with_capacity(3);
    // This is one read buffer, not a limit on any event or paste.
    let mut buffer = [0u8; 4096];
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        match poll.poll(&mut events, None) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
        for event in &events {
            if stop.load(Ordering::Acquire) {
                return Ok(());
            }
            match event.token() {
                INPUT => loop {
                    if stop.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    let mut out = Vec::new();
                    let count = match rustix::io::read(input, &mut buffer) {
                        Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "terminal input closed")),
                        Ok(count) => count,
                        Err(Errno::INTR) => continue,
                        Err(Errno::AGAIN) => {
                            decoder.boundary(utf8.pending(), &mut out);
                            for event in out {
                                if super::send(sender, event).is_err() {
                                    return Ok(());
                                }
                            }
                            break;
                        }
                        Err(error) => return Err(error.into()),
                    };
                    let decoded = utf8.push(&buffer[..count], &mut decoder, &mut out);
                    let waiting = rustix::io::ioctl_fionread(input).map_err(io::Error::from);
                    if let Ok(waiting) = waiting.as_ref() {
                        // A partial scalar is already evidence of a following
                        // character; do not separate its preceding Alt prefix.
                        decoder.boundary(*waiting != 0 || utf8.pending(), &mut out);
                    }
                    // Resize notifications must also make progress while an
                    // arbitrarily long paste keeps the input queue readable.
                    if signals.pending().next().is_some() {
                        out.push(Input::Resize);
                    }
                    for event in out {
                        if super::send(sender, event).is_err() {
                            return Ok(());
                        }
                    }
                    decoded?;
                    // Drain to WouldBlock even after an empty queue snapshot:
                    // bytes may arrive between the query and the next read.
                    waiting?;
                },
                RESIZE => {
                    if signals.pending().next().is_some() && super::send(sender, Input::Resize).is_err() {
                        return Ok(());
                    }
                }
                STOP => {}
                _ => unreachable!("only the three input sources are registered"),
            }
        }
    }
}
