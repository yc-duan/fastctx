//! Identify paths in project-memory projections before any filesystem mutation.

use fastctx_types::{TownName, memory::Kind};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub town: TownName,
    pub kind: Kind,
    pub name: String,
    pub evidence: Option<String>,
}

/// A path through a link into memory is memory too; a replica redirected elsewhere is refused.
pub fn locate(world: &Path, path: &Path) -> Result<Option<Target>, String> {
    let native = fastctx_platform::local_path(path).map_err(|error| error.to_string())?;
    if !native.is_absolute() {
        return Ok(None);
    }
    let root = crate::files::canonical_path(world).map_err(|error| error.to_string())?;
    let lexical = parse(&native, world)?.or(parse(&native, &root)?);
    let resolved = crate::files::canonical_path(&native).map_err(|error| error.to_string())?;
    let actual = parse(&resolved, &root)?;
    if lexical.is_some() && lexical != actual {
        return Err(fastctx_types::memory::words::REDIRECTED_PATH.into());
    }
    Ok(actual.or(lexical))
}

fn parse(path: &Path, world: &Path) -> Result<Option<Target>, String> {
    let mut path = path.components();
    for part in world.components().chain(Path::new("towns").components()) {
        let Some(next) = path.next() else { return Ok(None) };
        let same = if cfg!(windows) { next.as_os_str().eq_ignore_ascii_case(part.as_os_str()) } else { next == part };
        if !same {
            return Ok(None);
        }
    }
    let parts: Vec<_> = path.map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
    let [town, folder, rest @ ..] = parts.as_slice() else { return Ok(None) };
    let same = |name: &str| if cfg!(windows) { folder.eq_ignore_ascii_case(name) } else { folder == name };
    let kind = if same("journals") {
        Kind::Journal
    } else if same("specs") {
        Kind::Spec
    } else {
        return Ok(None);
    };
    let town = TownName::try_from(town.as_str()).map_err(|error| error.to_string())?;
    let (name, evidence) = match rest {
        [file] => (file.strip_suffix(".md").ok_or(fastctx_types::memory::words::MARKDOWN_EXTENSION)?.to_string(), None),
        [name, files @ ..] if kind == Kind::Journal && !files.is_empty() => (name.clone(), Some(files.join("/"))),
        _ => return Err(fastctx_types::memory::words::WRITE_TARGET.into()),
    };
    fastctx_types::memory::markdown::check_name(&name)?;
    if evidence.as_ref().is_some_and(|name| name.split('/').any(|part| part.is_empty() || part == "." || part == ".."))
    {
        return Err(fastctx_types::memory::words::EVIDENCE_FOLDER.into());
    }
    Ok(Some(Target { town, kind, name, evidence }))
}
