//! Copy on one filesystem through the same manifest and receiver as a network copy.

use std::fs::File;
use std::io::{self, Read as _};
use std::path::Path;

use super::{Known, Receiver, walk};
use fastctx_types::MachineName;
use fastctx_types::copy::CopyReport;

/// Hashes the source first, then verifies and publishes each destination file.
pub fn copy(from: &Path, to: &str, machine: &MachineName) -> io::Result<CopyReport> {
    copy_in(from, to, machine, None)
}

pub fn copy_protected(from: &Path, to: &str, machine: &MachineName, world: &Path) -> io::Result<CopyReport> {
    copy_in(from, to, machine, Some(world))
}

fn copy_in(from: &Path, to: &str, machine: &MachineName, world: Option<&Path>) -> io::Result<CopyReport> {
    let walked = walk(from)?;
    let receiver = Receiver::new(to, walked.manifest).map_err(io::Error::other)?;
    let mut receiver = match world {
        Some(world) => receiver.protect(world).map_err(io::Error::other)?,
        None => receiver,
    };
    let source = crate::files::canonical_path(from)?;
    let destination = crate::files::canonical_path(receiver.destination())?;
    if source == destination || (source.is_dir() && destination.starts_with(&source)) {
        return Err(io::Error::other(format!(
            "{} is the source itself or inside it",
            receiver.destination().display()
        )));
    }
    receiver.problems.extend(walked.problems);
    // One pass, with nothing to go on after: nothing is kept for later.
    let known = &mut Known::default();
    let mut buffer = vec![0; 1 << 20];
    for wanted in receiver.wanted(false, known) {
        let index = wanted.index;
        let (_, mut arriving) = receiver.arrive(index, 0, known);
        let read = (|| {
            let mut file = File::open(&walked.sources[index])?;
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                arriving.write(&buffer[..count]).map_err(io::Error::other)?;
            }
            Ok::<_, io::Error>(())
        })();
        match read {
            Ok(()) => receiver.arrived(index, arriving, known),
            Err(error) => receiver.problems.push(format!("{}: {error}", walked.sources[index].display())),
        }
    }
    receiver.link();
    Ok(CopyReport {
        from: displayed(from),
        from_machine: machine.clone(),
        to: displayed(receiver.destination()),
        to_machine: machine.clone(),
        files: receiver.files,
        bytes: receiver.bytes,
        directories: receiver.directories,
        links: receiver.links,
        unchanged: receiver.unchanged,
        problems: receiver.problems,
    })
}

fn displayed(path: &Path) -> String {
    let path = path.to_string_lossy();
    if cfg!(windows) { path.replace('\\', "/") } else { path.into_owned() }
}
