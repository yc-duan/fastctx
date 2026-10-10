//! Readable notices travel with copied executables and extracted ripgrep.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use fastctx_platform::Staged;

pub const APACHE: &str = include_str!("../../../LICENSE-APACHE");
pub const NOTICE: &str = include_str!("../../../NOTICE");
pub const THIRD_PARTY: &str = include_str!(concat!(env!("OUT_DIR"), "/THIRD_PARTY_LICENSES.md"));

/// Writes the complete licenses without contacting or starting a World.
pub fn write(mut output: impl Write) -> io::Result<()> {
    for (name, text) in files() {
        writeln!(output, "# {name}\n\n{text}")?;
    }
    Ok(())
}

/// Places exact build notices beside an extracted program. Existing notices
/// for a different build are replaced atomically, just like the program.
pub(crate) fn place(directory: &Path) -> io::Result<()> {
    for (name, text) in files() {
        let path = directory.join(name);
        match fs::read(&path) {
            Ok(bytes) if bytes == text.as_bytes() => continue,
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
        let mut staged = Staged::create(&path, Some(0o644))?;
        staged.write_all(text.as_bytes())?;
        staged.commit()?;
    }
    Ok(())
}

pub fn files() -> [(&'static str, &'static str); 3] {
    [("LICENSE-APACHE", APACHE), ("NOTICE", NOTICE), ("THIRD_PARTY_LICENSES.md", THIRD_PARTY)]
}
