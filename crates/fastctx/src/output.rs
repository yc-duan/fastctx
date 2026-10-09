//! Standard output of the commands whose output a pipeline may cut short,
//! as `fastctx view big.log | head` does. Once the reader of standard output
//! is gone, the command ends quietly with status 141, as a Unix tool ends on
//! SIGPIPE and as bash then reports it: the pipe's end is not a failure of
//! the command, and saying so would put a false error before whoever reads
//! the script's output. Other pipes, such as the one to the node, keep
//! failing loudly.

use std::io::{self, StdoutLock, Write};

/// The status bash gives a command a closed pipe ended: 128 + SIGPIPE.
const READER_LEFT: i32 = 141;

/// Standard output, locked.
pub struct Out(StdoutLock<'static>);

pub fn out() -> Out {
    Out(io::stdout().lock())
}

/// Writes all of `text` to standard output.
pub fn print(text: &str) -> io::Result<()> {
    let mut out = out();
    out.write_all(text.as_bytes())?;
    out.flush()
}

fn checked<T>(result: io::Result<T>) -> io::Result<T> {
    if let Err(error) = &result
        && error.kind() == io::ErrorKind::BrokenPipe
    {
        std::process::exit(READER_LEFT);
    }
    result
}

impl Write for Out {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        checked(self.0.write(bytes))
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        checked(self.0.write_all(bytes))
    }

    fn flush(&mut self) -> io::Result<()> {
        checked(self.0.flush())
    }
}
