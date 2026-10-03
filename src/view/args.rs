//! Native argument and NUL-delimited filename handling.

use clap::Args;
use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

/// One-based inclusive line or page selection.
#[derive(Clone, Copy, Debug)]
pub struct Range {
    pub(super) first: usize,
    pub(super) last: usize,
}

impl std::str::FromStr for Range {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (first, last) = value.split_once('-').unwrap_or((value, value));
        let parse = |part: &str| part.parse::<usize>().ok().filter(|value| *value > 0);
        let first = parse(first).ok_or("Use a one-based range such as 120-180 or 3.")?;
        let last = parse(last)
            .filter(|last| *last >= first)
            .ok_or("Use a one-based range such as 120-180 or 3.")?;
        Ok(Self { first, last })
    }
}

/// Arguments shared by the executable and an in-process shell entry point.
#[derive(Args, Clone, Debug)]
#[command(
    after_help = "Paths are read in argument order, without sorting or expansion.\nText uses line<TAB>content. Images always use OSC 1337 inline sequences.\nPDFs show their text layer unless --image is specified.\n\nExamples:\n  fastctx view -- README.md src/main.rs\n  fastctx view --lines 120-180 -- src/main.rs\n  fastctx view --pages 3-5 -- design.pdf\n  fastctx view --image --pages 3 -- design.pdf\n  fastctx view --encoding gbk -- 'D:/project/旧记录.txt'\n  rg -l0 -- SessionContext src | fastctx view --files0-from -"
)]
pub struct ViewArgs {
    /// Inclusive text lines; applies to every text file and each PDF page.
    #[arg(long, value_name = "FIRST-LAST")]
    pub lines: Option<Range>,
    /// Inclusive PDF pages; omitted means every page, until the output budget is reached.
    #[arg(long, value_name = "FIRST-LAST")]
    pub pages: Option<Range>,
    /// Render selected PDF pages as images.
    #[arg(long)]
    pub image: bool,
    /// Source text encoding (WHATWG label, utf-32le, or utf-32be).
    #[arg(long, value_name = "LABEL")]
    pub encoding: Option<String>,
    /// Append NUL-separated filenames from FILE; use - for stdin.
    #[arg(long, value_name = "FILE")]
    pub files0_from: Option<PathBuf>,
    /// Files to show; use -- before a filename starting with a hyphen.
    #[arg(value_name = "PATH", required_unless_present = "files0_from")]
    pub paths: Vec<PathBuf>,
}

pub(super) fn paths(args: &ViewArgs) -> Result<Vec<PathBuf>, String> {
    let mut paths = args.paths.clone();
    if let Some(source) = &args.files0_from {
        let mut input: Box<dyn BufRead> = if source.as_os_str() == "-" {
            Box::new(BufReader::new(std::io::stdin()))
        } else {
            Box::new(BufReader::new(
                std::fs::File::open(native_path(source)).map_err(|error| error.to_string())?,
            ))
        };
        loop {
            let mut bytes = Vec::new();
            if input
                .read_until(0, &mut bytes)
                .map_err(|error| error.to_string())?
                == 0
            {
                break;
            }
            if bytes.last() == Some(&0) {
                bytes.pop();
            }
            if bytes.is_empty() {
                return Err("--files0-from contains an empty filename.".to_string());
            }
            #[cfg(unix)]
            let name = {
                use std::os::unix::ffi::OsStringExt;
                OsString::from_vec(bytes)
            };
            #[cfg(not(unix))]
            let name = OsString::from(
                String::from_utf8(bytes)
                    .map_err(|error| format!("--files0-from filename is not UTF-8: {error}"))?,
            );
            paths.push(PathBuf::from(name));
        }
    }
    Ok(paths.into_iter().map(|path| native_path(&path)).collect())
}

fn native_path(path: &std::path::Path) -> PathBuf {
    #[cfg(windows)]
    if let Some(path) = path.to_str() {
        return crate::paths::parse_input_path(path);
    }
    path.to_path_buf()
}
