//! Input budgets for local file operations, shared by both transport entrances.

pub const PATH_BYTES: usize = 4096;
pub const QUERY_BYTES: usize = 4096;
pub const ROOTS: usize = 32;
pub const SEARCH_RESULTS: usize = 200;
pub const ID_BYTES: usize = 128;
pub const SEEN_FILES: usize = 64;
pub const MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

pub fn text(value: &str, maximum: usize, name: &str) -> Result<(), String> {
    if value.len() > maximum { Err(format!("{name} exceeds its {maximum}-byte limit")) } else { Ok(()) }
}

pub fn paths(paths: &[String]) -> Result<(), String> {
    if paths.len() > ROOTS {
        return Err(format!("at most {ROOTS} paths may be requested at once"));
    }
    for path in paths {
        text(path, PATH_BYTES, "path")?;
    }
    Ok(())
}
